use serde::{Deserialize, Serialize};
use solidcraft_geom::{Plane, Vec2, Vec3};
use solidcraft_kernel::Continuity;
use solidcraft_sketch::Sketch;

use crate::construct::{AxisDef, GeoRef, PointDef};
use crate::expr::{self, Expr, Kind};
use crate::names::{BodyName, FeatureName, SketchId};
use crate::points::{Dir, FacePick, Position};
use crate::{DocError, Result};

pub const MAX_FEATURES: usize = 10_000;
pub const MAX_PARAMS: usize = 10_000;
/// Largest feature id a replayed feature may keep (`Document::insert_feature`): exact in JSON
/// numbers, and far from overflowing the next id.
pub const MAX_FEATURE_ID: u64 = 1 << 52;

/// A named parameter. User parameters are created by the user; model parameters (`d1`, `d2`…)
/// are created by dimensions and features.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub expr: String,
    /// `mm`, `deg` or empty (unit-less).
    pub unit: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// Created by a dimension or feature (not by the user).
    #[serde(default)]
    pub model: bool,
}

impl Param {
    pub fn kind(&self) -> Kind {
        Kind::from_unit(&self.unit)
    }
}

/// Where a sketch lies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlaneRef {
    /// `XY`, `XZ` or `YZ`.
    Origin { name: String },
    /// An explicit plane (e.g. picked from a planar face).
    Custom { plane: Plane },
    /// An explicit plane whose origin is given as length expressions
    /// (`{origin: ["half", 0, 0], normal: [1, 0, 0]}`), so it follows parameter changes.
    Placed {
        #[serde(with = "point_expr")]
        origin: PointExpr,
        x_dir: Vec3,
        y_dir: Vec3,
    },
    /// Another plane offset along its normal by an expression.
    Offset { base: Box<PlaneRef>, distance: Expr },
    /// Another plane rotated about a line (in world coordinates) by an angle expression.
    AtAngle { base: Box<PlaneRef>, axis_origin: Vec3, axis_dir: Vec3, angle: Expr },
    /// A construction plane feature (by name).
    Construction { name: FeatureName },
    /// `base` seen from its other side: the normal reversed by a half turn about the base's
    /// own x axis (y reversed), or about its y axis (x reversed) when `about_y`. A sketch
    /// started from behind a plane faces the viewer this way.
    Flipped {
        base: Box<PlaneRef>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        about_y: bool,
    },
    /// Halfway between two planes (or planar faces); bisecting them when they meet at an angle.
    Midplane { a: Box<GeoRef>, b: Box<GeoRef> },
    /// Through two straight edges (lines) that lie in one plane.
    TwoEdges { a: Box<GeoRef>, b: Box<GeoRef> },
    /// Through three points.
    ThreePoints { a: Box<GeoRef>, b: Box<GeoRef>, c: Box<GeoRef> },
    /// Tangent to a face (cylinder, cone, sphere, torus) where the point `at` lies over it.
    Tangent { face: Box<GeoRef>, at: Vec3 },
    /// Square to an edge or sketch curve, a fraction `t` (expression, 0…1) of the way along it.
    AlongPath { path: Box<GeoRef>, t: Expr },
    /// Through a straight edge (or line, axis) and square to a plane (or planar face).
    Perpendicular { plane: Box<GeoRef>, line: Box<GeoRef> },
    /// A planar body face. Certified attachments resolve only their exact source and report
    /// an error when it is lost; `plane` then supplies a repair guide. Legacy references without
    /// an anchor try the persistent `name`, then the nearest parallel face at `at`, and retain
    /// the picked `plane` as their historical fallback.
    Face {
        plane: Plane,
        at: Vec3,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Exact attachment identity for newly certified faces; absent on legacy designs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor: Option<Box<crate::FaceAnchor>>,
    },
}

/// Complete source curves of an outer boundary and each of its holes. Unlike a
/// span multiset, this identity is independent of adaptive spline subdivision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WholeProfileBoundary {
    pub outer: Vec<String>,
    pub holes: Vec<Vec<String>>,
}

/// Which closed profiles of a sketch a feature uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileSel {
    /// Every profile.
    All,
    /// Profiles by index in the solved sketch's profile list.
    Indices { indices: Vec<usize> },
    /// Profiles whose outer loop is made of exactly these curves, or pieces of them where other
    /// curves cut them (order and repeats don't matter; see `sketch::profile_by_curves`). Found
    /// again on every evaluation, so stable across parameter changes; no match or several is
    /// an error, never another profile.
    Curves { loops: Vec<Vec<String>> },
    /// Complete current boundaries of topology-aware analytic offset associations.
    /// The local association identity survives retirement and resurrection of arc roles.
    OffsetBoundaries { associations: Vec<String> },
    /// Exact whole source coverage, including holes; partial source curves do not match.
    WholeBoundaries { profiles: Vec<WholeProfileBoundary> },
    /// The profile containing each sketch point.
    Points { points: Vec<Vec2> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    #[default]
    NewBody,
    Join,
    Cut,
    Intersect,
}

impl Operation {
    pub fn parse(s: &str) -> Option<Operation> {
        Some(match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "new" | "newbody" | "newcomponent" => Operation::NewBody,
            "join" | "union" | "add" => Operation::Join,
            "cut" | "subtract" | "difference" => Operation::Cut,
            "intersect" | "intersection" => Operation::Intersect,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Along the sketch normal.
    #[default]
    Positive,
    Negative,
    /// Both sides; the extent convention determines whether `distance` is total or per side.
    Symmetric,
}

/// The direction a ruled surface leaves its edges (`surface.ruled`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuledMode {
    /// Along the normal of the face beside each edge.
    #[default]
    Normal,
    /// Continuing the face beside each edge.
    Tangent,
    /// At an angle from a reference direction (the face's normal unless one is given).
    Angle,
    /// Along a fixed direction.
    Direction,
}

impl RuledMode {
    fn is_normal(&self) -> bool {
        *self == RuledMode::Normal
    }
}

/// How a ruled surface's strips meet at sharp corners of its edge chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuledCorners {
    /// Extended or cut back until they meet.
    #[default]
    Mitre,
    /// Joined by a sector (inside corners are cut back).
    Round,
}

impl RuledCorners {
    fn is_mitre(&self) -> bool {
        *self == RuledCorners::Mitre
    }
}

/// Angular starts in older designs used the raw axis sign. New revolves measure starts
/// along the same sketch-normal sweep direction as the kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevolveConvention {
    #[default]
    LegacyAxisStart,
    SketchNormal,
}

/// Symmetric distance semantics. Missing markers preserve existing designs' thickness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtrudeConvention {
    #[default]
    LegacyPerSide,
    TotalDistance,
}

impl ExtrudeConvention {
    /// Distance from the mid-plane to either cap for a distance-limited extent.
    pub fn symmetric_half_distance(self, distance: f64) -> f64 {
        match self {
            Self::LegacyPerSide => distance,
            Self::TotalDistance => distance * 0.5,
        }
    }
}

/// Extent of an extrude.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct Extent {
    #[walk(length = "Distance")]
    pub distance: Expr,
    #[serde(default)]
    pub convention: ExtrudeConvention,
    #[serde(default)]
    pub direction: Direction,
    /// Optional second side distance (two-sided extrude), along the negative normal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[walk(length = "Distance 2")]
    pub distance2: Option<Expr>,
    /// Start offset from the sketch plane (expression), along the sketch normal whatever the
    /// direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[walk(length = "Start offset")]
    pub start_offset: Option<Expr>,
    /// Through all bodies in the direction(s) instead of `distance`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub through_all: bool,
    /// Taper angle (expression; positive grows the profile).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[walk(angle = "Taper angle")]
    pub taper: Option<Expr>,
    /// To Object: end at the face (or vertex/point) at this point, instead of `distance`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<FacePick>,
    /// Offset past the To Object face (expression; negative stops short).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[walk(length = "To offset")]
    pub to_offset: Option<Expr>,
    /// From Object: start at the face (or point) here instead of the sketch plane (plus
    /// `start_offset`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<FacePick>,
}

/// Thin Extrude: a wall of constant thickness along sketch curves instead of the profiles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct ThinWall {
    /// The sketch curves the wall follows: open chains and closed loops, joined where they
    /// meet end to end.
    pub curves: Vec<String>,
    /// The wall thickness (expression); with `side: two_sides`, the thickness inside.
    #[walk(length = "Thickness")]
    pub thickness: Expr,
    #[serde(default, skip_serializing_if = "ThinSide::is_center")]
    pub side: ThinSide,
    /// With `side: two_sides`, the thickness outside (expression).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[walk(length = "Thickness 2")]
    pub thickness2: Option<Expr>,
}

/// How a swept profile turns as it follows its path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
#[serde(rename_all = "snake_case")]
pub enum SweepOrientation {
    /// Square to the path, turning with it (a rotation-minimising frame).
    #[default]
    Perpendicular,
    /// As drawn: the profile moves along the path without turning.
    Parallel,
    /// Square to the path, its turn about the path fixed by this direction (in the feature's
    /// frame): the direction's projection onto the profile stays where it was at the start.
    Direction(Dir),
}

impl SweepOrientation {
    pub fn is_perpendicular(&self) -> bool {
        *self == SweepOrientation::Perpendicular
    }
}

/// A sweep's guide rail: a chain of curves of a sketch, followed from the end nearest the path's
/// start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct SweepRail {
    pub sketch: SketchId,
    pub curves: Vec<String>,
}

/// How a sweep's guide rails reshape its profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RailFit {
    /// Evenly, with the distance across to the rail (between the rails).
    #[default]
    Scale,
    /// Only across to the rail (between the rails).
    Stretch,
}

impl RailFit {
    pub fn is_scale(&self) -> bool {
        *self == RailFit::Scale
    }
}

/// Where a thin wall lies against its curves. A closed loop's inside is the area it encloses;
/// an open chain's is the side it bends toward (a straight one's, the left of its first
/// curve as drawn).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinSide {
    /// Half the thickness either side of the curves.
    #[default]
    Center,
    Inside,
    Outside,
    /// `thickness` inside and `thickness2` outside.
    TwoSides,
}

impl ThinSide {
    pub fn is_center(&self) -> bool {
        *self == ThinSide::Center
    }
    pub fn parse(s: &str) -> Option<ThinSide> {
        match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "center" | "centre" | "centered" | "centred" | "symmetric" => Some(ThinSide::Center),
            "inside" | "side1" | "side_1" => Some(ThinSide::Inside),
            "outside" | "side2" | "side_2" => Some(ThinSide::Outside),
            "two_sides" | "both" => Some(ThinSide::TwoSides),
            _ => None,
        }
    }
}

/// Revolve axis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AxisRef {
    /// A line of the profile's sketch.
    SketchLine { curve: String },
    /// Sketch x or y axis through the sketch origin (`x` / `y`).
    SketchAxis { axis: String },
    /// World X / Y / Z axis through the origin.
    World { axis: String },
    /// Any line in the sketch plane, in world coordinates.
    Line { origin: Position, dir: Dir },
}

/// Persistent cylindrical Emboss selection, authored in the feature component frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct EmbossFace {
    pub body: BodyName,
    #[walk(pinned)]
    pub at: FacePick,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub author_path: Vec<u64>,
}

impl EmbossFace {
    /// The selected sketch placement, shared by evaluation and interactive guides.
    pub fn sketch_frame(&self, doc: &Document, author: u64, sketch_owner: u64, body: &crate::ModelBody) -> Result<crate::Mat> {
        let owner = doc.model_body_component(body);
        if sketch_owner == owner {
            return Self::component_frame(doc, owner, &self.path);
        }
        if sketch_owner == author {
            return Self::component_frame(doc, author, &self.author_path);
        }
        let path = doc.first_path(sketch_owner);
        let mut component = sketch_owner;
        for _ in 0..64 {
            if component == 0 {
                break;
            }
            let mut occurrences = doc.occurrences.iter().filter(|o| o.component == component);
            let occurrence = occurrences.next().ok_or_else(|| DocError::Invalid("sketch placement no longer exists".into()))?;
            if occurrences.next().is_some() {
                return Err(DocError::Invalid("sketch placement is ambiguous".into()));
            }
            component = occurrence.parent;
        }
        if component != 0 {
            return Err(DocError::Invalid("sketch placement exceeds depth limit".into()));
        }
        Self::component_frame(doc, sketch_owner, &path)
    }

    /// Resolve this exact placement without expanding every assembly instance.
    pub fn body_frame(&self, doc: &Document, body: &crate::ModelBody) -> Result<crate::Mat> {
        let owner = doc.model_body_component(body);
        let mut frame = Self::component_frame(doc, owner, &self.path)?;
        if let Some(offset) = doc.model_body_offset(body) {
            frame = crate::mat_mul(&frame, offset);
        }
        Ok(frame)
    }

    /// Resolve one saved component path without expanding other occurrences.
    pub fn component_frame(doc: &Document, owner: u64, path: &[u64]) -> Result<crate::Mat> {
        if path.len() > 64 || doc.occurrences.len() > 100_000 {
            return Err(DocError::Invalid("selected cylinder path exceeds work limits".into()));
        }
        let mut frame = crate::IDENTITY;
        let mut component = 0;
        let mut seen = std::collections::BTreeSet::new();
        for id in path {
            if *id == 0 || !seen.insert(*id) {
                return Err(DocError::Invalid("invalid selected cylinder path".into()));
            }
            let occurrence = doc
                .occurrences
                .iter()
                .find(|o| o.id == *id && o.parent == component)
                .ok_or_else(|| DocError::Invalid("selected cylinder placement no longer exists".into()))?;
            frame = crate::mat_mul(&frame, &occurrence.transform);
            component = occurrence.component;
        }
        if path.is_empty() {
            frame = doc.component_transform(owner);
        } else if component != owner {
            return Err(DocError::Invalid("selected cylinder path belongs to another component".into()));
        }
        Ok(frame)
    }
}

/// One edge of an edge flange feature: a point on the sheet's edge and, where given, this
/// edge's own angle and height (expressions) in place of the feature's. In a design file an
/// edge with neither is the point `[x, y, z]`, as every edge was before; one with its own
/// values is `{"point": [x, y, z], "angle": "…", "height": "…"}`.
#[derive(Clone, Debug, Default, PartialEq, solidcraft_derive::Walk)]
pub struct FlangeEdge {
    pub point: Position,
    #[walk(angle = "Edge angle")]
    pub angle: Option<Expr>,
    #[walk(length = "Edge height")]
    pub height: Option<Expr>,
}

