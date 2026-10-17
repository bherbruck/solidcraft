use serde::{Deserialize, Serialize};
use solidcraft_geom::{Seg2, Vec2, Vec3};

use crate::union_find::{find, union};

/// Hard caps that keep hostile input from exhausting memory or time.
pub const MAX_POINTS: usize = 20_000;
pub const MAX_CONSTRAINTS: usize = 40_000;
/// Curves need points, so this follows the point cap.
pub const MAX_CURVES: usize = MAX_POINTS;

/// Largest coordinate or dimension value (mm) a sketch accepts.
pub const MAX_COORD: f64 = 1e9;

/// A position a sketch accepts: finite and within [`MAX_COORD`].
pub(crate) fn in_range(p: Vec2) -> bool {
    p.is_finite() && p.x.abs() < MAX_COORD && p.y.abs() < MAX_COORD
}

/// [`in_range`] for a 3D position.
pub(crate) fn in_range3(p: Vec3) -> bool {
    p.is_finite() && p.x.abs() < MAX_COORD && p.y.abs() < MAX_COORD && p.z.abs() < MAX_COORD
}

/// A radius a sketch accepts: finite, positive and within [`MAX_COORD`].
pub(crate) fn radius_in_range(r: f64) -> bool {
    r.is_finite() && r > 1e-9 && r < MAX_COORD
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SketchError {
    #[error("unknown sketch entity `{0}`")]
    Unknown(String),
    #[error("`{0}` is the wrong kind of entity for this: {1}")]
    WrongKind(String, String),
    #[error("duplicate id `{0}`")]
    Duplicate(String),
    #[error("invalid value: {0}")]
    Invalid(String),
    #[error("sketch is too large")]
    TooLarge,
}

pub(crate) type Result<T> = std::result::Result<T, SketchError>;

/// A sketch point. `fixed` points are constants for the solver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SPoint {
    pub id: String,
    pub pos: Vec2,
    #[serde(default)]
    pub fixed: bool,
    /// Projected (linked) reference geometry: the id of the link that owns this point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CurveKind {
    /// Segment between two points (indices into `Sketch::points`).
    Line { a: usize, b: usize },
    /// Full circle: centre point and radius.
    Circle { c: usize, r: f64 },
    /// Counter-clockwise arc around `c` from `a` to `b` (radius = |a − c|; the solver keeps
    /// |b − c| equal to it).
    Arc { c: usize, a: usize, b: usize },
    /// Ellipse: centre `c`, end of the major axis `m`, minor radius `r`.
    Ellipse { c: usize, m: usize, r: f64 },
    /// Spline through its points (fit points) or, with `control`, a clamped B-spline of
    /// `degree` over them (control points).
    Spline {
        pts: Vec<usize>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        control: bool,
        #[serde(default = "three")]
        degree: u8,
        /// Explicit cyclic C2 fit interpolation; absent retains natural endpoints.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        periodic: bool,
    },
    /// Conic from `a` to `b` shaped by the `apex` point and `rho` in (0, 1) (0.5 parabola,
    /// less elliptical, more hyperbolic).
    Conic { a: usize, b: usize, apex: usize, rho: f64 },
}

fn three() -> u8 {
    3
}