impl FlangeEdge {
    /// An edge that takes the feature's angle and height.
    pub fn at(point: Vec3) -> Self {
        Self { point: Position(point), angle: None, height: None }
    }
}

impl Serialize for FlangeEdge {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        if self.angle.is_none() && self.height.is_none() {
            return self.point.serialize(s);
        }
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("point", &self.point)?;
        if let Some(a) = &self.angle {
            m.serialize_entry("angle", a)?;
        }
        if let Some(h) = &self.height {
            m.serialize_entry("height", h)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for FlangeEdge {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::value::{MapAccessDeserializer, SeqAccessDeserializer};
        /// The object form; a key this build doesn't know is refused, not dropped.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Own {
            point: Vec3,
            #[serde(default)]
            angle: Option<Expr>,
            #[serde(default)]
            height: Option<Expr>,
        }
        struct Edge;
        impl<'de> serde::de::Visitor<'de> for Edge {
            type Value = FlangeEdge;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a point [x, y, z] or an edge {point, angle?, height?}")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, seq: A) -> std::result::Result<FlangeEdge, A::Error> {
                Vec3::deserialize(SeqAccessDeserializer::new(seq)).map(FlangeEdge::at)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> std::result::Result<FlangeEdge, A::Error> {
                let o = Own::deserialize(MapAccessDeserializer::new(map))?;
                Ok(FlangeEdge { point: Position(o.point), angle: o.angle, height: o.height })
            }
        }
        d.deserialize_any(Edge)
    }
}