impl CurveKind {
    /// Every point the curve uses.
    pub fn point_ids(&self) -> Vec<usize> {
        match self {
            CurveKind::Line { a, b } => vec![*a, *b],
            CurveKind::Circle { c, .. } => vec![*c],
            CurveKind::Arc { c, a, b } => vec![*c, *a, *b],
            CurveKind::Ellipse { c, m, .. } => vec![*c, *m],
            CurveKind::Spline { pts, .. } => pts.clone(),
            CurveKind::Conic { a, b, apex, .. } => vec![*a, *b, *apex],
        }
    }
    pub fn uses(&self, p: usize) -> bool {
        self.point_ids().contains(&p)
    }
    /// Renumber the points the curve uses.
    pub fn map_points(&mut self, f: &dyn Fn(usize) -> usize) {
        match self {
            CurveKind::Line { a, b } => {
                *a = f(*a);
                *b = f(*b);
            }
            CurveKind::Circle { c, .. } => *c = f(*c),
            CurveKind::Arc { c, a, b } => {
                *c = f(*c);
                *a = f(*a);
                *b = f(*b);
            }
            CurveKind::Ellipse { c, m, .. } => {
                *c = f(*c);
                *m = f(*m);
            }
            CurveKind::Spline { pts, .. } => {
                for q in pts.iter_mut() {
                    *q = f(*q);
                }
            }
            CurveKind::Conic { a, b, apex, .. } => {
                *a = f(*a);
                *b = f(*b);
                *apex = f(*apex);
            }
        }
    }
    /// Start and end points of an open curve.
    pub fn ends(&self) -> Option<(usize, usize)> {
        match self {
            CurveKind::Line { a, b } | CurveKind::Arc { a, b, .. } | CurveKind::Conic { a, b, .. } => Some((*a, *b)),
            CurveKind::Spline { periodic: true, .. } => None,
            CurveKind::Spline { pts, .. } => Some((*pts.first()?, *pts.last()?)),
            CurveKind::Circle { .. } | CurveKind::Ellipse { .. } => None,
        }
    }
    /// Free-form curves (drawn and profiled as polylines).
    pub fn is_freeform(&self) -> bool {
        matches!(self, CurveKind::Ellipse { .. } | CurveKind::Spline { .. } | CurveKind::Conic { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub id: String,
    #[serde(flatten)]
    pub kind: CurveKind,
    #[serde(default)]
    pub construction: bool,
    /// An arc drawn clockwise: stored counter-clockwise, but `<id>.start` / `<id>.end` keep the
    /// as-drawn meaning.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reversed: bool,
    /// Projected (linked) reference geometry: the id of the link that owns this curve. Its
    /// points are fixed and a linked circle keeps its radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Centerline line type: like construction (not part of profiles), drawn dash-dot; marks
    /// an axis (revolve, diameter dimensions).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub centerline: bool,
    /// Fixed (locked): its points and radius are constants for the solver.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fixed: bool,
    /// Infinite line type (#1359), for lines only: construction geometry drawn across the whole
    /// view, through its two points, which fix where it lies and which way it runs. Never part of
    /// a profile (it is construction too).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub infinite: bool,
}

/// Geometric constraints and dimensions. Indices refer to `Sketch::points` (`p`, `q`) or
/// `Sketch::curves` (`l`, `c`, `a`, `b`). Dimension values are millimetres or radians.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstraintKind {
    /// A pattern instance point is a rigid transform of its seed. Angles are radians;
    /// a circular pattern keeps its centre as a point, rather than a sampled position.
    PatternPoint {
        p: usize,
        q: usize,
        center: Option<usize>,
        angle: f64,
        offset: Vec2,
        /// Displacements along and perpendicular to live sketch lines (at most two).
        #[serde(default)]
        directions: [Option<(usize, Vec2)>; 2],
    },
    /// The independent radius of a patterned circle or ellipse follows its seed.
    PatternRadius {
        a: usize,
        b: usize,
    },
    Coincident {
        p: usize,
        q: usize,
    },
    PointOnCurve {
        p: usize,
        c: usize,
    },
    Horizontal {
        l: usize,
    },
    Vertical {
        l: usize,
    },
    HorizontalPoints {
        p: usize,
        q: usize,
    },
    VerticalPoints {
        p: usize,
        q: usize,
    },
    Parallel {
        a: usize,
        b: usize,
    },
    Perpendicular {
        a: usize,
        b: usize,
    },
    Collinear {
        a: usize,
        b: usize,
    },
    Tangent {
        a: usize,
        b: usize,
    },
    Equal {
        a: usize,
        b: usize,
    },
    Concentric {
        a: usize,
        b: usize,
    },
    Midpoint {
        p: usize,
        l: usize,
    },
    Symmetric {
        p: usize,
        q: usize,
        l: usize,
    },
    Fix {
        p: usize,
    },
    // Dimensions (driving).
    Distance {
        p: usize,
        q: usize,
        value: f64,
    },
    DistanceX {
        p: usize,
        q: usize,
        value: f64,
    },
    DistanceY {
        p: usize,
        q: usize,
        value: f64,
    },
    PointLineDistance {
        p: usize,
        l: usize,
        value: f64,
    },
    /// Distance from line `l` to the edge of circle or arc `c`, along the line's normal: the
    /// gap `|distance from the centre to the line| − radius` while the line misses the circle,
    /// or, `inside`, how far the edge reaches past the line, `radius − |distance|`, while the
    /// line cuts it. Zero is tangency either way. An arc is measured to its whole circle.
    LineCircleDistance {
        l: usize,
        c: usize,
        value: f64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        inside: bool,
    },
    Length {
        l: usize,
        value: f64,
    },
    /// Signed angle of a line's stored start-to-end direction from sketch +X (radians).
    LineAngle {
        l: usize,
        value: f64,
    },
    /// Signed logical sweep of an arc (radians), preserving clockwise and major arcs.
    ArcSweep {
        c: usize,
        value: f64,
    },
    /// Overall length of a straight slot: cap-centre distance plus both cap radii.
    SlotOverallLength {
        a: usize,
        b: usize,
        value: f64,
    },
    Radius {
        c: usize,
        value: f64,
    },
    Diameter {
        c: usize,
        value: f64,
    },
    /// Angle from line `a` to line `b`, radians, counter-clockwise. With `flip` the angle is
    /// measured to the reversed direction of one line (the constraint holds `value + 180°`), which
    /// lets a dimension pick any of the four angles between two lines.
    Angle {
        a: usize,
        b: usize,
        value: f64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
    },
    /// Curvature continuity (G2) where curves `a` and `b` share an end point: tangent and
    /// equal curvature.
    Smooth {
        a: usize,
        b: usize,
    },
    /// Length along an arc.
    ArcLength {
        c: usize,
        value: f64,
    },
    /// Twice the distance from point `p` to line `l` (a diameter about a centerline).
    LinearDiameter {
        p: usize,
        l: usize,
        value: f64,
    },
    /// Normal distance of an associative offset. Its output id remains stable
    /// when a curved boundary retires a corner; refresh_offsets drives the geometry.
    OffsetDistance {
        output: String,
        value: f64,
        /// Keep the chosen side when the positive driving length is edited.
        #[serde(default)]
        reverse: bool,
    },
}

impl ConstraintKind {
    pub fn is_dimension(&self) -> bool {
        self.value().is_some()
    }
    pub fn value(&self) -> Option<f64> {
        use ConstraintKind::*;
        match *self {
            Distance { value, .. }
            | DistanceX { value, .. }
            | DistanceY { value, .. }
            | PointLineDistance { value, .. }
            | LineCircleDistance { value, .. }
            | Length { value, .. }
            | LineAngle { value, .. }
            | ArcSweep { value, .. }
            | SlotOverallLength { value, .. }
            | Radius { value, .. }
            | Diameter { value, .. }
            | ArcLength { value, .. }
            | LinearDiameter { value, .. }
            | OffsetDistance { value, .. }
            | Angle { value, .. } => Some(value),
            _ => None,
        }
    }
    pub fn set_value(&mut self, v: f64) {
        use ConstraintKind::*;
        match self {
            Distance { value, .. }
            | DistanceX { value, .. }
            | DistanceY { value, .. }
            | PointLineDistance { value, .. }
            | LineCircleDistance { value, .. }
            | Length { value, .. }
            | LineAngle { value, .. }
            | ArcSweep { value, .. }
            | SlotOverallLength { value, .. }
            | Radius { value, .. }
            | Diameter { value, .. }
            | ArcLength { value, .. }
            | LinearDiameter { value, .. }
            | OffsetDistance { value, .. }
            | Angle { value, .. } => *value = v,
            _ => {}
        }
    }
    pub fn is_angle(&self) -> bool {
        matches!(self, ConstraintKind::Angle { .. } | ConstraintKind::LineAngle { .. } | ConstraintKind::ArcSweep { .. })
    }
    /// Short type name (Fusion-like) for listings.
    pub fn name(&self) -> &'static str {
        use ConstraintKind::*;
        match self {
            PatternPoint { .. } | PatternRadius { .. } => "Pattern",
            Coincident { .. } | PointOnCurve { .. } => "Coincident",
            Horizontal { .. } | HorizontalPoints { .. } => "Horizontal",
            Vertical { .. } | VerticalPoints { .. } => "Vertical",
            Parallel { .. } => "Parallel",
            Perpendicular { .. } => "Perpendicular",
            Collinear { .. } => "Collinear",
            Tangent { .. } => "Tangent",
            Equal { .. } => "Equal",
            Concentric { .. } => "Concentric",
            Midpoint { .. } => "MidPoint",
            Symmetric { .. } => "Symmetry",
            Fix { .. } => "Fix",
            Distance { .. } | DistanceX { .. } | DistanceY { .. } | PointLineDistance { .. } | LineCircleDistance { .. } | Length { .. } => {
                "LinearDimension"
            }
            Radius { .. } => "RadialDimension",
            Diameter { .. } => "DiameterDimension",
            Angle { .. } | LineAngle { .. } | ArcSweep { .. } => "AngularDimension",
            SlotOverallLength { .. } => "LinearDimension",
            Smooth { .. } => "Smooth",
            ArcLength { .. } => "ArcLengthDimension",
            LinearDiameter { .. } => "LinearDiameterDimension",
            OffsetDistance { .. } => "OffsetDimension",
        }
    }
    /// Does the constraint refer to point `p` (by index)?
    pub fn refers_to_point(&self, p: usize) -> bool {
        self.indices().0.contains(&p)
    }
    /// The points and the curves the constraint refers to.
    pub fn indices(&self) -> (Vec<usize>, Vec<usize>) {
        use ConstraintKind::*;
        match *self {
            PatternPoint { p, q, center, directions, .. } => {
                (vec![p, q].into_iter().chain(center).collect(), directions.into_iter().flatten().map(|(l, _)| l).collect())
            }
            PatternRadius { a, b } => (vec![], vec![a, b]),
            Coincident { p, q } | HorizontalPoints { p, q } | VerticalPoints { p, q } => (vec![p, q], vec![]),
            Distance { p, q, .. } | DistanceX { p, q, .. } | DistanceY { p, q, .. } => (vec![p, q], vec![]),
            PointOnCurve { p, c } => (vec![p], vec![c]),
            Horizontal { l } | Vertical { l } | Length { l, .. } | LineAngle { l, .. } => (vec![], vec![l]),
            Radius { c, .. } | Diameter { c, .. } | ArcSweep { c, .. } => (vec![], vec![c]),
            Parallel { a, b } | Perpendicular { a, b } | Collinear { a, b } | Tangent { a, b } | Equal { a, b } | Concentric { a, b } => {
                (vec![], vec![a, b])
            }
            Angle { a, b, .. } | SlotOverallLength { a, b, .. } => (vec![], vec![a, b]),
            Midpoint { p, l } | PointLineDistance { p, l, .. } => (vec![p], vec![l]),
            LineCircleDistance { l, c, .. } => (vec![], vec![l, c]),
            Symmetric { p, q, l } => (vec![p, q], vec![l]),
            Fix { p } => (vec![p], vec![]),
            Smooth { a, b } => (vec![], vec![a, b]),
            ArcLength { c, .. } => (vec![], vec![c]),
            LinearDiameter { p, l, .. } => (vec![p], vec![l]),
            OffsetDistance { .. } => (vec![], vec![]),
        }
    }
    /// Renumber the points and curves the constraint refers to; false when one has no new
    /// number (the constraint is then partly renumbered and should be dropped).
    pub fn remap(&mut self, pmap: &dyn Fn(usize) -> Option<usize>, cmap: &dyn Fn(usize) -> Option<usize>) -> bool {
        use ConstraintKind::*;
        let mut ok = true;
        let mut fp = |i: &mut usize| match pmap(*i) {
            Some(n) => *i = n,
            None => ok = false,
        };
        let mut ok2 = true;
        let mut fc = |i: &mut usize| match cmap(*i) {
            Some(n) => *i = n,
            None => ok2 = false,
        };
        match self {
            PatternPoint { p, q, center, directions, .. } => {
                fp(p);
                fp(q);
                if let Some(c) = center {
                    fp(c);
                }
                for (l, _) in directions.iter_mut().flatten() {
                    fc(l);
                }
            }
            PatternRadius { a, b } => {
                fc(a);
                fc(b);
            }
            Coincident { p, q } | HorizontalPoints { p, q } | VerticalPoints { p, q } => {
                fp(p);
                fp(q);
            }
            Distance { p, q, .. } | DistanceX { p, q, .. } | DistanceY { p, q, .. } => {
                fp(p);
                fp(q);
            }
            PointOnCurve { p, c } => {
                fp(p);
                fc(c);
            }
            Horizontal { l } | Vertical { l } | Length { l, .. } | LineAngle { l, .. } => fc(l),
            Radius { c, .. } | Diameter { c, .. } | ArcSweep { c, .. } => fc(c),
            Parallel { a, b } | Perpendicular { a, b } | Collinear { a, b } | Tangent { a, b } | Equal { a, b } | Concentric { a, b } => {
                fc(a);
                fc(b);
            }
            Angle { a, b, .. } | SlotOverallLength { a, b, .. } => {
                fc(a);
                fc(b);
            }
            Midpoint { p, l } | PointLineDistance { p, l, .. } => {
                fp(p);
                fc(l);
            }
            LineCircleDistance { l, c, .. } => {
                fc(l);
                fc(c);
            }
            Symmetric { p, q, l } => {
                fp(p);
                fp(q);
                fc(l);
            }
            Fix { p } => fp(p),
            Smooth { a, b } => {
                fc(a);
                fc(b);
            }
            ArcLength { c, .. } => fc(c),
            LinearDiameter { p, l, .. } => {
                fp(p);
                fc(l);
            }
            OffsetDistance { .. } => {}
        }
        ok && ok2
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub id: String,
    #[serde(flatten)]
    pub kind: ConstraintKind,
    /// For dimensions: the model parameter that drives the value (e.g. `d1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// A driven (reference) dimension: it measures, it does not constrain.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub driven: bool,
    /// For dimensions: where the value text was placed, relative to the dimension's frame
    /// (see [`crate::dim_frame`]); None for the default place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Vec2>,
}

/// How a sketch is shown and drawn in (the Sketch Palette options kept with the sketch). Every
/// flag is off by default, so a sketch shows everything, with grid and snap.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SketchView {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hide_profile: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hide_points: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hide_dimensions: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hide_constraints: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hide_projected: bool,
    /// Construction geometry (the `construction` line type) is not drawn and cannot be picked.
    /// Centerlines are a line type of their own and keep showing; projected construction
    /// geometry belongs to `hide_projected`, the way its colour already says it does.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hide_construction: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hide_grid: bool,
    /// Clicks don't snap to the grid.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_snap: bool,
    /// Cut away the model in front of the sketch plane while editing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub slice: bool,
    /// 3D sketch: drawing may leave the sketch plane.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub three_d: bool,
}

/// A 2D sketch. Point 0 is always the fixed sketch origin (id `origin`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sketch {
    pub points: Vec<SPoint>,
    pub curves: Vec<Curve>,
    pub constraints: Vec<Constraint>,
    /// Editable sketch patterns; their relations are ordinary solver constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<crate::pattern::SketchPattern>,
    /// Links to geometry outside the sketch (projections, intersections, includes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<crate::link::Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offsets: Vec<crate::offset::OffsetChain>,
    /// New certified normal offsets; legacy sampled output splines remain independent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub freeform_offsets: Vec<crate::freeform_offset::FreeformOffset>,
    /// 3D curves (world coordinates) carried by the sketch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wires: Vec<crate::wire::Wire>,
    /// Next number per id prefix (`l` → l1, l2…).
    #[serde(default)]
    counters: std::collections::BTreeMap<String, u64>,
    /// Display options (Sketch Palette).
    #[serde(default, skip_serializing_if = "is_default_view")]
    pub view: SketchView,
}

fn is_default_view(v: &SketchView) -> bool {
    *v == SketchView::default()
}

impl Default for Sketch {
    fn default() -> Self {
        Sketch::new()
    }
}

impl Sketch {
    pub fn new() -> Self {
        Sketch {
            points: vec![SPoint { id: "origin".into(), pos: Vec2::ZERO, fixed: true, link: None }],
            curves: Vec::new(),
            constraints: Vec::new(),
            patterns: Vec::new(),
            links: Vec::new(),
            offsets: Vec::new(),
            freeform_offsets: Vec::new(),
            wires: Vec::new(),
            counters: Default::default(),
            view: SketchView::default(),
        }
    }

    /// A new unused id with the given prefix (`l` → `l3`).
    pub fn fresh(&mut self, prefix: &str) -> Result<String> {
        let mut n = self.counters.get(prefix).copied().unwrap_or(0);
        n = n.checked_add(1).ok_or_else(|| SketchError::Invalid("sketch identity counter exhausted".into()))?;
        let first = format!("{prefix}{n}");
        if !self.id_taken(&first) {
            self.counters.insert(prefix.to_string(), n);
            return Ok(first);
        }
        let budget = self
            .points
            .len()
            .saturating_add(self.curves.len())
            .saturating_add(self.constraints.len())
            .saturating_add(self.patterns.len())
            .saturating_add(self.links.len())
            .saturating_add(self.wires.len())
            .saturating_add(
                self.offsets.iter().filter_map(|r| r.topology.as_ref()).fold(0usize, |n, r| n.saturating_add(r.roles.len()).saturating_add(1)),
            )
            .saturating_add(1);
        // A damaged counter may start before a long occupied prefix. Index
        // reservations once so probing that prefix cannot rescan all geometry
        // for each candidate ID.
        let mut taken: std::collections::BTreeSet<&str> = self
            .points
            .iter()
            .map(|p| p.id.as_str())
            .chain(self.curves.iter().map(|c| c.id.as_str()))
            .chain(self.constraints.iter().map(|c| c.id.as_str()))
            .chain(self.patterns.iter().map(|p| p.id.as_str()))
            .chain(self.links.iter().map(|l| l.id.as_str()))
            .chain(self.wires.iter().map(|w| w.id.as_str()))
            .collect();
        for topology in self.offsets.iter().filter_map(|r| r.topology.as_ref()) {
            taken.insert(topology.id.as_str());
            taken.extend(topology.roles.iter().map(|r| r.output.as_str()));
        }
        for _ in 0..budget.saturating_sub(1) {
            n = n.checked_add(1).ok_or_else(|| SketchError::Invalid("sketch identity counter exhausted".into()))?;
            let id = format!("{prefix}{n}");
            if !taken.contains(id.as_str()) {
                self.counters.insert(prefix.to_string(), n);
                return Ok(id);
            }
        }
        Err(SketchError::TooLarge)
    }
    pub(crate) fn id_taken(&self, id: &str) -> bool {
        self.points.iter().any(|p| p.id == id)
            || self.curves.iter().any(|c| c.id == id)
            || self.constraints.iter().any(|c| c.id == id)
            || self.patterns.iter().any(|p| p.id == id)
            || self.links.iter().any(|l| l.id == id)
            || self.wires.iter().any(|w| w.id == id)
            || self.offsets.iter().filter_map(|r| r.topology.as_ref()).any(|r| r.id == id || r.roles.iter().any(|role| role.output == id))
    }