/// Feature definitions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FeatureKind {
    /// A recorded timeline ownership boundary. Geometry is converted from its
    /// current local frame into the destination component frame exactly once.
    BodyTransfer {
        bodies: Vec<BodyName>,
        from: u64,
        to: u64,
        transform: crate::Mat,
    },
    Sketch {
        plane: PlaneRef,
        sketch: Sketch,
    },
    Extrude {
        /// Feature id of the sketch.
        sketch: SketchId,
        // A thin extrude follows curves; its profiles are not used.
        #[walk(profiles_of = sketch, when = thin.is_none())]
        profiles: ProfileSel,
        extent: Extent,
        #[serde(default)]
        operation: Operation,
        /// Target body names for join/cut/intersect (empty = all bodies).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyName>,
        /// Thin: a wall along sketch curves instead of the profiles (which are then unused).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thin: Option<ThinWall>,
    },
    Revolve {
        sketch: SketchId,
        #[walk(profiles_of = sketch)]
        profiles: ProfileSel,
        axis: AxisRef,
        #[walk(angle = "Angle")]
        angle: Expr,
        #[serde(default)]
        operation: Operation,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyName>,
        /// Two Sides: the angle on the other side.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Angle 2")]
        angle2: Option<Expr>,
        /// Symmetric: `angle` split evenly either side of the profile.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        symmetric: bool,
        /// Missing fields retain the angular placement of existing designs.
        #[serde(default)]
        convention: RevolveConvention,
        /// To Object: turn until the face, vertex or point here.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<FacePick>,
    },
    Fillet {
        /// Reference points on the edges to round (each re-finds the nearest edge).
        edges: Vec<Position>,
        #[walk(length = style.radius_label())]
        radius: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
        /// Constant radius (the default), chord length (`radius` is the width across), or a
        /// radius varying along each edge.
        #[serde(default, skip_serializing_if = "FilletStyle::is_constant")]
        style: FilletStyle,
        /// What to do when the radius is too large for the geometry around the edges.
        #[serde(default, skip_serializing_if = "FilletFit::is_exact")]
        fit: FilletFit,
        /// What fills the corners where three blended edges meet: the rolling ball's sphere
        /// (default) or a G2 patch repeating each blend up to the second order.
        #[serde(default, skip_serializing_if = "FilletCorner::is_rolling")]
        corner: FilletCorner,
    },
    Chamfer {
        edges: Vec<Position>,
        #[walk(length = "Distance")]
        distance: Expr,
        /// Unequal chamfers: the distance along the second face…
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Distance 2")]
        distance2: Option<Expr>,
        /// …or the angle the chamfer makes with the first face.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Angle")]
        angle: Option<Expr>,
        /// Swap which face is the first (by default the one facing up most).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    Box {
        #[serde(with = "point_expr")]
        #[walk(placement)]
        corner: PointExpr,
        #[walk(length = "Length")]
        length: Expr,
        #[walk(length = "Width")]
        width: Expr,
        #[walk(length = "Height")]
        height: Expr,
        #[serde(default)]
        operation: Operation,
    },
    Cylinder {
        #[serde(with = "point_expr")]
        #[walk(placement)]
        base: PointExpr,
        #[serde(default = "z_axis")]
        axis: Dir,
        #[walk(length = "Radius")]
        radius: Expr,
        #[walk(length = "Height")]
        height: Expr,
        #[serde(default)]
        operation: Operation,
    },
    Sphere {
        #[serde(with = "point_expr")]
        #[walk(placement)]
        center: PointExpr,
        #[walk(length = "Radius")]
        radius: Expr,
        #[serde(default)]
        operation: Operation,
    },
    Torus {
        #[serde(with = "point_expr")]
        #[walk(placement)]
        center: PointExpr,
        #[walk(length = "Major radius")]
        major: Expr,
        #[walk(length = "Minor radius")]
        minor: Expr,
        #[serde(default)]
        operation: Operation,
    },
    Combine {
        target: BodyName,
        tools: Vec<BodyName>,
        operation: Operation,
        #[serde(default)]
        keep_tools: bool,
    },
    /// Copies of other features' results, or of bodies (rectangular, circular or along a path).
    Pattern {
        features: Vec<FeatureName>,
        pattern: PatternKind,
        /// Copy these bodies (as new bodies) instead of replaying features.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bodies: Vec<BodyName>,
    },
    /// Mirror copies of other features' results in a plane.
    Mirror {
        features: Vec<FeatureName>,
        plane: PlaneRef,
        /// Mirror these bodies instead of replaying features.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bodies: Vec<BodyName>,
        /// Join each mirrored body with its original.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        combine: bool,
        /// Where each of `bodies` is mirrored from: the occurrence path of the placement it was
        /// picked at. The body is mirrored where that placement shows it, as the feature's own
        /// component sees it. Empty in older designs, which mirror the bodies' own geometry.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        placements: Vec<Vec<u64>>,
    },
    /// A drilled hole (simple, counterbore or countersink) at a point, into the material.
    Hole {
        #[walk(pick_if = points.is_none())]
        position: FacePick,
        /// The position as length expressions, when it was given that way (#1678): the hole is
        /// drilled where they evaluate, and follows the parameters. `position` keeps the point
        /// they gave when the hole was made.
        #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_point_expr")]
        #[walk(placement)]
        at: Option<PointExpr>,
        /// Drilling direction (into the material).
        direction: Dir,
        #[walk(length = "Diameter")]
        diameter: Expr,
        /// Depth to the shoulder; `None` = through all.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Depth")]
        depth: Option<Expr>,
        #[serde(default)]
        hole: HoleKind,
        /// Drill at these sketch points instead (one hole each, perpendicular to the sketch, into the material: against the sketch normal, or along it when only that side has material);
        /// the holes follow the points when the sketch changes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        points: Option<SketchPoints>,
        /// A cosmetic thread in the holes (e.g. "M6x1").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<String>,
        /// To Object: drill down to the face (or point) here instead of `depth`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<FacePick>,
    },
    /// A thread on a cylindrical face. Cosmetic by default (no geometry change; drawn and
    /// exported as an annotation); `modeled` cuts the real helical ISO profile into the face.
    /// `designation` is an ISO metric size such as "M8" or "M8x1".
    Thread {
        face: FacePick,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        designation: Option<String>,
        /// Thread length from the face's end nearest `face`; `None` = full length.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Length")]
        length: Option<Expr>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        modeled: bool,
    },
    /// Ruled loft through profiles of several sketches, in order.
    Loft {
        sections: Vec<LoftSection>,
        #[serde(default)]
        operation: Operation,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyName>,
    },
    /// Sweep profiles along a chain of sketch curves.
    Sweep {
        sketch: SketchId,
        #[walk(profiles_of = sketch)]
        profiles: ProfileSel,
        path_sketch: SketchId,
        path: Vec<String>,
        /// How the profile turns along the path (default: perpendicular to it).
        #[serde(default, skip_serializing_if = "SweepOrientation::is_perpendicular")]
        orientation: SweepOrientation,
        /// Angle the profile turns about the path over its length.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Twist")]
        twist: Option<Expr>,
        /// The profile's scale at the end of the path (linear along its length).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(unitless = "Scale")]
        scale: Option<Expr>,
        /// Guide rails (one or two) the profile follows, instead of orientation, twist and scale.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rails: Vec<SweepRail>,
        /// How the rails reshape the profile.
        #[serde(default, skip_serializing_if = "RailFit::is_scale")]
        rail_fit: RailFit,
        #[serde(default)]
        operation: Operation,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyName>,
    },
    /// Hollow a body leaving walls of `thickness`, removing the faces at the given points.
    Shell {
        faces: Vec<FacePick>,
        #[walk(length = "Thickness")]
        thickness: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
        /// inside (default) | outside | both (the wall straddles the faces).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        direction: String,
        /// Open the faces smoothly joined to the picked ones too.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        tangent_chain: bool,
    },
    /// Tilt faces (at the given points) about a neutral plane, leaning toward `pull`.
    Draft {
        faces: Vec<FacePick>,
        #[walk(angle = "Angle")]
        angle: Expr,
        neutral: PlaneRef,
        pull: Dir,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// A construction plane (sketch placement, split tool).
    ConstructionPlane {
        plane: PlaneRef,
    },
    /// Split a body into the parts on either side of a plane, or of a face of a body (the face
    /// through `point`, its surface extended) when `tool` is set.
    Split {
        body: BodyName,
        plane: PlaneRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<FaceAt>,
        /// A straight line of this sketch as the tool (#1552): the plane through it along the
        /// sketch's normal, which follows the sketch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sketch: Option<SketchId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<String>,
    },
    /// A construction axis (Fusion's CONSTRUCT axes).
    ConstructionAxis {
        def: AxisDef,
    },
    /// A construction point (Fusion's CONSTRUCT points).
    ConstructionPoint {
        def: PointDef,
    },
    /// Split faces (at the given points) where a tool meets them, the solid unchanged: a plane,
    /// a face of a body (`tool`, its surface extended) or a sketch's curves (`sketch`, swept
    /// along its normal). The pieces are named after the face (`#k`).
    SplitFace {
        faces: Vec<FacePick>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
        plane: PlaneRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<FaceAt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sketch: Option<SketchId>,
    },
    /// Scale bodies about a point (uniform, or per axis).
    Scale {
        bodies: Vec<BodyName>,
        #[serde(default)]
        origin: Position,
        #[walk(unitless = "Scale factor")]
        factor: Expr,
        /// Per-axis factors instead of `factor`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(unitless = ["X scale", "Y scale", "Z scale"])]
        factors: Option<[Expr; 3]>,
    },
    /// Move planar faces (at the given points) along their normals; the body keeps its shape.
    OffsetFace {
        faces: Vec<FacePick>,
        #[walk(length = "Distance")]
        distance: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// Remove faces (at the given points) and heal the gap from the neighbouring planes, as when
    /// a fillet, chamfer, hole or boss is taken off. A picked face brings the rest of its surface
    /// (both halves of a hole's wall).
    DeleteFace {
        faces: Vec<FacePick>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
        /// Close the gap by extending the neighbouring faces (the default); `false` leaves it
        /// open, the faces left a surface body.
        #[serde(default = "yes", skip_serializing_if = "is_yes")]
        heal: bool,
    },
    /// A box around bodies, grown by a margin.
    BoundingSolid {
        bodies: Vec<BodyName>,
        #[serde(default = "zero_expr")]
        #[walk(length = "Margin")]
        margin: Expr,
    },
    /// A tube along a chain of sketch curves (solid, or hollow with a wall thickness).
    Pipe {
        path_sketch: SketchId,
        path: Vec<String>,
        #[walk(length = "Diameter")]
        diameter: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Wall")]
        wall: Option<Expr>,
        #[serde(default)]
        operation: Operation,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyName>,
    },
    /// Raise or sink profiles on a planar face, or wrap them on a selected cylindrical wall.
    Emboss {
        sketch: SketchId,
        #[walk(profiles_of = sketch)]
        profiles: ProfileSel,
        #[walk(length = "Depth")]
        depth: Expr,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        deboss: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        face: Option<EmbossFace>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyName>,
    },
    /// A thin wall from open sketch curves: `thickness` across the sketch plane, filling from
    /// the curves toward the body (to the next face, or `depth`), or with `normal` along the
    /// sketch normal. A web makes one per curve.
    Rib {
        sketch: SketchId,
        curves: Vec<String>,
        #[walk(length = "Thickness")]
        thickness: Expr,
        /// Depth from the curves; `None` = to the next face of the body.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Depth")]
        depth: Option<Expr>,
        /// Fill toward the other side of the curves.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
        /// Web: each curve is its own wall.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        web: bool,
        /// Grow the wall along the sketch normal (thickness across the curves, in the sketch
        /// plane) instead of across the plane from the curves.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        normal: bool,
    },
    /// Move planar faces (at the given points) onto a target plane parallel to them.
    ReplaceFace {
        faces: Vec<FacePick>,
        target: PlaneRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// Move bodies so that `from` lands on `to` (and, with normals, the faces meet face to face).
    Align {
        bodies: Vec<BodyName>,
        from: Position,
        to: Position,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_normal: Option<Dir>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_normal: Option<Dir>,
        /// Normals the same way instead of facing each other.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
    },
    /// A helical coil: a circular or square section swept `turns` times about `axis`, rising
    /// `pitch` per turn, on a helix of `diameter` (the section's centre, or its inside or outside).
    Coil {
        #[serde(with = "point_expr")]
        #[walk(placement)]
        base: PointExpr,
        #[serde(default = "z_axis")]
        axis: Dir,
        #[walk(length = "Diameter")]
        diameter: Expr,
        #[walk(length = "Pitch")]
        pitch: Expr,
        #[walk(unitless = "Revolutions")]
        turns: Expr,
        #[walk(length = "Section size")]
        section_size: Expr,
        #[serde(default)]
        section: CoilSection,
        /// -1 inside, 0 on the centre, 1 outside of the diameter.
        #[serde(default, skip_serializing_if = "is_zero_i8")]
        position: i8,
        /// Start angle about the axis (expression; 0 = from the axis toward +x, or a
        /// perpendicular direction when the axis is x).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Start angle")]
        start_angle: Option<Expr>,
        /// Turn the other way (left-handed).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clockwise: bool,
        #[serde(default)]
        operation: Operation,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyName>,
    },
    /// Sheet metal base flange: a sheet from a closed profile (thickness from the rule).
    SheetBase {
        sketch: SketchId,
        #[walk(profiles_of = sketch)]
        profiles: ProfileSel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<String>,
        /// Thickness grows against the sketch normal.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
    },
    /// Sheet metal contour flange: a sheet bent along an open chain of sketch lines.
    SheetContour {
        sketch: SketchId,
        curves: Vec<String>,
        #[walk(length = "Distance")]
        distance: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<String>,
        /// Material on the other side of the chain.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
        /// Width the other way along the sketch normal.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reverse: bool,
    },
    /// Edge flanges on sheet edges (points on the top or bottom edges). `height` and `angle`
    /// hold for every edge that doesn't carry its own (see [`FlangeEdge`]).
    SheetFlange {
        #[walk(after = radius)]
        edges: Vec<FlangeEdge>,
        #[walk(length = "Height")]
        height: Expr,
        #[walk(angle = "Angle")]
        angle: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Bend radius")]
        radius: Option<Expr>,
        /// inside | outside | middle.
        #[serde(default = "inside_str")]
        position: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
        /// The relief cut where two of this feature's bends meet at a corner of the base, in place
        /// of the rule's: `corner_relief_shape` (notch | round | rectangular | tear) and
        /// `corner_relief`, the size across. Either one left out follows the rule.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        corner_relief_shape: Option<String>,
        /// Walked last, after the edges' values, so the inputs before it keep their names.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Corner relief")]
        corner_relief: Option<Expr>,
    },
    /// Flat hems on sheet edges.
    SheetHem {
        edges: Vec<Position>,
        #[walk(length = "Length")]
        length: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Gap")]
        gap: Option<Expr>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// A perforation field (#732): round holes of `diameter` on a staggered (60°) or square grid of `pitch` over the
    /// whole flat sheet, kept `margin` (default: the diameter) clear of its edges, cut-outs and bend zones. Stored like
    /// a cosmetic thread: the holes go to the flat pattern, the DXF and the measured volume, not into the B-rep, so a
    /// panel can carry tens of thousands of them.
    SheetPerforate {
        #[walk(length = "Diameter")]
        diameter: Expr,
        #[walk(length = "Pitch")]
        pitch: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Margin")]
        margin: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// Fold a sheet along a line on its base face: a sketch line (followed when the sketch
    /// changes), else the points `a`, `b`.
    SheetFold {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sketch: Option<SketchId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curve: Option<String>,
        a: Position,
        b: Position,
        #[walk(angle = "Angle")]
        angle: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Bend radius")]
        radius: Option<Expr>,
        /// centerline | start | end | mould
        position: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
        /// A point on the side that stays put (default: the larger side).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fixed: Option<Position>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// Unfold a sheet body flat (all bends), or refold it.
    SheetUnfold {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        refold: bool,
    },
    /// Convert a body to sheet metal from its fixed face at `face`: a plate of constant
    /// thickness or, with `bends`, a solid or shelled body whose faces reached from the fixed
    /// face across those edges become the sheet.
    SheetConvert {
        body: BodyName,
        #[walk(pick_if = !bends.is_empty())]
        face: FacePick,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<String>,
        /// Points on the sharp edges that become bends.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bends: Vec<Position>,
    },
    /// Plastic boss: a round post on a face (draft, root fillet, hole, ribs).
    Boss {
        position: FacePick,
        /// Out of the face (the boss grows along it).
        #[serde(default = "z_axis")]
        direction: Dir,
        #[walk(length = "Diameter")]
        diameter: Expr,
        #[walk(length = "Height")]
        height: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Hole diameter")]
        hole_diameter: Option<Expr>,
        /// Hole depth from the top (default: down to the face).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Hole depth")]
        hole_depth: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Draft angle")]
        draft: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Root fillet")]
        fillet: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(unitless = "Ribs")]
        ribs: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Rib thickness")]
        rib_thickness: Option<Expr>,
        /// Rib reach out from the boss wall.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Rib length")]
        rib_length: Option<Expr>,
        /// Rib top below the boss top.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Rib offset")]
        rib_offset: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// Plastic lip (raised band) or groove (recess) along the rim face picked at `face`.
    Lip {
        face: Position,
        #[walk(length = "Width")]
        width: Expr,
        #[walk(length = "Height")]
        height: Expr,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        groove: bool,
        /// Clearance added to a groove's width (and depth).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Gap")]
        gap: Option<Expr>,
        /// Along the rim's outer edge instead of its inner one.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        outside: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// Plastic snap fit: a cantilever arm on a face with a hook (catch) at its tip.
    SnapFit {
        position: FacePick,
        /// The arm grows along this (out of the face).
        #[serde(default = "z_axis")]
        direction: Dir,
        /// The catch sticks out this way.
        hook: Dir,
        #[walk(length = "Length")]
        length: Expr,
        #[walk(length = "Thickness")]
        thickness: Expr,
        #[walk(length = "Width")]
        width: Expr,
        #[walk(length = "Catch depth")]
        catch_depth: Expr,
        /// The catch's height along the arm.
        #[walk(length = "Catch length")]
        catch_length: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// Plastic rest: a raised pad on a face (round, or a rectangle), solid or a hollow wall,
    /// with draft.
    Rest {
        position: FacePick,
        /// Out of the face (the pad grows along it).
        #[serde(default = "z_axis")]
        direction: Dir,
        /// The rectangle's length runs along this (default: any direction in the face).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        along: Option<Dir>,
        /// Diameter of a round rest, or the rectangle's width.
        #[walk(length = "Width")]
        width: Expr,
        /// The rectangle's length (none: round).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Length", after = height)]
        length: Option<Expr>,
        #[walk(length = "Height")]
        height: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Draft angle")]
        draft: Option<Expr>,
        /// Wall thickness of a hollow rest (none: solid).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Wall thickness")]
        thickness: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
    },
    /// Boundary Fill: solids from the cells the tool bodies enclose (a cell: inside some
    /// tools and outside the rest), each picked by a point inside it.
    BoundaryFill {
        tools: Vec<BodyName>,
        cells: Vec<Position>,
        #[serde(default)]
        operation: Operation,
        /// Remove the tool bodies afterwards.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        remove_tools: bool,
    },
    /// Remove bodies from the model (from here on in the timeline).
    Remove {
        bodies: Vec<BodyName>,
    },
    /// Surface bodies: planar patches filling sketch regions (`profiles` of `sketch`), or a
    /// closed loop of a body's edges (`edges`: points on them, on `body`).
    Patch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sketch: Option<SketchId>,
        #[serde(default = "all_profiles")]
        #[walk(profiles_of = sketch)]
        profiles: ProfileSel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        edges: Vec<Position>,
        /// How full a fitted patch's middle is (`crown`, -0.9…3, default 0; a planar loop is an
        /// exact plane whatever this says).
        #[serde(default, skip_serializing_if = "is_zero")]
        crown: f64,
        /// How a patch on edges meets the faces beside its loop (G0 everywhere by default, and
        /// then not saved).
        #[serde(default, skip_serializing_if = "PatchContinuity::is_g0")]
        continuity: PatchContinuity,
        /// Points a patch on edges passes through.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        points: Vec<Position>,
    },
    /// Surface bodies sewn along shared edges into the first one (a solid when they close).
    Stitch {
        bodies: Vec<BodyName>,
        #[serde(default = "zero_expr")]
        #[walk(length = "Tolerance")]
        tolerance: Expr,
    },
    /// Surface bodies thickened into solids along their normals (by half each way when
    /// `symmetric`).
    Thicken {
        bodies: Vec<BodyName>,
        #[walk(length = "Thickness")]
        thickness: Expr,
        #[serde(default)]
        symmetric: bool,
        operation: Operation,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<BodyName>,
    },
    /// A surface body swept straight along the sketch normal from open curve chains and the
    /// boundaries of profiles: walls only, no caps.
    SurfaceExtrude {
        sketch: SketchId,
        /// Profiles whose boundary loops are swept (none: only `curves`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(profiles_of = sketch)]
        profiles: Option<ProfileSel>,
        /// Sketch curve ids, grouped into chains of curves that meet end to end.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        curves: Vec<String>,
        extent: Extent,
    },
    /// A body taken apart: each face at `faces` (every face when empty) its own surface body,
    /// the faces left one surface body under the body's name. A solid comes apart into surfaces.
    Unstitch {
        body: BodyName,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        faces: Vec<Position>,
    },
    /// A surface body with the faces at `faces` (every face when empty) turned over.
    ReverseNormal {
        body: BodyName,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        faces: Vec<Position>,
    },
    /// The face at `face` restored towards its surface: the inner loops at the `holes` points
    /// taken away (every one when `None`; none when empty), and with `outer` its outer boundary
    /// back at the surface's natural bounds (an analytic surface's: the face's own span). A new
    /// surface body; with `in_place`, a surface body's face is untrimmed where it is.
    SurfaceUntrim {
        // The face is picked (and named); the holes are picked by points on their edges.
        #[walk(picks_faces)]
        face: FaceAt,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        holes: Option<Vec<Position>>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        outer: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        in_place: bool,
    },
    /// A surface body turned about an axis from open curve chains and the boundaries of
    /// profiles: walls only, no caps (a solid where a full turn closes them).
    SurfaceRevolve {
        sketch: SketchId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(profiles_of = sketch)]
        profiles: Option<ProfileSel>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        curves: Vec<String>,
        axis: AxisRef,
        #[walk(angle = "Angle")]
        angle: Expr,
        /// Two Sides: the angle on the other side.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Angle 2")]
        angle2: Option<Expr>,
        /// Symmetric: `angle` split evenly either side of the sketch.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        symmetric: bool,
    },
    /// A surface body swept along a chain of sketch curves from open curve chains and the
    /// boundaries of profiles: walls only, no caps.
    SurfaceSweep {
        sketch: SketchId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(profiles_of = sketch)]
        profiles: Option<ProfileSel>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        curves: Vec<String>,
        path_sketch: SketchId,
        path: Vec<String>,
        /// How the profile turns along the path (default: perpendicular to it).
        #[serde(default, skip_serializing_if = "SweepOrientation::is_perpendicular")]
        orientation: SweepOrientation,
        /// Angle the profile turns about the path over its length.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Twist")]
        twist: Option<Expr>,
        /// The profile's scale at the end of the path (linear along its length).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(unitless = "Scale")]
        scale: Option<Expr>,
        /// Guide rails (one or two) the curves follow, instead of orientation, twist and scale.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rails: Vec<SweepRail>,
        /// How the rails reshape the curves.
        #[serde(default, skip_serializing_if = "RailFit::is_scale")]
        rail_fit: RailFit,
    },
    /// A surface body lofted through sections in order, each one open or closed chain of
    /// sketch curves: walls only, no caps.
    SurfaceLoft {
        sections: Vec<SheetSection>,
        #[serde(default = "loft_plain_continuity", skip_serializing_if = "loft_continuity_is_plain")]
        continuity: [Continuity; 2],
        #[serde(default, skip_serializing_if = "loft_crown_is_plain")]
        crown: [f64; 2],
    },
    /// The faces at `faces` copied out at a signed `distance` along their normals as a new
    /// surface body (0 copies them); `chain` takes each pick's whole smooth chain of faces;
    /// fitted faces within `tolerance`.
    SurfaceOffset {
        faces: Vec<FacePick>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodyName>,
        #[walk(length = "Distance")]
        distance: Expr,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        chain: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Tolerance")]
        tolerance: Option<Expr>,
    },
    /// A strip of ruled surface `distance` wide hanging off the chain of `body`'s edges through
    /// `edges`, as a new surface body: along `mode` (`angle` from `direction`, or from the
    /// face's normal; along `direction`), from the face at `face` where it borders the chain;
    /// `flip` puts it on the other side; sharp corners as `corners` says.
    SurfaceRuled {
        body: BodyName,
        edges: Vec<Position>,
        #[walk(length = "Distance")]
        distance: Expr,
        #[serde(default, skip_serializing_if = "RuledMode::is_normal")]
        mode: RuledMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Angle")]
        angle: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<Dir>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
        #[serde(default, skip_serializing_if = "RuledCorners::is_mitre")]
        corners: RuledCorners,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        face: Option<FacePick>,
    },
    /// A surface body's boundary edges through `edges` extended outward: by `distance`, or up
    /// to `up_to`; the surface continued (`mode` natural) or a run-off face along each edge
    /// (tangent, curvature). `neighbours`: the edges next to an extended edge are lengthened to
    /// meet it (else the extension's sides run square to the edge).
    SurfaceExtend {
        body: BodyName,
        edges: Vec<Position>,
        #[walk(length = "Distance")]
        distance: Expr,
        #[serde(default, skip_serializing_if = "ExtendMode::is_natural")]
        mode: ExtendMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        up_to: Option<ExtendTo>,
        #[serde(default = "yes", skip_serializing_if = "is_yes")]
        neighbours: bool,
    },
    /// A blend surface bridging two picked edge chains of two bodies (`surface.blend`): each end
    /// repeats the surface it leaves up to its requested continuity, with the fullness `bulge`.
    SurfaceBlend {
        from: BlendSide,
        to: BlendSide,
        /// How much of each support surface the blend repeats: [from, to].
        #[serde(default = "blend_orders")]
        continuity: [Continuity; 2],
        /// How full each end leaves: [from, to]; 0 is the plain bridge.
        #[serde(default)]
        bulge: [f64; 2],
        /// Points the blend leaves towards, when the faces at the picks do not say: [from, to].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        align: Option<[Position; 2]>,
        /// Sew the blend onto its two bodies where the boundaries meet.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stitch: bool,
    },
    /// One surface patch through a network of two to four boundary curves of a sketch
    /// (`surface.boundary`, issue #763): the patch runs through the curves in loop order and leaves
    /// each boundary at its own `order` — a sketch curve's boundary is a plane, so G1 and up hold
    /// exactly — with `crown` shaping the middle without touching a boundary.
    SurfaceBoundary {
        sketch: SketchId,
        /// The curves in loop order: each one ends where the next begins, and the last where the
        /// first does.
        curves: Vec<String>,
        /// How much of the surface across each boundary the patch repeats, one per curve (none: a
        /// tangent plane, `g1`, on every side).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        orders: Vec<Continuity>,
        /// How full the middle is: 0 the plain construction, up to 3 fuller, down to -0.9 flatter.
        #[serde(default)]
        crown: f64,
        /// The stations every boundary carries.
        #[serde(default = "boundary_samples")]
        samples: usize,
    },
    /// One surface through a curve network (`surface.boundary` with `u` and `v`, issue #763): the
    /// curves `u` run one way (two or more: the surface lofts through them) and `v` cross them the
    /// other way (two each way: a Coons patch; more: a Gordon surface through every curve). A side
    /// may be a point (a three-sided patch). A curve that is a chain of a surface's open edges may
    /// carry G1 or G2 with the face beside it. `operation` join sews the surface into the body of
    /// its first edge curve (a solid when it closes the gap there).
    SurfaceNetwork {
        u: Vec<NetworkCurve>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        v: Vec<NetworkCurve>,
        #[serde(default)]
        operation: Operation,
    },
    /// One face of a surface body rebuilt against another body (`surface.match`): the rebuilt
    /// boundary nearest the chain of `edges` on `target` takes that chain's place and repeats the
    /// target's own shape along the seam up to `order`, while the boundary opposite it, and the
    /// body's name, stay where they were. `faces` picks the face to rebuild (its first, when none).
    SurfaceMatch {
        body: BodyName,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        faces: Vec<FacePick>,
        target: BodyName,
        edges: Vec<Position>,
        #[serde(default = "match_order")]
        order: Continuity,
    },
    /// The B-spline faces at `faces` of `body` faired (`surface.fair`): each face's interior
    /// control vertices move to flatten the thin-plate (curvature-variation) energy of the face,
    /// `strength` of the full least-squares step at a time, for at most `iterations` steps.
    /// `keep_edges` holds the row of control points next to each edge as well as the boundary row
    /// itself, so the face keeps meeting its neighbours in position and in tangent. `tolerance`
    /// is the farthest a control point may move, so the faired face stays within it of the one the
    /// design asked for; an empty `faces` fairs every face of the body.
    SurfaceFair {
        body: BodyName,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        faces: Vec<FacePick>,
        #[serde(default = "fair_strength")]
        strength: f64,
        #[serde(default = "fair_iterations")]
        iterations: u32,
        #[serde(default = "fair_keep_edges")]
        keep_edges: bool,
        #[walk(length = "Tolerance")]
        tolerance: Expr,
    },
    /// A surface body trimmed by a plane, or by a face of a body (`tool`), keeping the side of
    /// `keep`.
    SurfaceTrim {
        body: BodyName,
        plane: PlaneRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<FaceAt>,
        keep: Position,
    },
    Move {
        bodies: Vec<BodyName>,
        #[walk(length = ["X distance", "Y distance", "Z distance"])]
        translate: [Expr; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rotate_axis: Option<Dir>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(angle = "Angle")]
        angle: Option<Expr>,
        /// When set, the original bodies are kept and the transform is applied to a new copy
        /// of each instead of moving them in place.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        copy: bool,
    },
    /// A non-parametric base feature: bodies imported from a STEP file. The STEP text is kept
    /// in the design (it is the B-rep), so the design reopens without the original file.
    Import {
        /// Source file name (display only).
        file: String,
        /// The STEP (ISO 10303-21) text, shared by every copy of the design (undo states,
        /// previews, scratch sessions): a large file is held once, not once per edit.
        step: std::sync::Arc<str>,
        /// The file's product/assembly tree, kept for components.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        components: Vec<solidcraft_kernel::StepNode>,
        /// The file's bodies (by index) this feature does not build: later instances of a part
        /// the file places more than once, shown by the other occurrences of that part's
        /// component (#1386). Empty in designs from before format 22: every body is built.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skip: Vec<u64>,
    },
    /// Rebuild an upstream constant-section mesh with analytic planar/cylindrical faces.
    MeshConvert {
        body: BodyName,
        #[walk(length = "Tolerance")]
        tolerance: Expr,
    },
    /// Mesh bodies imported from a 3MF or STL file, embedded in the design.
    MeshImport {
        /// Source file name (display only).
        file: String,
        meshes: Vec<MeshData>,
    },
}

/// What fills the corner where three blended edges meet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilletCorner {
    /// A piece of the rolling ball's sphere: tangent (G1) to each blend.
    #[default]
    Rolling,
    /// A G2 patch repeating each blend's surface up to the second order (see `corner_g2`).
    G2,
}

impl FilletCorner {
    pub fn is_rolling(&self) -> bool {
        matches!(self, FilletCorner::Rolling)
    }
}

/// How Extend continues a surface past its edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtendMode {
    /// The surface itself, continued.
    #[default]
    Natural,
    /// A ruled run-off in the surface's tangent plane (G1).
    Tangent,
    /// A run-off that keeps the surface's curvature across the edge (G2).
    Curvature,
}

impl ExtendMode {
    pub fn is_natural(&self) -> bool {
        *self == ExtendMode::Natural
    }
}

/// What Extend extends up to instead of a distance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
#[serde(rename_all = "snake_case")]
pub enum ExtendTo {
    /// A plane.
    Plane(PlaneRef),
    /// The surface of a body's face (extended).
    Face(FaceAt),
    /// The surface's own bounds (natural only): a trim undone.
    Bounds,
}

/// How a fillet's size is given.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FilletStyle {
    #[default]
    Constant,
    /// `radius` is the chord: the width across the blend.
    Chord,
    /// G2 quintic blend of planar faces along a straight edge. `radius` sets the
    /// equivalent circular setback, not a constant curvature radius.
    Curvature,
    /// `radius` at the end of each edge nearest `start`, `radius2` at the other; `mid` radii at
    /// positions between (0 to 1 from the start end); straight between them, or `smooth`.
    Variable {
        #[walk(length = "End radius")]
        radius2: Expr,
        start: Position,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[walk(length = "Mid radius")]
        mid: Vec<(f64, Expr)>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        smooth: bool,
    },
    /// The radius along each edge as a clamped B-spline of the position from the end nearest
    /// `start` (0) to the other (1): `radius` is its first control radius and `radii` the rest,
    /// its degree what `knots` leave (knots = control radii + degree + 1).
    Spline {
        start: Position,
        knots: Vec<f64>,
        #[walk(length = |i: usize, n: usize| if i + 1 == n { "End radius" } else { "Control radius" })]
        radii: Vec<Expr>,
    },
}

/// A fillet radius too large for the geometry around its edges: refused (`Exact`, the default),
/// reduced to the largest that works for all of them (`Reduce`), or tapered along each edge it
/// doesn't fit to the room there, down to zero where a face closes to a point (`Taper`); a
/// warning says what was done.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilletFit {
    #[default]
    Exact,
    Reduce,
    Taper,
}