    pub fn point_index(&self, id: &str) -> Option<usize> {
        self.points.iter().position(|p| p.id == id)
    }
    pub fn curve_index(&self, id: &str) -> Option<usize> {
        self.curves.iter().position(|c| c.id == id)
    }
    pub fn point(&self, i: usize) -> Option<Vec2> {
        self.points.get(i).map(|p| p.pos)
    }

    fn check_pos(p: Vec2) -> Result<Vec2> {
        if in_range(p) { Ok(p) } else { Err(SketchError::Invalid(format!("point {:?}", [p.x, p.y]))) }
    }

    /// Add a free point (or reuse an existing one within `merge_tol` of `p` when `merge_tol > 0`).
    pub fn add_point(&mut self, p: Vec2, id: Option<&str>) -> Result<usize> {
        let p = Self::check_pos(p)?;
        if self.points.len() >= MAX_POINTS {
            return Err(SketchError::TooLarge);
        }
        let id = match id {
            Some(i) if self.id_taken(i) => return Err(SketchError::Duplicate(i.into())),
            Some("") => return Err(SketchError::Invalid("empty id".into())),
            Some(i) => i.to_string(),
            None => self.fresh("p")?,
        };
        self.points.push(SPoint { id, pos: p, fixed: false, link: None });
        Ok(self.points.len() - 1)
    }

    pub(crate) fn curve_id(&mut self, id: Option<&str>, prefix: &str) -> Result<String> {
        match id {
            Some(i) if self.id_taken(i) => Err(SketchError::Duplicate(i.into())),
            Some("") => Err(SketchError::Invalid("empty id".into())),
            Some(i) => Ok(i.into()),
            None => self.fresh(prefix),
        }
    }

    /// Endpoint ids of a curve's own points: `<curve>.start`, `.end`, `.center`.
    fn own_point(&mut self, curve: &str, role: &str, p: Vec2) -> Result<usize> {
        let id = format!("{curve}.{role}");
        if self.id_taken(&id) {
            return self.add_point(p, None);
        }
        self.add_point(p, Some(&id))
    }

    /// Line between existing points `a` and `b`.
    pub fn add_line_pts(&mut self, a: usize, b: usize, id: Option<&str>) -> Result<usize> {
        if a >= self.points.len() || b >= self.points.len() || a == b {
            return Err(SketchError::Invalid("line needs two distinct points".into()));
        }
        let id = self.curve_id(id, "l")?;
        self.curves.push(Curve {
            id,
            kind: CurveKind::Line { a, b },
            construction: false,
            reversed: false,
            link: None,
            centerline: false,
            fixed: false,
            infinite: false,
        });
        Ok(self.curves.len() - 1)
    }

    /// Line from `p0` to `p1`, reusing given point indices when provided.
    pub fn add_line(&mut self, p0: Vec2, p1: Vec2, a: Option<usize>, b: Option<usize>, id: Option<&str>) -> Result<usize> {
        Self::check_pos(p0)?;
        Self::check_pos(p1)?;
        let id = self.curve_id(id, "l")?;
        let a = match a {
            Some(i) if i < self.points.len() => i,
            _ => self.own_point(&id, "start", p0)?,
        };
        let b = match b {
            Some(i) if i < self.points.len() && i != a => i,
            _ => self.own_point(&id, "end", p1)?,
        };
        self.add_line_pts(a, b, Some(&id))
    }

    pub fn add_circle(&mut self, center: Vec2, r: f64, c: Option<usize>, id: Option<&str>) -> Result<usize> {
        if !radius_in_range(r) {
            return Err(SketchError::Invalid(format!("radius {r}")));
        }
        let id = self.curve_id(id, "c")?;
        let c = match c {
            Some(i) if i < self.points.len() => i,
            _ => self.own_point(&id, "center", center)?,
        };
        self.curves.push(Curve {
            id,
            kind: CurveKind::Circle { c, r },
            construction: false,
            reversed: false,
            link: None,
            centerline: false,
            fixed: false,
            infinite: false,
        });
        Ok(self.curves.len() - 1)
    }

    /// Counter-clockwise arc around `center` from `p0` to `p1` (the end point is projected onto
    /// the circle through `p0`).
    pub fn add_arc(&mut self, center: Vec2, p0: Vec2, p1: Vec2, pts: [Option<usize>; 3], id: Option<&str>) -> Result<usize> {
        let r = center.dist(p0);
        if !radius_in_range(r) {
            return Err(SketchError::Invalid("degenerate arc".into()));
        }
        let p1 = center + (p1 - center).normalized().ok_or_else(|| SketchError::Invalid("degenerate arc".into()))? * r;
        let id = self.curve_id(id, "a")?;
        let n = self.points.len();
        let c = match pts[0] {
            Some(i) if i < n => i,
            _ => self.own_point(&id, "center", center)?,
        };
        let a = match pts[1] {
            Some(i) if i < n => i,
            _ => self.own_point(&id, "start", p0)?,
        };
        let b = match pts[2] {
            Some(i) if i < n && i != a => i,
            _ => self.own_point(&id, "end", p1)?,
        };
        self.curves.push(Curve {
            id,
            kind: CurveKind::Arc { c, a, b },
            construction: false,
            reversed: false,
            link: None,
            centerline: false,
            fixed: false,
            infinite: false,
        });
        Ok(self.curves.len() - 1)
    }

    pub fn add_constraint(&mut self, kind: ConstraintKind, param: Option<String>) -> Result<String> {
        if self.constraints.len() >= MAX_CONSTRAINTS {
            return Err(SketchError::TooLarge);
        }
        self.validate(&kind)?;
        let id = self.fresh("k")?;
        self.constraints.push(Constraint { id: id.clone(), kind, param, driven: false, text: None });
        Ok(id)
    }

    /// Check indices and entity kinds of a constraint.
    pub fn validate(&self, k: &ConstraintKind) -> Result<()> {
        use ConstraintKind::*;
        let (ps, cs) = k.indices();
        for p in &ps {
            if *p >= self.points.len() {
                return Err(SketchError::Unknown(format!("point #{p}")));
            }
        }
        for c in &cs {
            if *c >= self.curves.len() {
                return Err(SketchError::Unknown(format!("curve #{c}")));
            }
        }
        let is_line = |i: usize| matches!(self.curves.get(i).map(|c| &c.kind), Some(CurveKind::Line { .. }));
        let is_round = |i: usize| matches!(self.curves.get(i).map(|c| &c.kind), Some(CurveKind::Circle { .. } | CurveKind::Arc { .. }));
        let name = |i: usize| self.curves.get(i).map(|c| c.id.clone()).unwrap_or_default();
        let need_line = |i: usize| if is_line(i) { Ok(()) } else { Err(SketchError::WrongKind(name(i), "needs a line".into())) };
        let need_round = |i: usize| if is_round(i) { Ok(()) } else { Err(SketchError::WrongKind(name(i), "needs a circle or arc".into())) };
        match *k {
            PatternPoint { p, q, angle, offset, directions, .. } => {
                for (l, distance) in directions.into_iter().flatten() {
                    need_line(l)?;
                    if !distance.x.is_finite() || !distance.y.is_finite() || distance.x.abs() > MAX_COORD || distance.y.abs() > MAX_COORD {
                        return Err(SketchError::Invalid("invalid pattern direction displacement".into()));
                    }
                }
                if p == q
                    || !angle.is_finite()
                    || angle.abs() > std::f64::consts::TAU
                    || !offset.x.is_finite()
                    || !offset.y.is_finite()
                    || offset.x.abs() > MAX_COORD
                    || offset.y.abs() > MAX_COORD
                {
                    return Err(SketchError::Invalid("invalid pattern transform".into()));
                }
            }
            PatternRadius { a, b } => {
                let independent_radius =
                    |c: usize| matches!(self.curves.get(c).map(|c| &c.kind), Some(CurveKind::Circle { .. } | CurveKind::Ellipse { .. }));
                if a == b || !independent_radius(a) || !independent_radius(b) {
                    return Err(SketchError::Invalid("pattern radius needs two circles or ellipses".into()));
                }
            }
            Horizontal { l }
            | Vertical { l }
            | Length { l, .. }
            | LineAngle { l, .. }
            | Midpoint { l, .. }
            | Symmetric { l, .. }
            | PointLineDistance { l, .. }
            | LinearDiameter { l, .. } => need_line(l)?,
            ArcLength { c, .. } | ArcSweep { c, .. } if !matches!(self.curves.get(c).map(|c| &c.kind), Some(CurveKind::Arc { .. })) => {
                return Err(SketchError::WrongKind(name(c), "arc length needs an arc".into()));
            }
            Smooth { a, b } => {
                let ends = |i: usize| self.curves.get(i).and_then(|c| c.kind.ends());
                let shared = match (ends(a), ends(b)) {
                    (Some((a0, a1)), Some((b0, b1))) => a0 == b0 || a0 == b1 || a1 == b0 || a1 == b1,
                    _ => false,
                };
                if !shared || a == b {
                    return Err(SketchError::Invalid("curvature continuity needs two curves sharing an end point".into()));
                }
            }
            Parallel { a, b } | Perpendicular { a, b } | Collinear { a, b } | Angle { a, b, .. } => {
                need_line(a)?;
                need_line(b)?;
            }
            // On an ellipse, a radius dimension holds its minor radius.
            Radius { c, .. } if matches!(self.curves.get(c).map(|x| &x.kind), Some(CurveKind::Ellipse { .. })) => {}
            Radius { c, .. } | Diameter { c, .. } => need_round(c)?,
            Concentric { a, b } => {
                need_round(a)?;
                need_round(b)?;
            }
            SlotOverallLength { a, b, value } => {
                if a == b || ![a, b].iter().all(|i| matches!(self.curves.get(*i).map(|c| &c.kind), Some(CurveKind::Arc { .. }))) {
                    return Err(SketchError::Invalid("overall slot length needs two different arc caps".into()));
                }
                let radius = |cap: usize| {
                    self.constraints
                        .iter()
                        .filter(|c| !c.driven)
                        .find_map(|dimension| match dimension.kind {
                            Radius { c, value } if c == cap => Some(value),
                            Diameter { c, value } if c == cap => Some(value / 2.0),
                            // Slot caps are constrained equal; a diameter on either drives both.
                            Diameter { c, value } | Radius { c, value }
                                if (c == a || c == b)
                                    && self
                                        .constraints
                                        .iter()
                                        .any(|e| matches!(e.kind, Equal { a: x, b: y } if (x == a && y == b) || (x == b && y == a))) =>
                            {
                                Some(if matches!(dimension.kind, Diameter { .. }) { value / 2.0 } else { value })
                            }
                            _ => None,
                        })
                        .or_else(|| self.radius(cap))
                        .unwrap_or(0.0)
                };
                if value <= radius(a) + radius(b) + 1e-9 {
                    return Err(SketchError::Invalid("overall slot length must exceed its width".into()));
                }
            }
            LineCircleDistance { l, c, .. } => {
                need_line(l)?;
                need_round(c)?;
            }
            Tangent { a, b } => {
                let free = |i: usize| self.curves.get(i).is_some_and(|c| c.kind.is_freeform());
                if !(is_round(a) || is_round(b) || free(a) || free(b)) {
                    return Err(SketchError::WrongKind(name(a), "tangent needs a circle or arc".into()));
                }
            }
            Equal { a, b } if is_line(a) != is_line(b) => {
                return Err(SketchError::WrongKind(name(b), "equal needs two lines or two circles/arcs".into()));
            }
            Coincident { p, q } if p == q => return Err(SketchError::Invalid("a point is always coincident with itself".into())),
            _ => {}
        }
        if let Some(v) = k.value()
            && (!v.is_finite() || v.abs() > MAX_COORD || (v < 0.0 && !k.is_angle()))
        {
            return Err(SketchError::Invalid(format!("dimension value {v}")));
        }
        if let OffsetDistance { output, value, .. } = k
            && (*value <= 1e-9 || output.len() > 512 || self.offset_distance(output).is_none())
        {
            return Err(SketchError::Invalid("offset dimension needs a nonzero distance and an existing association".into()));
        }
        if let ArcSweep { value, .. } = *k
            && !(value.abs() > 1e-9 && value.abs() < std::f64::consts::TAU - 1e-9)
        {
            return Err(SketchError::Invalid("arc sweep must be nonzero and less than a full turn".into()));
        }
        Ok(())
    }

    /// The sketch scaled by `factor` about its origin, for its first dimension (#1365): drawn
    /// geometry has no size of its own until a dimension gives it one, so the first length
    /// dimension scales the whole sketch to its value instead of distorting it. Only a sketch
    /// with nothing that fixes a size: no other driving length dimension, nothing fixed,
    /// projected or patterned, no offsets and no 3D curves. Every geometric constraint
    /// (coincident, horizontal, parallel, tangent, equal, symmetric, …) and every angle holds
    /// on the scaled sketch as it held before. `None` when the sketch does not qualify.
    pub fn scaled_for_first_dimension(&self, factor: f64) -> Option<Sketch> {
        if !(factor.is_finite() && (1e-6..=1e6).contains(&factor)) {
            return None;
        }
        let sizes = self.constraints.iter().any(|c| match c.kind {
            ConstraintKind::Fix { .. } | ConstraintKind::PatternPoint { .. } | ConstraintKind::PatternRadius { .. } => true,
            ref k => k.is_dimension() && !k.is_angle() && !c.driven,
        });
        if sizes
            || !self.links.is_empty()
            || !self.patterns.is_empty()
            || !self.offsets.is_empty()
            || !self.freeform_offsets.is_empty()
            || !self.wires.is_empty()
            || self.curves.iter().any(|c| c.fixed || c.link.is_some())
            || self.points.iter().enumerate().any(|(i, p)| (p.fixed || p.link.is_some()) && (i != 0 || p.pos != Vec2::ZERO))
        {
            return None;
        }
        let mut out = self.clone();
        for p in out.points.iter_mut().filter(|p| !p.fixed) {
            p.pos = p.pos * factor;
            if !in_range(p.pos) {
                return None;
            }
        }
        for c in &mut out.curves {
            if let CurveKind::Circle { r, .. } | CurveKind::Ellipse { r, .. } = &mut c.kind {
                *r *= factor;
                if !radius_in_range(*r) {
                    return None;
                }
            }
        }
        // Dimension texts keep their places relative to what they measure.
        for t in out.constraints.iter_mut().filter_map(|c| c.text.as_mut()) {
            *t = *t * factor;
        }
        Some(out)
    }