impl FilletFit {
    pub fn is_exact(&self) -> bool {
        *self == FilletFit::Exact
    }
}

impl FilletStyle {
    pub fn is_constant(&self) -> bool {
        *self == FilletStyle::Constant
    }

    /// What the feature's `radius` is in this style (the label of its parameter).
    pub fn radius_label(&self) -> &'static str {
        match self {
            FilletStyle::Constant => "Radius",
            FilletStyle::Curvature => "Equivalent radius",
            FilletStyle::Chord => "Chord length",
            FilletStyle::Variable { .. } | FilletStyle::Spline { .. } => "Start radius",
        }
    }

    /// The spline of a [`FilletStyle::Spline`] with its control radii `values` (the first is
    /// the feature's `radius`).
    pub fn spline(knots: &[f64], values: Vec<f64>) -> solidcraft_kernel::RadiusSpline {
        let degree = knots.len().saturating_sub(values.len() + 1);
        solidcraft_kernel::RadiusSpline { degree, knots: knots.to_vec(), radii: values }
    }
}

/// A face picked by a point on it, on a named body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct FaceAt {
    pub body: BodyName,
    pub point: Position,
}

/// One end of a blend: a body and the points that pick the edges of its boundary chain. With two
/// picks on the same edge the chain is that one edge; more pick a chain that runs on from pick to
/// pick (the order of the picks does not matter).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct BlendSide {
    pub body: BodyName,
    pub edges: Vec<Position>,
}

/// One curve of a boundary surface's network ([`FeatureKind::SurfaceNetwork`]): a chain of a
/// sketch's `curves` (planar or 3D, meeting end to end), a chain of `body`'s edges picked by points
/// on them (`edges`), or a side collapsed to the point `point` (the apex of a three-sided patch).
/// A chain of a surface's open edges may carry `continuity` with the face beside it (G0, the
/// default, G1 or G2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct NetworkCurve {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sketch: Option<SketchId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Position>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<Position>,
    #[serde(default = "patch_g0", skip_serializing_if = "is_g0")]
    pub continuity: Continuity,
}

/// How a patch on a loop of edges meets the faces beside the loop: every edge with `all`, but the
/// edges picked in `edges` with their own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct PatchContinuity {
    #[serde(default = "patch_g0", skip_serializing_if = "is_g0")]
    pub all: Continuity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<EdgeContinuity>,
}

/// One edge of a patch's loop (a point on it) and how the patch meets the face beside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct EdgeContinuity {
    pub edge: Position,
    pub g: Continuity,
}

impl Default for PatchContinuity {
    fn default() -> PatchContinuity {
        PatchContinuity { all: Continuity::G0, edges: Vec::new() }
    }
}

impl PatchContinuity {
    /// G0 everywhere: the patch only meets its edges.
    pub fn is_g0(&self) -> bool {
        self.all == Continuity::G0 && self.edges.iter().all(|e| e.g == Continuity::G0)
    }
}

fn patch_g0() -> Continuity {
    Continuity::G0
}

fn is_g0(g: &Continuity) -> bool {
    *g == Continuity::G0
}

/// The continuity a blend repeats by default: the tangent plane on both sides.
/// The stations a boundary patch carries by default.
fn boundary_samples() -> usize {
    BOUNDARY_SAMPLES
}

/// The stations a boundary patch carries when a design does not say.
pub const BOUNDARY_SAMPLES: usize = 25;

fn blend_orders() -> [Continuity; 2] {
    [Continuity::G1, Continuity::G1]
}

/// The continuity a match holds by default: position, tangent plane and curvature.
fn match_order() -> Continuity {
    Continuity::G2
}

/// A fairing's defaults for a design that leaves them out: the whole least-squares step at a
/// time, and the boundary held (the row next to each edge too, so the face still meets its
/// neighbours in tangent).
fn fair_strength() -> f64 {
    1.0
}
fn fair_iterations() -> u32 {
    1
}
fn fair_keep_edges() -> bool {
    true
}

/// Triangles of one imported mesh body (millimetres).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshData {
    pub name: String,
    /// x, y, z per vertex.
    pub positions: Vec<f64>,
    /// Three vertex indices per triangle.
    pub triangles: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 3]>,
}

/// Points of a sketch, by id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct SketchPoints {
    pub sketch: SketchId,
    pub ids: Vec<String>,
    /// Drill the other way: away from the side the holes would go into by themselves.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip: bool,
}

/// A component: a node of the design tree that owns sketches, features and bodies. The root
/// (id 0) is the design itself and is not stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub id: u64,
    pub name: String,
    /// Parent component (0 = the root).
    #[serde(default)]
    pub parent: u64,
}

/// One loft section: profiles of a sketch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct LoftSection {
    pub sketch: SketchId,
    #[walk(profiles_of = sketch, when = point.is_none())]
    pub profiles: ProfileSel,
    /// A sketch point instead of a profile (the loft closes to it, as a pyramid's apex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<String>,
}

/// A section of a surface loft: one chain from a sketch (one profile's boundary, or curves
/// meeting end to end).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct SheetSection {
    pub sketch: SketchId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[walk(profiles_of = sketch)]
    pub profiles: Option<ProfileSel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<String>,
    /// A section picked as an edge chain of a body instead of a sketch: `edges` are points on the
    /// chain, and the face the chain bounds is the surface the loft leaves there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Position>,
}

/// A loft's continuity where a design does not say: both ends G0, the loft of old, which only
/// passes through its sections.
fn loft_plain_continuity() -> [Continuity; 2] {
    [Continuity::G0, Continuity::G0]
}

/// A loft's continuity is plain when it only passes through its sections: both ends G0.
fn loft_continuity_is_plain(c: &[Continuity; 2]) -> bool {
    *c == [Continuity::G0, Continuity::G0]
}

/// A loft's crown is plain when neither end is filled out.
fn loft_crown_is_plain(c: &[f64; 2]) -> bool {
    c.iter().all(|v| *v == 0.0)
}

/// Coil cross-sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoilSection {
    #[default]
    Circular,
    Square,
}

fn is_zero_i8(v: &i8) -> bool {
    *v == 0
}

/// A parameter that is left out when it is 0 (an optional shape knob).
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Hole shapes.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, solidcraft_derive::Walk)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HoleKind {
    /// Plain hole; blind holes get a drill point when `tip_angle` is set.
    #[default]
    Simple,
    Drilled {
        #[walk(angle = "Tip angle")]
        tip_angle: Expr,
    },
    Counterbore {
        #[walk(length = "Counterbore diameter")]
        cb_diameter: Expr,
        #[walk(length = "Counterbore depth")]
        cb_depth: Expr,
    },
    Countersink {
        #[walk(length = "Countersink diameter")]
        cs_diameter: Expr,
        #[walk(angle = "Countersink angle")]
        cs_angle: Expr,
    },
}