    /// Is point `i` held in place: fixed, projected (linked), fixed by a Fix constraint or a
    /// point of a fixed curve.
    pub fn point_locked(&self, i: usize) -> bool {
        self.points.get(i).is_some_and(|p| p.fixed || p.link.is_some())
            || self.constraints.iter().any(|c| c.kind == ConstraintKind::Fix { p: i })
            || self.curves.iter().any(|c| c.fixed && c.kind.uses(i))
    }

    /// [`Sketch::point_locked`] for every point at once: the points every solve keeps as they are.
    pub fn locked_points(&self) -> Vec<bool> {
        let mut out: Vec<bool> = self.points.iter().map(|p| p.fixed || p.link.is_some()).collect();
        let mut set = |i: usize| {
            if let Some(x) = out.get_mut(i) {
                *x = true;
            }
        };
        for c in &self.constraints {
            if let ConstraintKind::Fix { p } = c.kind {
                set(p);
            }
        }
        for c in self.curves.iter().filter(|c| c.fixed) {
            for p in c.kind.point_ids() {
                set(p);
            }
        }
        out
    }

    /// The curves joined end to end with curve `ci` (through shared or coincident end points),
    /// `ci` first, then the others in sketch order: what a double-click selects.
    pub fn chain(&self, ci: usize) -> Vec<usize> {
        if ci >= self.curves.len() {
            return Vec::new();
        }
        // Points made one by coincident constraints.
        let mut root: Vec<usize> = (0..self.points.len()).collect();
        for c in &self.constraints {
            if let ConstraintKind::Coincident { p, q } = c.kind {
                union(&mut root, p, q);
            }
        }
        let ends: Vec<Vec<usize>> =
            self.curves.iter().map(|c| c.kind.ends().map(|(a, b)| vec![find(&mut root, a), find(&mut root, b)]).unwrap_or_default()).collect();
        let mut seen = vec![false; self.curves.len()];
        let mut stack = vec![ci];
        let mut out = Vec::new();
        while let Some(i) = stack.pop() {
            if seen.get(i).copied().unwrap_or(true) {
                continue;
            }
            if let Some(s) = seen.get_mut(i) {
                *s = true;
            }
            out.push(i);
            let mine = ends.get(i).cloned().unwrap_or_default();
            for (j, e) in ends.iter().enumerate() {
                if !seen.get(j).copied().unwrap_or(true) && e.iter().any(|p| mine.contains(p)) {
                    stack.push(j);
                }
            }
        }
        let first = out.first().copied();
        out.sort_unstable();
        if let Some(f) = first {
            out.retain(|x| *x != f);
            out.insert(0, f);
        }
        out
    }

    /// Virtual sharp corners (what a fillet or chamfer leaves where dimensions measured to the
    /// corner): points used by no curve, held on two or more lines or arcs (their extensions).
    /// Each with the extension segments from the corner to the nearest end of those curves, for
    /// drawing.
    pub fn virtual_corners(&self) -> Vec<(usize, Vec<(Vec2, Vec2)>)> {
        const MAX_POINT_REFERENCES: usize = 1_000_000;
        if self.points.len() > MAX_POINTS || self.constraints.len() > MAX_CONSTRAINTS || self.curves.len() > MAX_POINT_REFERENCES {
            return Vec::new();
        }
        let mut remaining = MAX_POINT_REFERENCES;
        for curve in &self.curves {
            let count = match &curve.kind {
                CurveKind::Line { .. } | CurveKind::Ellipse { .. } => 2,
                CurveKind::Circle { .. } => 1,
                CurveKind::Arc { .. } | CurveKind::Conic { .. } => 3,
                CurveKind::Spline { pts, .. } => pts.len(),
            };
            let Some(left) = remaining.checked_sub(count) else { return Vec::new() };
            remaining = left;
        }
        // Index curve usage once, including centres and spline interior points.
        let mut used = vec![false; self.points.len()];
        for curve in &self.curves {
            for point in curve.kind.point_ids() {
                if let Some(slot) = used.get_mut(point) {
                    *slot = true;
                }
            }
        }
        let mut incidence = vec![Vec::new(); self.points.len()];
        let mut seen = std::collections::HashSet::new();
        for constraint in &self.constraints {
            let ConstraintKind::PointOnCurve { p, c } = constraint.kind else { continue };
            if p == 0 || used.get(p).copied() != Some(false) || !self.point(p).is_some_and(|p| p.is_finite()) {
                continue;
            }
            let Some(curve) = self.curves.get(c) else { continue };
            if !matches!(curve.kind, CurveKind::Line { .. } | CurveKind::Arc { .. })
                || curve.kind.point_ids().iter().any(|i| !self.point(*i).is_some_and(|p| p.is_finite()))
            {
                continue;
            }
            if seen.insert((p, c))
                && let Some(on) = incidence.get_mut(p)
            {
                on.push(c);
            }
        }
        let mut out = Vec::new();
        for (i, on) in incidence.into_iter().enumerate().skip(1) {
            if on.len() < 2 {
                continue;
            }
            let Some(p) = self.point(i) else { continue };
            let mut ext = Vec::new();
            for c in on {
                let Some((a, b)) = self.curves.get(c).and_then(|c| c.kind.ends()) else { continue };
                let (Some(pa), Some(pb)) = (self.point(a), self.point(b)) else { continue };
                let near = if p.dist(pa) <= p.dist(pb) { pa } else { pb };
                // A line only needs one when the corner is beyond its ends.
                let inside = matches!(self.curves.get(c).map(|c| &c.kind), Some(CurveKind::Line { .. })) && {
                    let d = pb - pa;
                    let t = (p - pa).dot(d) / d.dot(d).max(1e-300);
                    (0.0..=1.0).contains(&t)
                };
                if !inside && p.dist(near) > 1e-9 {
                    ext.push((p, near));
                }
            }
            out.push((i, ext));
        }
        out
    }

    /// Every point a constraint touches: its points and the points of its curves.
    pub fn constraint_points(&self, k: &ConstraintKind) -> Vec<usize> {
        let (mut pts, curves) = k.indices();
        pts.extend(curves.into_iter().flat_map(|c| self.curves.get(c).map(|x| x.kind.point_ids()).unwrap_or_default()));
        pts
    }

    /// What a solve holding `points` and the radii of `radii` moves with them, as the solver
    /// couples its variables: (per point, per curve radius). The held points, and the free
    /// points and free circle/ellipse radii joined to them through curves and driving
    /// constraints without passing through a constant (a held, fixed or projected point or
    /// radius couples nothing).
    pub fn coupled(&self, points: &[usize], radii: &[usize]) -> (Vec<bool>, Vec<bool>) {
        let (n, m) = (self.points.len(), self.curves.len());
        // Nodes: points, then curve radii, then one node standing for everything held.
        let held = n + m;
        let mut root: Vec<usize> = (0..=held).collect();
        let mut seed = vec![false; n + m];
        for v in points.iter().copied().chain(radii.iter().map(|c| n.saturating_add(*c))) {
            if let Some(s) = seed.get_mut(v) {
                *s = true;
            }
        }
        let locked = self.locked_points();
        let free = |v: usize| {
            if seed.get(v).copied().unwrap_or(true) {
                return false;
            }
            match v.checked_sub(n) {
                None => !locked.get(v).copied().unwrap_or(true),
                Some(c) => self
                    .curves
                    .get(c)
                    .is_some_and(|x| matches!(x.kind, CurveKind::Circle { .. } | CurveKind::Ellipse { .. }) && x.link.is_none() && !x.fixed),
            }
        };
        let curve_nodes = |c: usize| {
            let mut v = self.curves.get(c).map(|x| x.kind.point_ids()).unwrap_or_default();
            v.push(n + c);
            v
        };
        // What the solver's equations join (see `solve_with`): an arc's centre and ends, a
        // periodic spline's first and last points, and each driving constraint's entities. A
        // curve alone joins nothing (a circle's radius and centre move independently).
        let own = self.curves.iter().filter_map(|x| match &x.kind {
            CurveKind::Arc { c, a, b } => Some(vec![*c, *a, *b]),
            CurveKind::Spline { pts, periodic: true, .. } => match (pts.first(), pts.last()) {
                (Some(a), Some(b)) if a != b => Some(vec![*a, *b]),
                _ => None,
            },
            _ => None,
        });
        let edges = own.chain(self.constraints.iter().filter(|c| !c.driven).map(|c| {
            let (mut v, curves) = c.kind.indices();
            v.extend(curves.into_iter().flat_map(curve_nodes));
            v
        }));
        for e in edges {
            let mut join = e.iter().copied().filter(|&v| free(v));
            let first = if e.iter().any(|v| seed.get(*v).copied().unwrap_or(false)) { Some(held) } else { join.next() };
            if let Some(first) = first {
                for v in join {
                    union(&mut root, first, v);
                }
            }
        }
        let r = find(&mut root, held);
        let mut coupled = |v: usize| seed.get(v).copied().unwrap_or(false) || (free(v) && find(&mut root, v) == r);
        ((0..n).map(&mut coupled).collect(), (n..n + m).map(&mut coupled).collect())
    }

    /// Is curve `c` held in place: fixed or projected (linked).
    pub fn curve_locked(&self, c: usize) -> bool {
        self.curves.get(c).is_some_and(|c| c.fixed || c.link.is_some() || self.freeform_offsets.iter().any(|offset| offset.output == c.id))
    }

    /// Radius of a circle or arc.
    pub fn radius(&self, ci: usize) -> Option<f64> {
        match self.curves.get(ci)?.kind {
            CurveKind::Circle { r, .. } => Some(r),
            CurveKind::Arc { c, a, .. } => Some(self.point(c)?.dist(self.point(a)?)),
            _ => None,
        }
    }
    pub fn center(&self, ci: usize) -> Option<Vec2> {
        match self.curves.get(ci)?.kind {
            CurveKind::Circle { c, .. } | CurveKind::Arc { c, .. } | CurveKind::Ellipse { c, .. } => self.point(c),
            _ => None,
        }
    }