/// Pattern layouts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PatternKind {
    Rectangular {
        dir1: Dir,
        #[walk(unitless = "Quantity")]
        count1: Expr,
        #[walk(length = "Spacing")]
        spacing1: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dir2: Option<Dir>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(unitless = "Quantity 2")]
        count2: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[walk(length = "Spacing 2")]
        spacing2: Option<Expr>,
        #[serde(default, skip_serializing_if = "default_pattern_alignment")]
        alignment: solidcraft_sketch::PatternAlignment,
        #[serde(default, skip_serializing_if = "default_pattern_alignment")]
        alignment2: solidcraft_sketch::PatternAlignment,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reversed: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reversed2: bool,
    },
    Circular {
        origin: Position,
        axis: Dir,
        #[walk(unitless = "Quantity")]
        count: Expr,
        /// Total angle (360 deg = evenly around).
        #[walk(angle = "Total angle")]
        angle: Expr,
        #[serde(default, skip_serializing_if = "default_pattern_alignment")]
        alignment: solidcraft_sketch::PatternAlignment,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reversed: bool,
    },
    /// Copies along a chain of sketch curves: instance k moves by path(k·spacing) − path(0),
    /// measured along the path from its start.
    Path {
        path_sketch: SketchId,
        path: Vec<String>,
        #[walk(unitless = "Quantity")]
        count: Expr,
        /// Distance along the path between instances, or the whole extent when `extent`.
        #[walk(length = "Distance")]
        spacing: Expr,
        /// `spacing` is the distance from the first to the last instance.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        extent: bool,
        /// Run from the path's other end.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
        /// Turn copies with the path's direction (else they keep their orientation).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        orient: bool,
    },
}

fn default_pattern_alignment(alignment: &solidcraft_sketch::PatternAlignment) -> bool {
    *alignment == solidcraft_sketch::PatternAlignment::Right
}

fn z_axis() -> Dir {
    Dir(Vec3::Z)
}

/// A point whose coordinates are length expressions (`["basin_width / 2", "10 mm", "0"]`), so
/// it follows parameter changes. Primitives are placed by one.
pub type PointExpr = [Expr; 3];

/// A point as expressions (plain numbers, mm).
pub fn point_expr(p: Vec3) -> PointExpr {
    [Expr::new(format!("{}", p.x)), Expr::new(format!("{}", p.y)), Expr::new(format!("{}", p.z))]
}

/// Stored as `[x, y, z]`: plain numbers as numbers (as points were before they took
/// expressions, so such designs read and write the same), anything else as a string.
mod point_expr {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum Coord {
        Num(f64),
        Expr(String),
    }

    pub fn serialize<S: Serializer>(p: &super::PointExpr, s: S) -> Result<S::Ok, S::Error> {
        let c = |e: &crate::Expr| match e.parse::<f64>() {
            Ok(v) if v.is_finite() && format!("{v}") == e.as_str() => Coord::Num(v),
            _ => Coord::Expr(e.to_string()),
        };
        [c(&p[0]), c(&p[1]), c(&p[2])].serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<super::PointExpr, D::Error> {
        let c = |x: Coord| match x {
            Coord::Num(v) => crate::Expr::new(format!("{v}")),
            Coord::Expr(e) => crate::Expr::new(e),
        };
        let [x, y, z] = <[Coord; 3]>::deserialize(d)?;
        Ok([c(x), c(y), c(z)])
    }
}

/// [`point_expr`] for an optional point.
mod opt_point_expr {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Wrap(#[serde(with = "super::point_expr")] super::PointExpr);

    pub fn serialize<S: Serializer>(p: &Option<super::PointExpr>, s: S) -> Result<S::Ok, S::Error> {
        p.as_ref().map(|p| Wrap(p.clone())).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<super::PointExpr>, D::Error> {
        Ok(Option::<Wrap>::deserialize(d)?.map(|w| w.0))
    }
}

impl FeatureKind {
    /// A sheet metal feature (its result depends on the sheet metal rules).
    pub fn is_plastic(&self) -> bool {
        matches!(self, FeatureKind::Boss { .. } | FeatureKind::Lip { .. } | FeatureKind::SnapFit { .. } | FeatureKind::Rest { .. })
    }

    pub fn is_sheet(&self) -> bool {
        matches!(
            self,
            FeatureKind::SheetBase { .. }
                | FeatureKind::SheetContour { .. }
                | FeatureKind::SheetFlange { .. }
                | FeatureKind::SheetHem { .. }
                | FeatureKind::SheetPerforate { .. }
                | FeatureKind::SheetFold { .. }
                | FeatureKind::SheetUnfold { .. }
                | FeatureKind::SheetConvert { .. }
        )
    }

    /// Fusion-like type name (timeline tooltip, `inspect`).
    pub fn type_name(&self) -> &'static str {
        match self {
            FeatureKind::BodyTransfer { .. } => "BodyTransferFeature",
            FeatureKind::Sketch { .. } => "Sketch",
            FeatureKind::Extrude { .. } => "ExtrudeFeature",
            FeatureKind::Revolve { .. } => "RevolveFeature",
            FeatureKind::Fillet { .. } => "FilletFeature",
            FeatureKind::Chamfer { .. } => "ChamferFeature",
            FeatureKind::Box { .. } => "BoxFeature",
            FeatureKind::Cylinder { .. } => "CylinderFeature",
            FeatureKind::Sphere { .. } => "SphereFeature",
            FeatureKind::Torus { .. } => "TorusFeature",
            FeatureKind::Combine { .. } => "CombineFeature",
            FeatureKind::Pattern { pattern: PatternKind::Rectangular { .. }, .. } => "RectangularPatternFeature",
            FeatureKind::Pattern { pattern: PatternKind::Path { .. }, .. } => "PathPatternFeature",
            FeatureKind::Pattern { .. } => "CircularPatternFeature",
            FeatureKind::Mirror { .. } => "MirrorFeature",
            FeatureKind::ConstructionPlane { .. } => "ConstructionPlane",
            FeatureKind::Hole { .. } => "HoleFeature",
            FeatureKind::Thread { .. } => "ThreadFeature",
            FeatureKind::Loft { .. } => "LoftFeature",
            FeatureKind::Sweep { .. } => "SweepFeature",
            FeatureKind::Shell { .. } => "ShellFeature",
            FeatureKind::Draft { .. } => "DraftFeature",
            FeatureKind::Split { .. } => "SplitBodyFeature",
            FeatureKind::SplitFace { .. } => "SplitFaceFeature",
            FeatureKind::ConstructionAxis { .. } => "ConstructionAxis",
            FeatureKind::ConstructionPoint { .. } => "ConstructionPoint",
            FeatureKind::Move { .. } => "MoveFeature",
            FeatureKind::Scale { .. } => "ScaleFeature",
            FeatureKind::OffsetFace { .. } => "OffsetFacesFeature",
            FeatureKind::DeleteFace { .. } => "DeleteFaceFeature",
            FeatureKind::SurfaceOffset { .. } => "OffsetSurfaceFeature",
            FeatureKind::BoundingSolid { .. } => "BoundingSolidFeature",
            FeatureKind::Pipe { .. } => "PipeFeature",
            FeatureKind::Emboss { .. } => "EmbossFeature",
            FeatureKind::Boss { .. } => "BossFeature",
            FeatureKind::Lip { groove: false, .. } => "LipFeature",
            FeatureKind::Lip { .. } => "GrooveFeature",
            FeatureKind::SnapFit { .. } => "SnapFitFeature",
            FeatureKind::Rest { .. } => "RestFeature",
            FeatureKind::BoundaryFill { .. } => "BoundaryFillFeature",
            FeatureKind::SheetBase { .. } => "BaseFlangeFeature",
            FeatureKind::SheetContour { .. } => "ContourFlangeFeature",
            FeatureKind::SheetFlange { .. } => "EdgeFlangeFeature",
            FeatureKind::SheetHem { .. } => "HemFeature",
            FeatureKind::SheetPerforate { .. } => "PerforateFeature",
            FeatureKind::SheetFold { .. } => "FoldFeature",
            FeatureKind::SheetUnfold { refold: false, .. } => "UnfoldFeature",
            FeatureKind::SheetUnfold { .. } => "RefoldFeature",
            FeatureKind::SheetConvert { .. } => "ConvertToSheetMetalFeature",
            FeatureKind::Coil { .. } => "CoilFeature",
            FeatureKind::Rib { web: false, .. } => "RibFeature",
            FeatureKind::Rib { .. } => "WebFeature",
            FeatureKind::ReplaceFace { .. } => "ReplaceFaceFeature",
            FeatureKind::Align { .. } => "AlignFeature",
            FeatureKind::Remove { .. } => "RemoveFeature",
            FeatureKind::Patch { .. } => "PatchFeature",
            FeatureKind::SurfaceExtrude { .. } => "SurfaceExtrudeFeature",
            FeatureKind::SurfaceRevolve { .. } => "SurfaceRevolveFeature",
            FeatureKind::SurfaceSweep { .. } => "SurfaceSweepFeature",
            FeatureKind::SurfaceLoft { .. } => "SurfaceLoftFeature",
            FeatureKind::SurfaceBlend { .. } => "BlendFeature",
            FeatureKind::SurfaceBoundary { .. } | FeatureKind::SurfaceNetwork { .. } => "BoundaryFeature",
            FeatureKind::SurfaceMatch { .. } => "MatchFeature",
            FeatureKind::SurfaceFair { .. } => "FairFeature",
            FeatureKind::Stitch { .. } => "StitchFeature",
            FeatureKind::Thicken { .. } => "ThickenFeature",
            FeatureKind::SurfaceTrim { .. } => "TrimFeature",
            FeatureKind::SurfaceExtend { .. } => "ExtendFeature",
            FeatureKind::SurfaceRuled { .. } => "RuledSurfaceFeature",
            FeatureKind::Unstitch { .. } => "UnstitchFeature",
            FeatureKind::ReverseNormal { .. } => "ReverseNormalFeature",
            FeatureKind::SurfaceUntrim { .. } => "UntrimFeature",
            FeatureKind::Import { .. } => "BaseFeature",
            FeatureKind::MeshImport { .. } => "MeshFeature",
            FeatureKind::MeshConvert { .. } => "MeshConvertFeature",
        }
    }
    /// Default name prefix (`Extrude` → `Extrude1`).
    pub fn base_name(&self) -> &'static str {
        match self {
            FeatureKind::BodyTransfer { .. } => "BodyTransfer",
            FeatureKind::Sketch { .. } => "Sketch",
            FeatureKind::Extrude { .. } => "Extrude",
            FeatureKind::Revolve { .. } => "Revolve",
            FeatureKind::Fillet { .. } => "Fillet",
            FeatureKind::Chamfer { .. } => "Chamfer",
            FeatureKind::Box { .. } => "Box",
            FeatureKind::Cylinder { .. } => "Cylinder",
            FeatureKind::Sphere { .. } => "Sphere",
            FeatureKind::Torus { .. } => "Torus",
            FeatureKind::Combine { .. } => "Combine",
            FeatureKind::Pattern { pattern: PatternKind::Rectangular { .. }, .. } => "RectangularPattern",
            FeatureKind::Pattern { pattern: PatternKind::Path { .. }, .. } => "PathPattern",
            FeatureKind::Pattern { .. } => "CircularPattern",
            FeatureKind::Mirror { .. } => "Mirror",
            FeatureKind::ConstructionPlane { .. } => "Plane",
            FeatureKind::Hole { .. } => "Hole",
            FeatureKind::Thread { .. } => "Thread",
            FeatureKind::Loft { .. } => "Loft",
            FeatureKind::Sweep { .. } => "Sweep",
            FeatureKind::Shell { .. } => "Shell",
            FeatureKind::Draft { .. } => "Draft",
            FeatureKind::Split { .. } => "Split",
            FeatureKind::SplitFace { .. } => "SplitFace",
            FeatureKind::ConstructionAxis { .. } => "Axis",
            FeatureKind::ConstructionPoint { .. } => "Point",
            FeatureKind::Move { .. } => "Move",
            FeatureKind::Scale { .. } => "Scale",
            FeatureKind::OffsetFace { .. } => "OffsetFace",
            FeatureKind::DeleteFace { .. } => "DeleteFace",
            FeatureKind::SurfaceOffset { .. } => "OffsetSurface",
            FeatureKind::BoundingSolid { .. } => "BoundingSolid",
            FeatureKind::Pipe { .. } => "Pipe",
            FeatureKind::Emboss { .. } => "Emboss",
            FeatureKind::Boss { .. } => "Boss",
            FeatureKind::Lip { groove: false, .. } => "Lip",
            FeatureKind::Lip { .. } => "Groove",
            FeatureKind::SnapFit { .. } => "SnapFit",
            FeatureKind::Rest { .. } => "Rest",
            FeatureKind::BoundaryFill { .. } => "BoundaryFill",
            FeatureKind::SheetBase { .. } => "BaseFlange",
            FeatureKind::SheetContour { .. } => "ContourFlange",
            FeatureKind::SheetFlange { .. } => "EdgeFlange",
            FeatureKind::SheetHem { .. } => "Hem",
            FeatureKind::SheetPerforate { .. } => "Perforate",
            FeatureKind::SheetFold { .. } => "Fold",
            FeatureKind::SheetUnfold { refold: false, .. } => "Unfold",
            FeatureKind::SheetUnfold { .. } => "Refold",
            FeatureKind::SheetConvert { .. } => "ConvertToSheetMetal",
            FeatureKind::Coil { .. } => "Coil",
            FeatureKind::Rib { web: false, .. } => "Rib",
            FeatureKind::Rib { .. } => "Web",
            FeatureKind::ReplaceFace { .. } => "ReplaceFace",
            FeatureKind::Align { .. } => "Align",
            FeatureKind::Remove { .. } => "Remove",
            FeatureKind::Patch { .. } => "Patch",
            FeatureKind::SurfaceExtrude { .. } => "SurfaceExtrude",
            FeatureKind::SurfaceRevolve { .. } => "SurfaceRevolve",
            FeatureKind::SurfaceSweep { .. } => "SurfaceSweep",
            FeatureKind::SurfaceLoft { .. } => "SurfaceLoft",
            FeatureKind::SurfaceBlend { .. } => "Blend",
            FeatureKind::SurfaceBoundary { .. } | FeatureKind::SurfaceNetwork { .. } => "Boundary",
            FeatureKind::SurfaceMatch { .. } => "Match",
            FeatureKind::SurfaceFair { .. } => "Fair",
            FeatureKind::Stitch { .. } => "Stitch",
            FeatureKind::Thicken { .. } => "Thicken",
            FeatureKind::SurfaceTrim { .. } => "Trim",
            FeatureKind::SurfaceExtend { .. } => "Extend",
            FeatureKind::SurfaceRuled { .. } => "RuledSurface",
            FeatureKind::Unstitch { .. } => "Unstitch",
            FeatureKind::ReverseNormal { .. } => "ReverseNormal",
            FeatureKind::SurfaceUntrim { .. } => "Untrim",
            FeatureKind::Import { .. } => "Import",
            FeatureKind::MeshImport { .. } => "Mesh",
            FeatureKind::MeshConvert { .. } => "ConvertMesh",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub suppressed: bool,
    /// Names for the bodies this feature creates (in order); missing ones are numbered after the
    /// first (`Ribs (4)`), or get `BodyN` when there is none (or see `shell_piece_order`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_names: Vec<String>,
    /// Parameter names of the feature's inputs (see `FeatureKind::inputs`), e.g. `d3`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_names: Vec<String>,
    /// The component the feature (and what it makes) belongs to; 0 = the root.
    #[serde(default, skip_serializing_if = "is_root")]
    pub component: u64,
    /// For each picked edge (fillets, chamfers): where it sat in its body when picked, so it
    /// is found again when earlier edits move or resize the body.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edge_refs: Vec<crate::EdgeRef>,
    /// Persistent names of the picked edges (resolved first; see `naming`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edge_names: Vec<String>,
    /// Persistent names of the faces at the feature's face points (`FeatureKind::face_points`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub face_names: Vec<String>,
    /// Bodies a join/cut/intersect may change (any component); empty = the feature's own
    /// component's bodies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,
    /// The pieces this feature splits a body into take its body names in the kernel's shell order,
    /// as builds before format 14 named them, instead of after their faces (`lineage::order_lumps`).
    /// Set on the features of designs saved before then, whose stored body names mean that order.
    /// Such features also name the bodies past their `body_names` `BodyN`, as those builds did,
    /// where newer ones number them after the first name (`Ribs (4)`, #373).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shell_piece_order: bool,
    #[serde(flatten)]
    pub kind: FeatureKind,
}

fn inside_str() -> String {
    "inside".into()
}

fn all_profiles() -> ProfileSel {
    ProfileSel::All
}

fn yes() -> bool {
    true
}

fn is_yes(b: &bool) -> bool {
    *b
}

fn zero_expr() -> Expr {
    "0".into()
}

fn is_root(c: &u64) -> bool {
    *c == 0
}

/// A standard part (fastener, pin, bearing) inserted as a component.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StandardPart {
    pub family: String,
    pub size: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
}

/// A component whose contents come from another design file (Insert Design): where that file
/// is, and the Import feature in the component that holds its bodies (Update Links replaces
/// them; Break Link keeps them and forgets the file).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DesignLink {
    /// The design file: relative to this design's folder once this design is saved, else as
    /// given (`/` separates folders).
    pub path: String,
    /// The Import feature holding the linked design's bodies.
    pub feature: u64,
}

/// The document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Named, renderer-independent setups. Device/job state is never saved here.
    #[serde(default, skip_serializing_if = "crate::render_settings::RenderSettings::is_empty")]
    pub render: std::sync::Arc<crate::render_settings::RenderSettings>,
    #[serde(default = "default_format")]
    pub format: String,
    /// The changes of `format` this design has while they are pending (no release has fixed
    /// that number yet; `crate::format::pending_names`). Empty, and not saved, otherwise.
    #[serde(default = "crate::format::pending_names", skip_serializing_if = "Vec::is_empty")]
    pub format_pending: Vec<String>,
    pub name: String,
    #[serde(default = "default_units")]
    pub units: String,
    /// Design properties (project, author…) and other tools' data, kept as they are (#1015).
    #[serde(default, skip_serializing_if = "crate::Metadata::is_empty")]
    pub metadata: crate::Metadata,
    pub params: Vec<Param>,
    pub features: Vec<Feature>,
    /// Timeline marker: features at or after this index are rolled back (not evaluated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<usize>,
    #[serde(default)]
    pub next_id: u64,
    /// Components below the root, as a tree (by parent).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    /// Placements of components in their parents (each non-root component has at least one).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub occurrences: Vec<crate::Occurrence>,
    /// Joints between occurrences, joint origins and motion links.
    #[serde(default, skip_serializing_if = "crate::joints::Assembly::is_empty")]
    pub assembly: crate::joints::Assembly,
    /// Sheet metal rules.
    #[serde(default, skip_serializing_if = "crate::sheet::SheetSettings::is_empty")]
    pub sheet: crate::sheet::SheetSettings,
    /// Plastic rules.
    #[serde(default, skip_serializing_if = "crate::plastic::PlasticSettings::is_empty")]
    pub plastic: crate::plastic::PlasticSettings,
    /// Components that are standard parts (by component id): what to rebuild them from.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub parts: std::collections::BTreeMap<u64, StandardPart>,
    /// Components linked to another design file (by component id).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub links: std::collections::BTreeMap<u64, DesignLink>,
    /// Configurations (rows of parameter values and suppressions).
    #[serde(default, skip_serializing_if = "crate::config::ConfigTable::is_empty")]
    pub configs: crate::config::ConfigTable,
    /// Favourite parameters (by name).
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub favorites: std::collections::BTreeSet<String>,
    /// Reference images on planes (Insert Canvas).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub canvases: Vec<crate::canvas::Canvas>,
    /// Comments of feature-input parameters (by name).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub param_comments: std::collections::BTreeMap<String, String>,
    /// Physical material per body (by body name); others use the default.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub materials: std::collections::BTreeMap<String, String>,
    /// Physical materials defined in this design (`material.create`), next to the built-in ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_materials: Vec<UserMaterial>,
    /// Appearance colour per body (by body name, sRGB); it overrides the material's look.
    #[serde(default, skip_serializing_if = "crate::appearance::Appearances::is_empty")]
    pub appearances: crate::appearance::Appearances,
    /// Bodies moved into another component than their feature's (by body name).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub body_components: std::collections::BTreeMap<String, u64>,
    /// Bodies moved into another component keep their place in the world: the transform from
    /// the new component's frame to the frame the body was made in (by body name).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub body_offsets: std::collections::BTreeMap<String, crate::Mat>,
    /// Browser groups (folders of bodies, sketches or construction planes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub browser_groups: Vec<BrowserGroup>,
    /// Named folders used to organise features in the timeline. They do not affect evaluation
    /// order; feature ids remain the source of truth when features are renamed or reordered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timeline_groups: Vec<TimelineGroup>,
    /// Browser item order per component folder (`"<component>/<folder>"` → item keys).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub browser_order: std::collections::BTreeMap<String, Vec<String>>,
    /// Named views: saved cameras.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub named_views: Vec<NamedView>,
    /// What was hidden when the design was saved. A session takes it from here when the design
    /// opens and keeps the live state itself (`browser.visibility`: saved, not an undo step), so
    /// a document in a session leaves this empty; the session puts it back when it saves.
    #[serde(default, skip_serializing_if = "Visibility::is_empty")]
    pub visibility: Visibility,
}

/// Items hidden or shown one by one. Saved with the design, but not an undo step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Visibility {
    /// Bodies by name.
    pub hidden_bodies: Vec<String>,
    /// Exact occurrence placements hidden with all descendants.
    pub hidden_occurrences: Vec<Vec<u64>>,
    /// Origin items (O, X, Y, Z, XY, XZ, YZ) and construction geometry, by name.
    pub hidden_origin: Vec<String>,
    /// Sketches by feature id.
    pub hidden_sketches: Vec<u64>,
    /// Finished sketches shown although a feature uses them.
    pub shown_sketches: Vec<u64>,
    /// Assembly joints by id: their glyphs are not drawn (they still constrain and move).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_joints: Vec<u64>,
}

impl Visibility {
    /// Nothing hidden or shown one by one (the default view).
    pub fn is_empty(&self) -> bool {
        self.hidden_bodies.is_empty()
            && self.hidden_occurrences.is_empty()
            && self.hidden_origin.is_empty()
            && self.hidden_sketches.is_empty()
            && self.shown_sketches.is_empty()
            && self.hidden_joints.is_empty()
    }
}

/// A saved camera (orbit camera: target, yaw, pitch, distance; `fov` 0 for orthographic).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NamedView {
    #[serde(default)]
    pub name: String,
    pub target: solidcraft_geom::Vec3,
    pub yaw: f64,
    pub pitch: f64,
    pub distance: f64,
    #[serde(default)]
    pub fov: f64,
}

/// A browser group: items of one folder (`bodies`, `sketches` or `construction`) of a
/// component, by key (body name, or the feature id of a sketch or construction plane).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrowserGroup {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub component: u64,
    pub folder: String,
    #[serde(default)]
    pub items: Vec<String>,
}

/// A saved feature-history folder. Folders may nest, and a feature can belong to at most one folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TimelineGroup {
    pub id: u64,
    pub name: String,
    /// The containing feature folder. `None` places the folder at the timeline root.
    #[serde(default)]
    pub parent: Option<u64>,
    #[serde(default)]
    pub features: Vec<u64>,
}

/// The first feature-history folder whose own and nested features do not form one
/// uninterrupted run in the timeline.
pub fn non_contiguous_timeline_group(features: &[Feature], groups: &[TimelineGroup]) -> Option<u64> {
    let positions: std::collections::BTreeMap<u64, usize> = features.iter().enumerate().map(|(index, feature)| (feature.id, index)).collect();
    let mut stats = std::collections::BTreeMap::new();
    let mut children = std::collections::BTreeMap::new();
    for group in groups {
        let mut count = 0usize;
        let mut first = usize::MAX;
        let mut last = 0usize;
        for index in group.features.iter().filter_map(|feature| positions.get(feature)) {
            count = count.saturating_add(1);
            first = first.min(*index);
            last = last.max(*index);
        }
        stats.insert(group.id, (count, first, last));
        children.entry(group.id).or_insert(0usize);
        if let Some(parent) = group.parent {
            let child_count = children.entry(parent).or_insert(0);
            *child_count = child_count.saturating_add(1);
        }
    }
    let parents: std::collections::BTreeMap<u64, Option<u64>> = groups.iter().map(|group| (group.id, group.parent)).collect();
    let mut pending: Vec<u64> = groups.iter().filter(|group| children.get(&group.id) == Some(&0)).map(|group| group.id).collect();
    while let Some(id) = pending.pop() {
        let (count, first, last) = stats.get(&id).copied().unwrap_or_default();
        if count > 0 && last.saturating_sub(first).saturating_add(1) != count {
            return Some(id);
        }
        if let Some(parent) = parents.get(&id).copied().flatten() {
            if let Some((parent_count, parent_first, parent_last)) = stats.get_mut(&parent) {
                *parent_count = parent_count.saturating_add(count);
                if count > 0 {
                    *parent_first = (*parent_first).min(first);
                    *parent_last = (*parent_last).max(last);
                }
            }
            if let Some(remaining) = children.get_mut(&parent) {
                *remaining = remaining.saturating_sub(1);
                if *remaining == 0 {
                    pending.push(parent);
                }
            }
        }
    }
    None
}

fn default_format() -> String {
    crate::format::format_string(crate::format::FORMAT)
}
fn default_units() -> String {
    "mm".into()
}

impl Default for Document {
    fn default() -> Self {
        Document::new("Untitled")
    }
}

impl Document {
    pub fn new(name: &str) -> Self {
        Document {
            render: Default::default(),
            format: default_format(),
            format_pending: crate::format::pending_names(),
            name: name.into(),
            units: default_units(),
            metadata: Default::default(),
            params: Vec::new(),
            features: Vec::new(),
            marker: None,
            next_id: 1,
            components: Vec::new(),
            occurrences: Vec::new(),
            assembly: Default::default(),
            sheet: Default::default(),
            plastic: Default::default(),
            configs: Default::default(),
            parts: Default::default(),
            links: Default::default(),
            body_components: Default::default(),
            body_offsets: Default::default(),
            materials: Default::default(),
            user_materials: Vec::new(),
            appearances: Default::default(),
            favorites: Default::default(),
            canvases: Vec::new(),
            param_comments: Default::default(),
            browser_groups: Vec::new(),
            timeline_groups: Vec::new(),
            browser_order: Default::default(),
            named_views: Vec::new(),
            visibility: Visibility::default(),
        }
    }

    /// A complete structural proof: no non-render field is allowed to differ. Keeping this
    /// comparison based on the whole Document also includes fields added in future versions.
    /// The clone/comparison is only needed when render intent actually differs.
    pub fn differs_only_in_render(&self, previous: &Document) -> bool {
        if std::sync::Arc::ptr_eq(&self.render, &previous.render) || self.render == previous.render || self.render.validate().is_err() {
            return false;
        }
        let mut normalized = previous.clone();
        normalized.render = self.render.clone();
        *self == normalized
    }

    /// The colour a body is shown and exported in (sRGB as 0–1 fractions): its appearance,
    /// else its component's, else its material's look, else the colour it was imported with.
    /// (Faces may have their own: `face_looks`.)
    pub fn body_color(&self, b: &crate::ModelBody) -> Option<[f32; 3]> {
        self.model_body_look(b).map(|l| l.rgb()).or_else(|| b.body.color()).or_else(|| b.imported.as_ref().and_then(|i| i.color))
    }

    /// The model with each body carrying the colour, opacity and face colours it is shown with
    /// (for export).
    pub fn painted(&self, state: &crate::ModelState) -> crate::ModelState {
        let mut st = state.clone();
        for b in &mut st.bodies {
            let c = self.body_color(b);
            let faces: Vec<solidcraft_kernel::FacePaint> = self
                .face_colors(b)
                .into_iter()
                .map(|(face, l)| solidcraft_kernel::FacePaint { face, color: l.rgb(), opacity: l.opacity as f32 })
                .collect();
            let opacity = self.model_body_opacity(b) as f32;
            let paint = solidcraft_kernel::Paint { opacity, faces };
            if c != b.body.color() || b.body.paint() != Some(&paint) {
                b.body = b.body.clone().with_color(c).with_paint(Some(paint));
            }
        }
        st
    }

    /// Density of a body's material (g/cm³).
    pub fn density(&self, body: &str) -> f64 {
        self.materials.get(body).and_then(|m| self.material_density(m)).unwrap_or(DEFAULT_DENSITY)
    }