    /// Boundary segments of a curve (a circle gives two half arcs).
    pub fn segs(&self, ci: usize) -> Vec<Seg2> {
        let Some(cu) = self.curves.get(ci) else { return Vec::new() };
        if cu.kind.is_freeform() {
            let poly = self.polyline(ci);
            return poly.windows(2).filter(|w| w[0].dist(w[1]) > 1e-12).map(|w| Seg2::Line { a: w[0], b: w[1] }).collect();
        }
        match cu.kind {
            CurveKind::Line { a, b } => match (self.point(a), self.point(b)) {
                (Some(a), Some(b)) => vec![Seg2::Line { a, b }],
                _ => Vec::new(),
            },
            CurveKind::Circle { c, r } => match self.point(c) {
                Some(c) => solidcraft_geom::Loop2::circle(c, r).segs,
                None => Vec::new(),
            },
            CurveKind::Arc { c, a, b } => match (self.point(c), self.point(a), self.point(b)) {
                (Some(c), Some(pa), Some(pb)) => {
                    let start = (pa - c).angle();
                    let mut sweep = (pb - c).angle() - start;
                    while sweep <= 1e-12 {
                        sweep += std::f64::consts::TAU;
                    }
                    vec![Seg2::Arc { center: c, radius: c.dist(pa), start, sweep }]
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Exact boundary segments of a free-form curve: ellipses as four conics, conics, fit
    /// splines and control splines up to `MAX_BEZIER_DEGREE` as Béziers. None for other curves
    /// (and for higher-degree splines, which stay polylines).
    pub fn exact_segs(&self, ci: usize) -> Option<Vec<Seg2>> {
        let cu = self.curves.get(ci)?;
        let pt = |i: &usize| self.point(*i);
        match &cu.kind {
            CurveKind::Ellipse { c, m, r } => {
                let (c, m) = (pt(c)?, pt(m)?);
                let u = m - c;
                let v = u.perp().normalized()? * *r;
                let w = std::f64::consts::FRAC_1_SQRT_2;
                let corners = [c + u, c + v, c - u, c - v];
                Some(
                    (0..4)
                        .map(|i| {
                            let (a, b) = (corners[i], corners[(i + 1) % 4]);
                            Seg2::Conic { a, apex: a + b - c, b, w }
                        })
                        .collect(),
                )
            }
            CurveKind::Conic { a, b, apex, rho } => Some(vec![Seg2::Conic { a: pt(a)?, apex: pt(apex)?, b: pt(b)?, w: rho / (1.0 - rho).max(1e-9) }]),
            CurveKind::Spline { pts, control: false, periodic, .. } => {
                let p: Vec<Vec2> = pts.iter().map(pt).collect::<Option<_>>()?;
                Some(
                    crate::curves::fit_beziers_mode(&p, *periodic).ok()?.into_iter().map(|[p0, p1, p2, p3]| Seg2::Cubic { p0, p1, p2, p3 }).collect(),
                )
            }
            CurveKind::Spline { pts, control: true, degree, .. } if (*degree as usize) <= solidcraft_geom::MAX_BEZIER_DEGREE => {
                let p: Vec<Vec2> = pts.iter().map(pt).collect::<Option<_>>()?;
                let k = (*degree as usize).min(p.len().saturating_sub(1)).max(1);
                Some(crate::curves::bspline_beziers(&p, k).iter().filter_map(|c| solidcraft_geom::bezier(c)).collect())
            }
            _ => None,
        }
    }

    /// The exact [`Seg2`] pieces of a curve of any kind, in the curve's own direction of travel:
    /// lines, circles and arcs from their analytic form, free-form curves through
    /// [`Sketch::exact_segs`]. Empty for a curve with no exact pieces (a spline above
    /// `solidcraft_geom::MAX_BEZIER_DEGREE`).
    pub fn exact_pieces(&self, ci: usize) -> Vec<Seg2> {
        match self.exact_segs(ci) {
            Some(segs) => segs,
            None => self.segs(ci),
        }
    }

    /// Polyline of a free-form curve (ellipse, spline, conic); other curves: their segments'
    /// polylines.
    pub fn polyline(&self, ci: usize) -> Vec<Vec2> {
        let Some(cu) = self.curves.get(ci) else { return Vec::new() };
        let pt = |i: &usize| self.point(*i);
        match &cu.kind {
            CurveKind::Ellipse { c, m, r } => match (pt(c), pt(m)) {
                (Some(c), Some(m)) => crate::curves::ellipse_polyline(c, m, *r),
                _ => Vec::new(),
            },
            CurveKind::Spline { pts, control, degree, periodic } => {
                let p: Option<Vec<Vec2>> = pts.iter().map(pt).collect();
                p.map(|p| crate::curves::spline_polyline_mode(&p, *control, *degree, *periodic)).unwrap_or_default()
            }
            CurveKind::Conic { a, b, apex, rho } => match (pt(a), pt(apex), pt(b)) {
                (Some(a), Some(x), Some(b)) => crate::curves::conic_polyline(a, x, b, *rho),
                _ => Vec::new(),
            },
            _ => {
                let mut out: Vec<Vec2> = Vec::new();
                for s in self.segs(ci) {
                    for q in s.polyline(1e-3) {
                        if out.last().is_none_or(|l| l.dist(q) > 1e-12) {
                            out.push(q);
                        }
                    }
                }
                out
            }
        }
    }

    /// Add a curve of any kind over existing points.
    pub fn add_curve(&mut self, kind: CurveKind, id: Option<&str>) -> Result<usize> {
        let ids = kind.point_ids();
        if ids.iter().any(|p| *p >= self.points.len()) {
            return Err(SketchError::Invalid("curve point out of range".into()));
        }
        let prefix = match &kind {
            CurveKind::Line { .. } => "l",
            CurveKind::Circle { .. } => "c",
            CurveKind::Arc { .. } => "a",
            CurveKind::Ellipse { m, c, r } => {
                if m == c || !radius_in_range(*r) {
                    return Err(SketchError::Invalid("degenerate ellipse".into()));
                }
                "e"
            }
            CurveKind::Spline { pts, control, degree, periodic } => {
                if pts.len() < 2 || pts.len() > crate::curves::MAX_SPLINE_POINTS || !(1..=solidcraft_geom::MAX_BEZIER_DEGREE as u8).contains(degree) {
                    return Err(SketchError::Invalid(format!(
                        "a spline needs 2 to 500 points and degree 1 to {}",
                        solidcraft_geom::MAX_BEZIER_DEGREE
                    )));
                }
                if *periodic {
                    if *control || *degree != 3 {
                        return Err(SketchError::Invalid("periodic mode requires a cubic fit spline".into()));
                    }
                    let p = pts
                        .iter()
                        .map(|i| self.point(*i).ok_or_else(|| SketchError::Invalid("missing periodic point".into())))
                        .collect::<Result<Vec<_>>>()?;
                    crate::periodic_beziers(&p)?;
                }
                "s"
            }
            CurveKind::Conic { rho, .. } => {
                if !(rho.is_finite() && *rho > 1e-6 && *rho < 1.0 - 1e-6) {
                    return Err(SketchError::Invalid("rho must be between 0 and 1".into()));
                }
                "k"
            }
        };
        let id = self.curve_id(id, prefix)?;
        self.curves.push(Curve { id, kind, construction: false, reversed: false, link: None, centerline: false, fixed: false, infinite: false });
        Ok(self.curves.len() - 1)
    }

    /// Remove curves (by index) and then any points no longer used by a curve (except the
    /// origin and free points), and constraints that referred to removed entities.
    pub fn remove_curves(&mut self, which: &[usize]) {
        let mut owned: Vec<usize> = Vec::new();
        for (i, c) in self.curves.iter().enumerate() {
            if which.contains(&i) {
                owned.extend(c.kind.point_ids());
            }
        }
        self.drop_curves(which);
        let mut orphans: Vec<usize> = owned.into_iter().filter(|p| *p != 0 && !self.point_used_by_curve(*p)).collect();
        orphans.sort_unstable();
        orphans.dedup();
        self.remove_points(&orphans);
        self.prune_links();
        self.prune_patterns();
    }

    fn point_used_by_curve(&self, p: usize) -> bool {
        self.curves.iter().any(|c| c.kind.uses(p))
    }

    /// Remove curves only (points stay), remapping constraints.
    fn drop_curves(&mut self, which: &[usize]) {
        self.prune_offsets_for_removed_curves(which);
        let mut cmap = vec![None; self.curves.len()];
        let mut n = 0;
        for (i, slot) in cmap.iter_mut().enumerate() {
            if !which.contains(&i) {
                *slot = Some(n);
                n += 1;
            }
        }
        let mut idx = 0;
        self.curves.retain(|_| {
            let k = !which.contains(&idx);
            idx += 1;
            k
        });
        let cm = |i: usize| cmap.get(i).copied().flatten();
        let pm = |i: usize| Some(i);
        self.constraints.retain_mut(|c| c.kind.remap(&pm, &cm));
    }

    /// Remove points (never the origin) and every curve/constraint using them.
    pub fn remove_points(&mut self, which: &[usize]) {
        if which.is_empty() {
            return;
        }
        let gone = |p: usize| p != 0 && which.contains(&p);
        let dead_curves: Vec<usize> =
            self.curves.iter().enumerate().filter(|(_, c)| c.kind.point_ids().into_iter().any(gone)).map(|(i, _)| i).collect();
        let private = dead_curves
            .iter()
            .filter_map(|i| self.curves.get(*i))
            .filter(|c| self.freeform_offsets.iter().any(|r| r.output == c.id))
            .flat_map(|c| c.kind.point_ids())
            .collect::<std::collections::BTreeSet<_>>();
        self.drop_curves(&dead_curves);
        let mut removed = which.iter().copied().filter(|i| *i != 0).collect::<std::collections::BTreeSet<_>>();
        // Keep any node still named by a surviving curve or constraint.
        let referenced = self.curves.iter().flat_map(|c| c.kind.point_ids()).collect::<std::collections::BTreeSet<_>>();
        let constraint_points = std::cell::RefCell::new(std::collections::BTreeSet::new());
        for constraint in &self.constraints {
            let mut kind = constraint.kind.clone();
            kind.remap(
                &|p| {
                    constraint_points.borrow_mut().insert(p);
                    Some(p)
                },
                &Some,
            );
        }
        let constraint_points = constraint_points.into_inner();
        for index in private {
            if index != 0 && !referenced.contains(&index) && !constraint_points.contains(&index) {
                removed.insert(index);
            }
        }
        let gone = |p: usize| removed.contains(&p);
        let mut pmap = vec![None; self.points.len()];
        let mut n = 0;
        for (i, slot) in pmap.iter_mut().enumerate() {
            if !gone(i) {
                *slot = Some(n);
                n += 1;
            }
        }
        let pm = |i: usize| pmap.get(i).copied().flatten();
        for c in &mut self.curves {
            c.kind.map_points(&|i| pm(i).unwrap_or(0));
        }
        let cm = |i: usize| Some(i);
        self.constraints.retain_mut(|c| c.kind.remap(&pm, &cm));
        let mut idx = 0;
        self.points.retain(|_| {
            let k = !gone(idx);
            idx += 1;
            k
        });
        self.prune_links();
        self.prune_patterns();
    }

    /// Resolve a point reference: a point id (`origin`, `l1.end`, `p3`) or `<curve>.start|end|center`.
    pub fn resolve_point(&self, r: &str) -> Option<usize> {
        if let Some(i) = self.point_index(r) {
            return Some(i);
        }
        let (cid, role) = r.rsplit_once('.')?;
        let c = self.curves.get(self.curve_index(cid)?)?;
        let role = match (role, c.reversed) {
            ("start", true) => "end",
            ("end", true) => "start",
            (r, _) => r,
        };
        match (&c.kind, role) {
            (CurveKind::Circle { c, .. }, "center") | (CurveKind::Arc { c, .. }, "center") | (CurveKind::Ellipse { c, .. }, "center") => Some(*c),
            (CurveKind::Ellipse { m, .. }, "major") => Some(*m),
            (CurveKind::Conic { apex, .. }, "apex") => Some(*apex),
            (k, "start") => k.ends().map(|e| e.0),
            (k, "end") => k.ends().map(|e| e.1),
            _ => None,
        }
    }

    /// Axis-aligned bounds of all geometry (for views).
    pub fn bounds(&self) -> Option<(Vec2, Vec2)> {
        let mut lo = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut any = false;
        for i in 0..self.curves.len() {
            for s in self.segs(i) {
                for p in s.polyline(0.5) {
                    lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
                    hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
                    any = true;
                }
            }
        }
        any.then_some((lo, hi))
    }
}