    /// A material defined in this design, by name (case-insensitive).
    pub fn user_material(&self, name: &str) -> Option<&UserMaterial> {
        let name = name.trim();
        self.user_materials.iter().find(|m| m.name.eq_ignore_ascii_case(name))
    }

    /// A physical material's name as the library or this design spells it (case-insensitive
    /// lookup): built-in materials first, then the design's own.
    pub fn material_name(&self, name: &str) -> Option<String> {
        MATERIALS
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
            .map(|(n, _)| n.to_string())
            .or_else(|| self.user_material(name).map(|m| m.name.clone()))
    }

    /// Density of a physical material (g/cm³): built-in or defined in this design. A design
    /// material with an unusable density (a damaged file) has none.
    pub fn material_density(&self, name: &str) -> Option<f64> {
        material_density(name).or_else(|| self.user_material(name).map(|m| m.density).filter(|d| valid_density(*d)))
    }

    /// The look a physical material gives a body without an appearance of its own.
    pub fn material_look(&self, name: &str) -> Option<crate::appearance::Look> {
        crate::appearance::material_look(name).or_else(|| {
            let mut look = self.user_material(name)?.appearance.clone()?;
            // A damaged file's opacity is shown opaque rather than trusted.
            if !(look.opacity.is_finite() && (0.0..=1.0).contains(&look.opacity)) {
                look.opacity = 1.0;
            }
            Some(look)
        })
    }

    /// Every physical material's name: the built-in library, then the design's own.
    pub fn material_names(&self) -> Vec<String> {
        MATERIALS.iter().map(|(n, _)| n.to_string()).chain(self.user_materials.iter().map(|m| m.name.clone())).collect()
    }

    /// The component a body belongs to: where it was moved, else its feature's.
    pub fn body_component(&self, body: &str, feature: u64) -> u64 {
        self.features
            .iter()
            .take(self.marker.unwrap_or(usize::MAX))
            .rev()
            .find_map(|f| {
                if f.suppressed {
                    return None;
                }
                match &f.kind {
                    FeatureKind::BodyTransfer { bodies, to, .. } if bodies.iter().any(|b| b == body) => Some(*to),
                    _ => None,
                }
            })
            .unwrap_or_else(|| self.legacy_body_component(body, feature))
    }

    pub fn legacy_body_component(&self, body: &str, feature: u64) -> u64 {
        self.body_components.get(body).copied().unwrap_or_else(|| self.feature(feature).map(|f| f.component).unwrap_or(0))
    }

    /// Ownership in this evaluated timeline state, rather than a later transfer.
    pub fn model_body_component(&self, body: &crate::ModelBody) -> u64 {
        if body.transferred || body.source_name.is_some() { body.component } else { self.legacy_body_component(&body.name, body.feature) }
    }

    /// Recorded transfers already converted geometry into the new local frame.
    pub fn model_body_offset(&self, body: &crate::ModelBody) -> Option<&crate::Mat> {
        if body.transferred { None } else { self.body_offsets.get(&body.name) }
    }

    /// Add a component under `parent` and return its id.
    pub fn add_component(&mut self, name: Option<&str>, parent: u64) -> Result<u64> {
        if parent != 0 && !self.components.iter().any(|c| c.id == parent) {
            return Err(DocError::Unknown(format!("component {parent}")));
        }
        if self.components.len() >= 10_000 {
            return Err(DocError::Invalid("too many components".into()));
        }
        let id = self.next_id.max(1);
        self.next_id = id.checked_add(1).ok_or_else(|| DocError::Invalid("too many items in the design".into()))?;
        let name = match name.map(str::trim) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => {
                let mut k = self.components.len() + 1;
                while self.components.iter().any(|c| c.name == format!("Component{k}")) {
                    k += 1;
                }
                format!("Component{k}")
            }
        };
        self.components.push(Component { id, name, parent });
        self.add_occurrence(id, parent, crate::IDENTITY)?;
        Ok(id)
    }

    /// A component by id or name (0 / "root" is the design itself).
    pub fn find_component(&self, key: &str) -> Option<u64> {
        if key == "0" || key.eq_ignore_ascii_case("root") || key == self.name {
            return Some(0);
        }
        self.components.iter().find(|c| c.id.to_string() == key || c.name == key).map(|c| c.id)
    }

    /// Read a design file's JSON (any format this build knows; older ones are upgraded).
    pub fn from_json(s: &str) -> Result<Document> {
        let d = crate::format::read(s)?;
        if d.features.len() > MAX_FEATURES || d.params.len() > MAX_PARAMS {
            return Err(DocError::Invalid("document too large".into()));
        }
        Ok(d)
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn feature(&self, id: u64) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }
    pub fn feature_mut(&mut self, id: u64) -> Option<&mut Feature> {
        self.features.iter_mut().find(|f| f.id == id)
    }
    pub fn feature_index(&self, id: u64) -> Option<usize> {
        self.features.iter().position(|f| f.id == id)
    }
    /// Find a feature by id (number) or name.
    pub fn find_feature(&self, r: &str) -> Option<&Feature> {
        if let Ok(id) = r.parse::<u64>()
            && let Some(f) = self.feature(id)
        {
            return Some(f);
        }
        self.features.iter().find(|f| f.name.eq_ignore_ascii_case(r))
    }

    fn unique_name(&self, base: &str) -> String {
        let mut n = 1;
        loop {
            let name = format!("{base}{n}");
            if !self.features.iter().any(|f| f.name == name) {
                return name;
            }
            n += 1;
        }
    }

    /// The deepest timeline folder spanning the insertion slot between `marker - 1` and
    /// `marker`. A slot at a folder's outer boundary is not inside that folder.
    fn timeline_group_at_insertion(&self, marker: usize) -> Option<u64> {
        let before = self.features.get(marker.checked_sub(1)?)?.id;
        let after = self.features.get(marker)?.id;
        let direct = |feature| self.timeline_groups.iter().find(|group| group.features.contains(&feature)).map(|group| group.id);
        let parents: std::collections::BTreeMap<u64, Option<u64>> = self.timeline_groups.iter().map(|group| (group.id, group.parent)).collect();

        let mut before_ancestors = std::collections::BTreeSet::new();
        let mut current = direct(before);
        let mut steps = 0usize;
        while let Some(id) = current {
            if !before_ancestors.insert(id) || steps >= self.timeline_groups.len() {
                return None;
            }
            current = parents.get(&id).copied().flatten();
            steps = steps.saturating_add(1);
        }

        current = direct(after);
        steps = 0;
        while let Some(id) = current {
            if before_ancestors.contains(&id) {
                return Some(id);
            }
            if steps >= self.timeline_groups.len() {
                return None;
            }
            current = parents.get(&id).copied().flatten();
            steps = steps.saturating_add(1);
        }
        None
    }

    /// Insert a feature at the marker (or the end) and return its id.
    pub fn add_feature(&mut self, kind: FeatureKind, name: Option<&str>) -> Result<u64> {
        if self.features.len() >= MAX_FEATURES {
            return Err(DocError::Invalid("too many features".into()));
        }
        let component = if let FeatureKind::BodyTransfer { bodies, from, to, transform } = &kind {
            crate::validate_body_transfer(self, &crate::plain(bodies), *from, *to, transform)?;
            *to
        } else {
            0
        };
        let id = self.next_id.max(1);
        self.next_id = id.checked_add(1).ok_or_else(|| DocError::Invalid("too many items in the design".into()))?;
        let name = match name {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => self.unique_name(kind.base_name()),
        };
        let f = Feature {
            id,
            name,
            suppressed: false,
            body_names: Vec::new(),
            param_names: Vec::new(),
            component,
            edge_refs: Vec::new(),
            edge_names: Vec::new(),
            face_names: Vec::new(),
            participants: Vec::new(),
            shell_piece_order: false,
            kind,
        };
        self.place_feature(f);
        Ok(id)
    }

    /// Insert a feature as given, keeping its id, name, parameter names and stored edge and
    /// face names, at the marker (or the end), with the model parameters its sketch dimensions
    /// use: a feature replayed from a script (`timeline.add`) is the feature the design had.
    /// An id of 0 or an empty name gets a new one. Refused, changing nothing, when the id, the
    /// name or a parameter name is already used, or the feature belongs to a component this
    /// design does not have.
    pub fn insert_feature(&mut self, mut f: Feature, params: Vec<Param>) -> Result<u64> {
        if self.features.len() >= MAX_FEATURES {
            return Err(DocError::Invalid("too many features".into()));
        }
        if f.id == 0 {
            f.id = self.next_id.max(1);
        } else if f.id > MAX_FEATURE_ID {
            return Err(DocError::Invalid(format!("feature id {} is out of range", f.id)));
        } else if self.feature(f.id).is_some() {
            return Err(DocError::Invalid(format!("feature id {} is already used", f.id)));
        }
        f.name = f.name.trim().to_string();
        if f.name.is_empty() {
            f.name = self.unique_name(f.kind.base_name());
        } else if self.features.iter().any(|x| x.name == f.name) {
            return Err(DocError::Invalid(format!("a feature named `{}` already exists", f.name)));
        }
        if f.component != 0 && !self.components.iter().any(|c| c.id == f.component) {
            return Err(DocError::Invalid(format!("feature `{}` belongs to an unknown component", f.name)));
        }
        if let FeatureKind::BodyTransfer { bodies, from, to, transform } = &f.kind {
            crate::validate_body_transfer(self, &crate::plain(bodies), *from, *to, transform)?;
            if f.component != *to {
                return Err(DocError::Invalid("a body transfer belongs to its destination component".into()));
            }
        }
        let mut own = std::collections::BTreeSet::new();
        for n in f.param_names.iter().filter(|n| !n.is_empty()) {
            if let Some(why) = Self::param_name_problem(n) {
                return Err(DocError::Invalid(why));
            }
            if self.name_taken(n) || !own.insert(n) {
                return Err(DocError::Invalid(format!("parameter name `{n}` is already used")));
            }
        }
        if self.params.len().saturating_add(params.len()) > MAX_PARAMS {
            return Err(DocError::Invalid("too many parameters".into()));
        }
        for p in &params {
            if let Some(why) = Self::param_name_problem(&p.name) {
                return Err(DocError::Invalid(why));
            }
            if self.name_taken(&p.name) || !own.insert(&p.name) {
                return Err(DocError::Invalid(format!("parameter name `{}` is already used", p.name)));
            }
            if p.expr.len() > 4096 || p.comment.len() > 4096 {
                return Err(DocError::Expr(format!("parameter `{}`: expression or comment too long", p.name)));
            }
            if !p.unit.is_empty() && expr::unit_info(&p.unit).is_none() {
                return Err(DocError::Invalid(format!("parameter `{}`: unknown unit `{}`", p.name, p.unit)));
            }
        }
        let id = f.id;
        self.next_id = self.next_id.max(id.saturating_add(1));
        self.params.extend(params.into_iter().map(|p| Param { model: true, ..p }));
        self.place_feature(f);
        Ok(id)
    }

    /// Put a feature at the marker (or the end), in the timeline folder that spans that slot,
    /// and name its inputs.
    fn place_feature(&mut self, f: Feature) {
        let id = f.id;
        let insertion_group = self.marker.and_then(|marker| self.timeline_group_at_insertion(marker));
        match self.marker {
            Some(m) if m < self.features.len() => {
                self.features.insert(m, f);
                self.marker = Some(m + 1);
            }
            _ => self.features.push(f),
        }
        if let Some(group_id) = insertion_group {
            let positions: std::collections::BTreeMap<u64, usize> =
                self.features.iter().enumerate().map(|(index, feature)| (feature.id, index)).collect();
            if let Some(group) = self.timeline_groups.iter_mut().find(|group| group.id == group_id) {
                group.features.push(id);
                group.features.sort_by_key(|feature| positions.get(feature).copied().unwrap_or(usize::MAX));
            }
        }
        self.name_feature_inputs(id);
    }

    /// Delete a feature (and, for a sketch, the features that use it).
    pub fn delete_feature(&mut self, id: u64) -> Result<Vec<u64>> {
        let idx = self.feature_index(id).ok_or_else(|| DocError::Unknown(format!("feature {id}")))?;
        let mut gone = vec![id];
        gone.extend(self.features.iter().filter(|f| f.kind.input_sketches().contains(&id)).map(|f| f.id));
        let gone_params: Vec<String> = self.features.iter().filter(|f| gone.contains(&f.id)).flat_map(|f| f.param_names.clone()).collect();
        self.configs
            .rules
            .retain(|rule| !rule.references_features(&gone) && !rule.parameter_references().iter().any(|name| gone_params.contains(name)));
        self.features.retain(|f| !gone.contains(&f.id));
        for group in &mut self.timeline_groups {
            group.features.retain(|feature| !gone.contains(feature));
        }
        // Configuration columns of deleted features go with them.
        let keep: Vec<bool> = self
            .configs
            .columns
            .iter()
            .map(|c| match c {
                crate::config::Column::Suppress(id) => !gone.contains(id),
                crate::config::Column::Param(name) => !gone_params.contains(name),
                crate::config::Column::FaceAppearance(target) => !gone.contains(&target.origin_feature),
                _ => true,
            })
            .collect();
        self.appearances
            .faces
            .retain(|face| !face.strict_name || !face.name.as_ref().is_some_and(|name| gone.iter().any(|id| name.starts_with(&format!("F{id}:")))));
        if keep.contains(&false) {
            let mut k = keep.iter();
            self.configs.columns.retain(|_| k.next().copied().unwrap_or(true));
            for r in &mut self.configs.rows {
                let mut k = keep.iter();
                r.cells.retain(|_| k.next().copied().unwrap_or(true));
            }
        }
        if let Some(m) = self.marker
            && m > idx
        {
            self.marker = Some(m.saturating_sub(gone.len()).max(idx));
        }
        // Drop model parameters nobody references any more.
        self.prune_model_params();
        Ok(gone)
    }

    /// Remove model parameters not referenced by any dimension, feature or other parameter.
    pub fn prune_model_params(&mut self) {
        let used = self.referenced_names();
        self.params.retain(|p| !p.model || used.contains(&p.name));
    }

    fn referenced_names(&self) -> Vec<String> {
        let mut used: Vec<String> = Vec::new();
        for rule in &self.configs.rules {
            used.extend(rule.parameter_references());
        }
        for (index, column) in self.configs.columns.iter().enumerate() {
            if let crate::config::Column::Param(name) = column {
                used.push(name.clone());
                for row in &self.configs.rows {
                    if let Some(value) = row.cells.get(index) {
                        used.extend(expr::references(value));
                    }
                }
            }
        }
        for f in &self.features {
            for e in f.kind.expressions() {
                used.extend(expr::references(e));
            }
            if let FeatureKind::Sketch { sketch, .. } = &f.kind {
                used.extend(sketch.constraints.iter().filter_map(|c| c.param.clone()));
                used.extend(sketch.text_exprs().into_iter().flat_map(|(_, e)| expr::references(e)));
            }
        }
        for p in &self.params {
            used.extend(expr::references(&p.expr));
        }
        used
    }

    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.name == name)
    }

    /// Why a name cannot be a parameter name (None = fine).
    pub fn param_name_problem(name: &str) -> Option<String> {
        let mut cs = name.chars();
        if !matches!(cs.next(), Some(c) if c.is_alphabetic() || c == '_') || !cs.all(|c| c.is_alphanumeric() || c == '_') {
            return Some(format!("`{name}` is not a valid parameter name (letters, digits and _, starting with a letter)"));
        }
        if name.len() > 64 {
            return Some("parameter names are at most 64 characters".into());
        }
        if expr::is_reserved(name) {
            return Some(format!("`{name}` is a unit, constant or function name"));
        }
        None
    }

    /// Add or change a parameter. Fails if the result would not evaluate. A new parameter
    /// without a unit takes the unit of its value (mm, deg or none).
    pub fn set_param(&mut self, name: &str, expr_s: &str, unit: Option<&str>, comment: Option<&str>) -> Result<()> {
        if let Some(p) = Self::param_name_problem(name) {
            return Err(DocError::Invalid(p));
        }
        if expr_s.len() > 4096 {
            return Err(DocError::Expr("expression too long".into()));
        }
        if expr_s.trim().is_empty() {
            return Err(DocError::Expr("empty expression".into()));
        }
        if let Some(u) = unit
            && expr::unit_info(u).is_none()
        {
            return Err(DocError::Expr(format!("unknown unit `{u}` (mm, cm, m, in, ft, deg, rad or none)")));
        }
        if comment.is_some_and(|c| c.len() > 4096) {
            return Err(DocError::Invalid("comment too long".into()));
        }
        if self.features.iter().any(|f| f.param_names.iter().any(|n| n == name)) {
            return Err(DocError::Invalid(format!("`{name}` is a feature's parameter")));
        }
        let mut next = self.clone();
        let inferred = match unit {
            Some(u) => u.trim().to_string(),
            None => next.infer_unit(expr_s),
        };
        match next.params.iter_mut().find(|p| p.name == name) {
            Some(p) => {
                p.expr = expr_s.trim().to_string();
                if let Some(u) = unit {
                    p.unit = u.trim().to_string();
                }
                if let Some(c) = comment {
                    p.comment = c.to_string();
                }
            }
            None => {
                if next.params.len() >= MAX_PARAMS {
                    return Err(DocError::Invalid("too many parameters".into()));
                }
                next.params.push(Param {
                    name: name.into(),
                    expr: expr_s.trim().into(),
                    unit: inferred,
                    comment: comment.unwrap_or_default().into(),
                    model: false,
                })
            }
        }
        let (_, errors) = next.param_values();
        if let Some(e) = errors.get(name) {
            return Err(DocError::Expr(e.clone()));
        }
        *self = next;
        Ok(())
    }

    /// Create the next free model parameter `dN` with the given expression and unit.
    pub fn new_model_param(&mut self, expr_s: &str, unit: &str) -> String {
        let name = self.next_d_name();
        self.params.push(Param { name: name.clone(), expr: expr_s.into(), unit: unit.into(), comment: String::new(), model: true });
        name
    }

    /// Evaluate an expression against the document's parameters.
    pub fn eval(&self, e: &str, kind: Kind) -> Result<f64> {
        // The parameters are evaluated only when the expression names one: most expressions an
        // agent sends are plain values, and evaluating every parameter of a large design for
        // each of them made every new feature cost more than the last.
        let vals: std::cell::OnceCell<std::collections::BTreeMap<String, expr::Value>> = std::cell::OnceCell::new();
        expr::eval_with(e, &|n| {
            vals.get_or_init(|| self.param_values().0).get(n).copied().ok_or_else(|| DocError::Expr(format!("unknown parameter `{n}`")))
        })?
        .to_kind(kind)
    }

    pub fn eval_in(vals: &std::collections::BTreeMap<String, expr::Value>, e: &str, kind: Kind) -> Result<f64> {
        expr::eval_with(e, &|n| vals.get(n).copied().ok_or_else(|| DocError::Expr(format!("unknown parameter `{n}`"))))?.to_kind(kind)
    }

    /// Resolve a plane reference.
    pub fn resolve_plane(&self, vals: &std::collections::BTreeMap<String, expr::Value>, p: &PlaneRef, depth: usize) -> Result<Plane> {
        self.resolve_plane_in(vals, p, depth, None)
    }

    /// Resolve a plane; planes built from model geometry (edges, faces, construction axes and
    /// points) need the model state `st`, and construction planes are read from it when given.
    pub fn resolve_plane_in(
        &self,
        vals: &std::collections::BTreeMap<String, expr::Value>,
        p: &PlaneRef,
        depth: usize,
        st: Option<&crate::ModelState>,
    ) -> Result<Plane> {
        if depth > 32 {
            return Err(DocError::Invalid("plane offsets nested too deeply".into()));
        }
        match p {
            PlaneRef::Origin { name } => Plane::named(name).ok_or_else(|| DocError::Unknown(format!("plane `{name}`"))),
            PlaneRef::Custom { plane } | PlaneRef::Face { plane, .. } => {
                Plane::new(plane.origin, plane.x, plane.y).ok_or_else(|| DocError::Invalid("degenerate plane".into()))
            }
            PlaneRef::Placed { origin, x_dir, y_dir } => {
                let [x, y, z] = origin;
                let o =
                    Vec3::new(Self::eval_in(vals, x, Kind::Length)?, Self::eval_in(vals, y, Kind::Length)?, Self::eval_in(vals, z, Kind::Length)?);
                if ![o.x, o.y, o.z].iter().all(|c| c.is_finite() && c.abs() < 1e9) {
                    return Err(DocError::Invalid("the plane's origin is out of range".into()));
                }
                Plane::new(o, *x_dir, *y_dir).ok_or_else(|| DocError::Invalid("degenerate plane".into()))
            }
            PlaneRef::Offset { base, distance } => {
                let b = self.resolve_plane_in(vals, base, depth + 1, st)?;
                Ok(b.offset(Self::eval_in(vals, distance, Kind::Length)?))
            }
            PlaneRef::AtAngle { base, axis_origin, axis_dir, angle } => {
                let b = self.resolve_plane_in(vals, base, depth + 1, st)?;
                let a = Self::eval_in(vals, angle, Kind::Angle)?;
                let d = axis_dir.normalized().ok_or_else(|| DocError::Invalid("rotation axis".into()))?;
                let rot = |v: Vec3| v * a.cos() + d.cross(v) * a.sin() + d * (d.dot(v) * (1.0 - a.cos()));
                let o = *axis_origin + rot(b.origin - *axis_origin);
                Plane::new(o, rot(b.x), rot(b.y)).ok_or_else(|| DocError::Invalid("degenerate plane".into()))
            }
            PlaneRef::Flipped { base, about_y } => Ok(self.resolve_plane_in(vals, base, depth + 1, st)?.flipped(*about_y)),
            PlaneRef::Construction { name } => {
                // Evaluated already: as it came out.
                if let Some(c) = st.and_then(|st| st.construct.iter().find(|c| &c.name == name))
                    && let crate::construct::ConstructGeom::Plane(pl) = &c.geom
                {
                    return Ok(*pl);
                }
                match self.find_feature(name).map(|f| &f.kind) {
                    Some(FeatureKind::ConstructionPlane { plane }) => self.resolve_plane_in(vals, plane, depth + 1, st),
                    _ => Err(DocError::Unknown(format!("construction plane `{name}`"))),
                }
            }
            PlaneRef::Midplane { .. }
            | PlaneRef::TwoEdges { .. }
            | PlaneRef::ThreePoints { .. }
            | PlaneRef::Tangent { .. }
            | PlaneRef::AlongPath { .. }
            | PlaneRef::Perpendicular { .. } => {
                let st = st.ok_or_else(|| DocError::Invalid("this plane is built from the model: evaluate first".into()))?;
                crate::construct::resolve_geo_plane(&crate::construct::Ctx { doc: self, vals, st, depth }, p)
            }
        }
    }

    /// Mutable access to a sketch feature's sketch.
    pub fn sketch_mut(&mut self, id: u64) -> Result<&mut Sketch> {
        match self.feature_mut(id).map(|f| &mut f.kind) {
            Some(FeatureKind::Sketch { sketch, .. }) => Ok(sketch),
            Some(_) => Err(DocError::Invalid(format!("feature {id} is not a sketch"))),
            None => Err(DocError::Unknown(format!("sketch {id}"))),
        }
    }
    pub fn sketch(&self, id: u64) -> Result<&Sketch> {
        match self.feature(id).map(|f| &f.kind) {
            Some(FeatureKind::Sketch { sketch, .. }) => Ok(sketch),
            Some(_) => Err(DocError::Invalid(format!("feature {id} is not a sketch"))),
            None => Err(DocError::Unknown(format!("sketch {id}"))),
        }
    }

    /// Apply parameter values to every dimension in a sketch (in place).
    pub fn apply_dimension_values(&self, vals: &std::collections::BTreeMap<String, expr::Value>, sk: &mut Sketch) -> Result<()> {
        for c in &mut sk.constraints {
            if let Some(p) = &c.param {
                let kind = if c.kind.is_angle() { Kind::Angle } else { Kind::Length };
                let v = vals.get(p).copied().ok_or_else(|| DocError::Unknown(format!("parameter `{p}`")))?.to_kind(kind)?;
                c.kind.set_value(v);
            }
        }
        for c in &sk.constraints {
            sk.validate(&c.kind)?;
        }
        Ok(())
    }

    /// Give the texts of a sketch the numbers their expressions come to (in place): a text's
    /// height, place and angle that were typed as expressions. A text's outlines are rebuilt from
    /// its numbers when the sketch's links are refreshed, so this comes before that. A number
    /// typed as a number is left as it is.
    pub fn apply_text_values(&self, vals: &std::collections::BTreeMap<String, expr::Value>, sk: &mut Sketch) -> Result<()> {
        use crate::eval::val;
        for link in &mut sk.links {
            let solidcraft_sketch::LinkSource::Text { at, height, angle, exprs, .. } = &mut link.source else { continue };
            let number = |e: &Option<String>, kind: Kind, what: &str| -> Result<Option<f64>> {
                match e {
                    None => Ok(None),
                    Some(e) => val(vals, e, kind).map(Some).map_err(|error| DocError::Expr(format!("{what}: {error}"))),
                }
            };
            if let Some(v) = number(&exprs.x, Kind::Length, "text X")? {
                at.x = v;
            }
            if let Some(v) = number(&exprs.y, Kind::Length, "text Y")? {
                at.y = v;
            }
            if let Some(v) = number(&exprs.height, Kind::Length, "text height")? {
                *height = v;
            }
            if let Some(v) = number(&exprs.angle, Kind::Angle, "text angle")? {
                *angle = v;
            }
            if exprs.height.is_some() && !(*height > 1e-6 && *height < 1e5) {
                return Err(DocError::Invalid("a text's height must be positive".into()));
            }
        }
        Ok(())
    }

    /// Apply parameter values to a sketch's dimensions and solve it. `stored` are the values
    /// its geometry is solved for (the stored sketch's): what the solve cannot reach in one
    /// jump it reaches by stepping the changed values from there, and an edit that would turn
    /// the sketch over is refused. Offsets whose sources moved are refreshed and solved again:
    /// from the stored state again after a refused or failed first solve (the geometry is still
    /// the stored one), from the solved one otherwise.
    ///
    /// `moved_from`: the stored sketch, when `sk` is that sketch with its links refreshed. A
    /// link's points are where their source is now; the sides and turns the solve keeps are
    /// those of the stored state ([`Sketch::positions_in`]), not the ones read beside a line
    /// that has moved past a point.
    pub fn solve_sketch(
        &self,
        vals: &std::collections::BTreeMap<String, expr::Value>,
        sk: &mut Sketch,
        stored: &solidcraft_sketch::DrivingValues,
        moved_from: Option<&Sketch>,
    ) -> Result<solidcraft_sketch::SolveReport> {
        self.apply_dimension_values(vals, sk)?;
        let was = |sk: &Sketch| moved_from.and_then(|from| sk.positions_in(from));
        let mut report = solidcraft_sketch::solve_after_edit(sk, stored, was(sk).as_deref());
        if !sk.offsets.is_empty() || !sk.freeform_offsets.is_empty() {
            // Source dimensions may have moved curves during the solve.
            sk.refresh_offsets()?;
            report = if report.ok() { solidcraft_sketch::solve(sk) } else { solidcraft_sketch::solve_after_edit(sk, stored, was(sk).as_deref()) };
        }
        Ok(report)
    }
}

/// Density of the default material (g/cm³), as Fusion's default.
#[path = "../../../assets/materials/library.rs"]
mod materials;
pub use materials::{DEFAULT_DENSITY, MATERIALS, material_color};

/// A physical material defined in a design (`material.create`): its name, its density and,
/// optionally, the look it gives bodies without an appearance of their own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserMaterial {
    pub name: String,
    /// Density (g/cm³).
    pub density: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<crate::appearance::Look>,
}

/// Largest density a material may have (g/cm³); the densest elements are about 22.6.
pub const MAX_DENSITY: f64 = 1000.0;

/// A density a material can have: finite, above zero and at most [`MAX_DENSITY`] g/cm³.
pub fn valid_density(d: f64) -> bool {
    d.is_finite() && d > 0.0 && d <= MAX_DENSITY
}

/// Density of a built-in physical material (g/cm³).
pub fn material_density(name: &str) -> Option<f64> {
    MATERIALS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name.trim())).map(|(_, d)| *d)
}
