//! Sheet metal: rules (thickness, K-factor, bend radius, reliefs as expressions that may use
//! parameters), and the sheet body model behind every sheet-metal feature.
//!
//! A sheet body is kept as its flat pattern: the base panel, flanges hanging off panel edges
//! (each a bend zone of width BA = θ·(R + K·T) followed by a flat panel), and cut-outs, all in
//! the flat frame (the base panel's bottom face; thickness grows along its normal). Folding maps
//! each flange's flat coordinates through its parents' bends: past a bend line, a point loses
//! the bend zone's width and turns by θ about the bend axis (R from the inner face). The folded
//! solid is built from extruded pieces (panels, and bend zones as annular sectors) joined
//! together; the flat solid is the outline extruded through the thickness.

use serde::{Deserialize, Serialize};
use solidcraft_geom::{LINEAR_TOL, Loop2, Plane, Region2, Seg2, Vec2, Vec3};
use solidcraft_kernel::{self as kernel, Body, EdgeSpec, FaceSpec, SurfSpec};

use crate::expr::{self, Kind, Value};
use crate::{DocError, Document, IDENTITY, Mat, Result, apply_point, apply_vector, mat_inverse, mat_mul};

#[path = "sheet_convert.rs"]
mod convert;
pub use convert::{bend_edge_names, convert_body};
pub(crate) use convert::{outline_loop, round_wall};

// Bound the aggregate geometry, including sampled curves and intermediate clipping pieces.
// These are work limits, never geometry truncation: a larger input returns a useful error.
// Exact lines and arcs count once each (the planar booleans work on them whole); free-form
// curves (conics, splines, text) count the arcs the planar booleans follow them with.
const MAX_FLAT_POINTS: usize = 16_384;
// The 0.001 mm chords of everything, arcs included: what the planar booleans sample (their
// own limit is the same).
const MAX_FLAT_SAMPLES: usize = 65_536;
const MAX_FLAT_PIECES: usize = 4096;
const MAX_FLAT_WORK: usize = 500_000_000;
/// Pieces of flat material sharing less area than this (mm²) only touch.
const TOUCH_AREA: f64 = 1e-6;

/// Starts the warning a fold gives when its `fixed` point touches no material: a recompute keeps
/// the side the point lies on, and the `sheet.fold` command refuses such a point (#263).
/// Tabs folded on one line whose material beyond the bend joins again.
const FOLD_TABS_REJOIN: &str = "not supported yet: folding tabs that join again beyond the bend";
pub const FOLD_FIXED_OFF_SHEET: &str = "the fold's fixed point is not on the sheet";

/// The most bores one sheet keeps.
const MAX_BORES: usize = 1000;

fn flat_budget_error() -> DocError {
    DocError::Invalid("sheet metal flat geometry exceeds the bounded processing budget".into())
}

pub(crate) struct FlatWork(usize);

impl FlatWork {
    pub(crate) fn new() -> Self {
        Self(MAX_FLAT_WORK)
    }
    pub(crate) fn charge(&mut self, work: usize) -> Result<()> {
        self.0 = self.0.checked_sub(work).ok_or_else(flat_budget_error)?;
        Ok(())
    }
}

fn flat_polyline(lp: &Loop2) -> Result<Vec<Vec2>> {
    if lp.segs.len() > MAX_FLAT_POINTS {
        return Err(flat_budget_error());
    }
    let mut points = Vec::new();
    for seg in &lp.segs {
        let p = seg.polyline(1e-3);
        let add = p.len().saturating_sub(1);
        if points.len().saturating_add(add) > MAX_FLAT_POINTS {
            return Err(flat_budget_error());
        }
        points.extend(p.iter().take(add));
    }
    Ok(points)
}

fn flat_points(polys: &[Vec<Vec2>]) -> Result<usize> {
    let n = polys.iter().try_fold(0usize, |n, p| n.checked_add(p.len()).ok_or_else(flat_budget_error))?;
    if n > MAX_FLAT_POINTS || polys.len() > MAX_FLAT_PIECES {
        return Err(flat_budget_error());
    }
    Ok(n)
}

/// A loop as the planar booleans arrange it: free-form segments (conics, splines, text)
/// followed by tangent arcs within the kernel's fit tolerance.
fn fitted_loop(lp: &Loop2) -> Result<Loop2> {
    if lp.is_line_arc() {
        return Ok(lp.clone());
    }
    if lp.segs.len() > MAX_FLAT_POINTS {
        return Err(flat_budget_error());
    }
    lp.fit_arcs(kernel::PLANAR_FIT_TOL).ok_or_else(flat_budget_error)
}

/// A region of lines and arcs only (see [`fitted_loop`]): what the folded solid is sewn from.
fn fitted_region(region: &Region2) -> Result<Region2> {
    Ok(Region2 { outer: fitted_loop(&region.outer)?, holes: region.holes.iter().map(fitted_loop).collect::<Result<_>>()? })
}

/// A region with every free-form loop as its 0.001 mm chords: the outline a pocket is
/// machined along. The body booleans take a tool of many small cylinders several times
/// slower than one of planes.
fn chorded_region(region: &Region2) -> Result<Region2> {
    let chorded = |lp: &Loop2| -> Result<Loop2> { if lp.is_line_arc() { Ok(lp.clone()) } else { Ok(Loop2::polygon(&flat_polyline(lp)?)) } };
    Ok(Region2 {
        outer: chorded(&region.outer)?.ccw(),
        holes: region.holes.iter().map(|h| chorded(h).map(|l| l.ccw().reversed())).collect::<Result<_>>()?,
    })
}

/// A line or free-form segment with its points mapped by `f`: exact under any affine map (a
/// conic keeps its weight). An arc is given by angles, not points, and stays as it is.
pub(crate) fn map_controls(seg: &Seg2, f: impl Fn(Vec2) -> Vec2) -> Seg2 {
    match *seg {
        Seg2::Line { a, b } => Seg2::Line { a: f(a), b: f(b) },
        Seg2::Cubic { p0, p1, p2, p3 } => Seg2::Cubic { p0: f(p0), p1: f(p1), p2: f(p2), p3: f(p3) },
        Seg2::Conic { a, apex, b, w } => Seg2::Conic { a: f(a), apex: f(apex), b: f(b), w },
        Seg2::Bezier { n, mut p } => {
            p.iter_mut().take(usize::from(n) + 1).for_each(|q| *q = f(*q));
            Seg2::Bezier { n, p }
        }
        arc @ Seg2::Arc { .. } => arc,
    }
}

fn planar_region(region: &Region2) -> Result<Region2> {
    if region.holes.len() > MAX_FLAT_PIECES {
        return Err(flat_budget_error());
    }
    region_complexity(std::slice::from_ref(region))?;
    // Free-form loops stay exact: the planar booleans follow them with arcs and hand a loop
    // they leave whole its exact segments back.
    Ok(Region2 { outer: region.outer.ccw(), holes: region.holes.iter().map(|h| h.ccw().reversed()).collect() })
}

fn region_complexity(regions: &[Region2]) -> Result<(usize, usize)> {
    regions_complexity(regions.iter())
}

fn regions_complexity<'a>(regions: impl Iterator<Item = &'a Region2>) -> Result<(usize, usize)> {
    let (mut edges, mut points, mut samples) = (0usize, 0usize, 0usize);
    let mut pieces = 0usize;
    for region in regions {
        pieces += 1;
        if pieces > MAX_FLAT_PIECES || region.holes.len() > MAX_FLAT_PIECES {
            return Err(flat_budget_error());
        }
        for lp in std::iter::once(&region.outer).chain(&region.holes) {
            // A free-form loop costs what the planar booleans work on: its fitted arcs.
            let fitted;
            let lp = if lp.is_line_arc() {
                lp
            } else {
                fitted = fitted_loop(lp)?;
                &fitted
            };
            edges = edges.saturating_add(lp.segs.len());
            if edges > MAX_FLAT_POINTS {
                return Err(flat_budget_error());
            }
            for seg in &lp.segs {
                let chords = seg.polyline(1e-3).len().saturating_sub(1);
                points = points.saturating_add(1);
                samples = samples.saturating_add(chords);
                if points > MAX_FLAT_POINTS || samples > MAX_FLAT_SAMPLES {
                    return Err(flat_budget_error());
                }
            }
        }
    }
    Ok((edges, points))
}

/// A flat region's cut-outs by their bounding boxes, to clip tools against only the ones near
/// them (the result is the same: a cut-out clear of a tool's box can't change what it meets).
pub(crate) struct NearHoles<'a> {
    region: &'a Region2,
    boxes: Vec<(Vec2, Vec2)>,
}

/// A loop's bounding box, a little larger than its chords so arcs stay inside.
fn loop_box(lp: &Loop2) -> Result<(Vec2, Vec2)> {
    let pts = flat_polyline(lp)?;
    let (lo, hi) = pts.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), p| {
        (Vec2::new(lo.x.min(p.x), lo.y.min(p.y)), Vec2::new(hi.x.max(p.x), hi.y.max(p.y)))
    });
    let pad = 1e-3 + 1e-9 * (lo.x.abs() + lo.y.abs() + hi.x.abs() + hi.y.abs());
    Ok((lo - Vec2::new(pad, pad), hi + Vec2::new(pad, pad)))
}

impl<'a> NearHoles<'a> {
    pub(crate) fn new(region: &'a Region2) -> Result<Self> {
        Ok(Self { region, boxes: region.holes.iter().map(loop_box).collect::<Result<_>>()? })
    }

    /// The region with only the cut-outs whose boxes meet `tool`'s.
    pub(crate) fn around(&self, tool: &Region2) -> Result<Region2> {
        if self.region.holes.is_empty() {
            return Ok(self.region.clone());
        }
        let (lo, hi) = loop_box(&tool.outer)?;
        let holes = self
            .region
            .holes
            .iter()
            .zip(&self.boxes)
            .filter(|(_, (a, b))| a.x <= hi.x && b.x >= lo.x && a.y <= hi.y && b.y >= lo.y)
            .map(|(h, _)| h.clone())
            .collect();
        Ok(Region2 { outer: self.region.outer.clone(), holes })
    }
}

pub(crate) fn material_boolean(a: &[Region2], b: &[Region2], op: kernel::BoolOp, work: &mut FlatWork) -> Result<Vec<Region2>> {
    let (ae, ap) = region_complexity(a)?;
    let (be, bp) = region_complexity(b)?;
    let edges = ae.saturating_add(be);
    work.charge(edges.saturating_mul(edges).saturating_add(edges.saturating_mul(ap.saturating_add(bp))).saturating_mul(8))?;
    let regions = kernel::planar_regions_boolean(a, b, op, 1e-7)?;
    region_complexity(&regions)?;
    Ok(regions)
}

// Merge in balanced rounds so hundreds of disjoint holes or adjoining panels do not
// repeatedly arrange a growing accumulated boundary against just one small region.
pub(crate) fn material_union(regions: Vec<Region2>, work: &mut FlatWork) -> Result<Vec<Region2>> {
    region_complexity(&regions)?;
    let mut groups: Vec<Vec<Region2>> = regions.into_iter().map(|r| vec![r]).collect();
    while groups.len() > 1 {
        let mut next = Vec::new();
        let mut iter = groups.into_iter();
        while let Some(a) = iter.next() {
            match iter.next() {
                Some(b) => next.push(material_boolean(&a, &b, kernel::BoolOp::Union, work)?),
                None => next.push(a),
            }
        }
        groups = next;
    }
    Ok(groups.pop().unwrap_or_default())
}
/// A sheet metal rule. Values are expressions; `Thickness` inside them is the rule's thickness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetRule {
    pub name: String,
    pub thickness: String,
    pub k_factor: String,
    pub bend_radius: String,
    #[serde(default = "rule_relief_width")]
    pub relief_width: String,
    #[serde(default = "rule_relief_depth")]
    pub relief_depth: String,
    #[serde(default = "rule_corner_relief")]
    pub corner_relief: String,
    /// What is cut where two bends meet at a corner of the base: `notch` (nothing more: the
    /// bends end square at the corner), `round` or `rectangular` (a relief `corner_relief`
    /// across, centred on the corner the two bends share), or `tear` (no relief: each bend's
    /// end is cut straight from that corner to where its flange's end starts).
    #[serde(default = "rule_corner_relief_shape")]
    pub corner_relief_shape: String,
    /// Inner radius of flat hems.
    #[serde(default = "rule_hem_gap")]
    pub hem_gap: String,
    /// Gap between flanges that meet at a corner.
    #[serde(default = "rule_gap")]
    pub gap: String,
}

fn rule_gap() -> String {
    "Thickness".into()
}

fn rule_relief_width() -> String {
    "Thickness".into()
}
fn rule_relief_depth() -> String {
    "Thickness * 0.5".into()
}
fn rule_corner_relief() -> String {
    "Thickness * 4.0".into()
}
fn rule_corner_relief_shape() -> String {
    ReliefShape::Notch.name().into()
}
fn rule_hem_gap() -> String {
    "0.01 mm".into()
}

impl SheetRule {
    /// Fusion's default: Steel (mm), 2.5 mm, K 0.44, bend radius = Thickness.
    pub fn steel() -> SheetRule {
        SheetRule {
            name: "Steel (mm)".into(),
            thickness: "2.5 mm".into(),
            k_factor: "0.44".into(),
            bend_radius: "Thickness".into(),
            relief_width: rule_relief_width(),
            relief_depth: rule_relief_depth(),
            corner_relief: rule_corner_relief(),
            corner_relief_shape: rule_corner_relief_shape(),
            hem_gap: rule_hem_gap(),
            gap: rule_gap(),
        }
    }
}

/// The design's sheet metal rules and the one new sheet bodies use.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetSettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<SheetRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

impl SheetSettings {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.active.is_none()
    }
}

/// The shape of the relief cut where two bends meet at a corner of the base (a rule's
/// `corner_relief_shape`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReliefShape {
    /// Nothing is cut: the bends end square at the corner.
    #[default]
    Notch,
    /// A round about the corner of the bends, through the angle the base fills there.
    Round,
    /// A square with a corner on the corner of the bends (a kite where the base's corner isn't
    /// square: both cuts leave their edge at a right angle).
    Rectangular,
    /// No relief: both bends run to their corner, and each one's end is cut straight, in the
    /// flat, from that corner to where its flange's closed end starts.
    Tear,
}

impl ReliefShape {
    pub fn parse(s: &str) -> Option<ReliefShape> {
        match s.trim().to_ascii_lowercase().as_str() {
            "notch" => Some(ReliefShape::Notch),
            "round" => Some(ReliefShape::Round),
            "rectangular" => Some(ReliefShape::Rectangular),
            "tear" => Some(ReliefShape::Tear),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ReliefShape::Notch => "notch",
            ReliefShape::Round => "round",
            ReliefShape::Rectangular => "rectangular",
            ReliefShape::Tear => "tear",
        }
    }
}

/// The corner relief an edge flange feature asks for in place of the rule's. A value left out
/// follows the rule.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReliefOverride {
    pub shape: Option<ReliefShape>,
    /// How far across the relief is (mm).
    pub size: Option<f64>,
}

/// A rule's values (mm, unit-less K).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RuleValues {
    pub thickness: f64,
    pub k_factor: f64,
    pub bend_radius: f64,
    pub relief_width: f64,
    pub relief_depth: f64,
    pub corner_relief: f64,
    pub corner_relief_shape: ReliefShape,
    pub hem_gap: f64,
    pub gap: f64,
}

impl Document {
    /// A rule by name, the active one, or the default steel rule.
    pub fn sheet_rule(&self, name: Option<&str>) -> SheetRule {
        let want = name.or(self.sheet.active.as_deref());
        want.and_then(|n| self.sheet.rules.iter().find(|r| r.name == n))
            .or_else(|| self.sheet.rules.first())
            .cloned()
            .unwrap_or_else(SheetRule::steel)
    }

    /// Evaluate a rule: `Thickness` in its expressions is the rule's thickness.
    pub fn rule_values(&self, vals: &std::collections::BTreeMap<String, Value>, r: &SheetRule) -> Result<RuleValues> {
        let look = |n: &str| vals.get(n).copied().ok_or_else(|| DocError::Expr(format!("unknown parameter `{n}`")));
        let t = expr::eval_with(&r.thickness, &look)?.to_kind(Kind::Length)?;
        if !(t > 0.0 && t < 1e4) {
            return Err(DocError::Invalid("the sheet thickness must be positive".into()));
        }
        let with_t = |e: &str, k: Kind| -> Result<f64> {
            let f = |n: &str| if n == "Thickness" && !vals.contains_key("Thickness") { Ok(Value::length(t)) } else { look(n) };
            expr::eval_with(e, &f)?.to_kind(k)
        };
        let v = RuleValues {
            thickness: t,
            k_factor: with_t(&r.k_factor, Kind::Unitless)?,
            bend_radius: with_t(&r.bend_radius, Kind::Length)?,
            relief_width: with_t(&r.relief_width, Kind::Length)?,
            relief_depth: with_t(&r.relief_depth, Kind::Length)?,
            corner_relief: with_t(&r.corner_relief, Kind::Length)?,
            corner_relief_shape: ReliefShape::parse(&r.corner_relief_shape).ok_or_else(|| {
                DocError::Invalid(format!("unknown corner relief shape `{}` (notch, round, rectangular or tear)", r.corner_relief_shape))
            })?,
            hem_gap: with_t(&r.hem_gap, Kind::Length)?,
            gap: with_t(&r.gap, Kind::Length)?,
        };
        if !(0.0..=1.0).contains(&v.k_factor) || v.bend_radius < 0.0 {
            return Err(DocError::Invalid("the K-factor must be 0…1 and the bend radius positive".into()));
        }
        Ok(v)
    }
}

/// A flange: a bend along a panel edge and the flat panel past it (flat coordinates).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SheetFlange {
    /// The flange it hangs off (None = the base panel).
    pub parent: Option<usize>,
    /// Bend start line: from `p` along `d` for `len`; `n` points into the flange.
    pub p: Vec2,
    pub d: Vec2,
    pub len: f64,
    pub n: Vec2,
    /// Bend angle (radians; positive bends toward the top face, +z of the flat frame).
    pub angle: f64,
    /// Inner bend radius.
    pub radius: f64,
    /// Flat length of the panel past the bend zone.
    pub leg: f64,
    /// The panel runs on past the bend's ends by these (toward a neighbouring flange), measured
    /// along the bend line; negative values cut the panel back.
    #[serde(default)]
    pub ext: (f64, f64),
    /// How much further each end runs on per unit of panel height: a closed corner between
    /// flanges that aren't at 90° ends on a slanted line in the flat.
    #[serde(default, skip_serializing_if = "is_zero_pair")]
    pub slope: (f64, f64),
    /// Torn ends (the rule's `tear` corner relief): there the bend doesn't end square across,
    /// but on the straight line, in the flat, from its start line's end to where the panel's
    /// end starts.
    #[serde(default, skip_serializing_if = "is_false_pair")]
    pub tear: (bool, bool),
    /// A fold's panel: the part of the sheet past the bend (flat coordinates, counter-clockwise)
    /// instead of a rectangle `leg` long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Vec<Vec2>>,
    /// A converted face's panel as drawn: `shape` with arcs where the face's edges are round
    /// (flat coordinates, counter-clockwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<Loop2>,
    /// Which feature made it: `F<feature>.<k>` (persistent edge names).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tag: String,
}

/// One edge flange to make: a pick on the sheet's edge, and that flange's height (to the outer
/// face, from the virtual sharp) and angle (radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeFlange {
    pub pick: Vec3,
    pub height: f64,
    pub angle: f64,
}

/// A sheet metal body (see the module docs).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SheetBody {
    pub body: String,
    pub rule: String,
    pub t: f64,
    pub k: f64,
    /// The flat frame: base panel bottom face; z = thickness direction.
    pub frame: Plane,
    pub base: Region2,
    pub flanges: Vec<SheetFlange>,
    /// Cut-outs (flat coordinates) through the sheet.
    pub holes: Vec<Region2>,
    /// Planar machining in flat coordinates, with depths measured from the flat bottom.
    /// Derived from feature history; these records are not a document schema change.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pockets: Vec<SheetPocket>,
    /// Straight bores drilled into a panel along its plane (a hole into a rim), in flat
    /// coordinates, each kept inside its own panel's material so the folded part and its flat
    /// pattern both carry it (#560). Derived from feature history, like `pockets`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bores: Vec<SheetBore>,
    /// Perforation fields (#732), flat coordinates; their holes are computed on demand and never cut into the B-rep.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub perforations: Vec<SheetPerforation>,
    /// Regions (flat coordinates) that text cut or machined, letters with their counters as
    /// holes: a record of where the sheet's text went, so that its DXF can write the letters on
    /// layers of their own (see [`SheetBody::letters`]). The cut-outs and pockets themselves are
    /// in `holes` and `pockets`. Derived from feature history, like `pockets`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<Region2>,
    /// Unfolded.
    pub flat: bool,
    /// Gap between flanges that meet at a corner.
    pub gap: f64,
    /// The relief cut where two bends meet at a corner of the base, and how far across it is
    /// (the rule's `corner_relief_shape` and `corner_relief`).
    pub relief: ReliefShape,
    pub relief_size: f64,
    /// The rule's bend relief (`relief_width`, `relief_depth`), cut into the base where a
    /// flange's bend ends beside more of the base.
    pub bend_relief: (f64, f64),
    /// The base sketch's lines (id, ends in flat coordinates), naming the base panel's edges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub base_curves: Vec<(String, Vec2, Vec2)>,
}

/// A straight round bore of one panel (None = the base), in flat coordinates: a cylinder from
/// `base` along the unit `axis` for `height`, cut only from that panel's material.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SheetBore {
    pub panel: Option<usize>,
    pub base: Vec3,
    pub axis: Vec3,
    pub radius: f64,
    pub height: f64,
}

/// A perforation field from a `sheet.perforate` feature (#732).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SheetPerforation {
    pub feature: u64,
    pub diameter: f64,
    pub pitch: f64,
    pub margin: f64,
    pub square: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SheetPocket {
    pub region: Region2,
    pub z0: f64,
    pub z1: f64,
}

/// A piece of the sheet: a panel (None = the base) or a flange's bend zone.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Piece {
    Panel(Option<usize>),
    Zone(usize),
}

/// A flat piece of the sheet (counter-clockwise outer loop, clockwise holes).
#[derive(Clone, Debug)]
struct Cell {
    piece: Piece,
    outer: Vec<Seg2>,
    holes: Vec<Vec<Seg2>>,
}

fn is_zero_pair(v: &(f64, f64)) -> bool {
    v.0 == 0.0 && v.1 == 0.0
}

fn is_false_pair(v: &(bool, bool)) -> bool {
    !v.0 && !v.1
}

fn rev_edge(e: EdgeSpec) -> EdgeSpec {
    e.reversed()
}

/// Largest distance between a folded bend edge and the chain standing in for it.
const BEND_CHAIN_TOL: f64 = 1e-7;
/// Degree of the pieces of a folded bend edge's chain. Hermite pieces of degree 7 match the
/// edge's point and first three derivatives at both ends, so one or two of them follow a helix
/// round a whole bend, where cubic pieces (point and tangent) took 32 and more.
const BEND_CHAIN_DEGREE: usize = 7;
/// Most pieces of a folded bend edge's chain.
const BEND_CHAIN_PIECES: usize = 256;

/// The point at `t` of the Bézier over `c` (de Casteljau).
fn bezier_at(c: &[Vec3], t: f64) -> Vec3 {
    let mut w = c.to_vec();
    while w.len() > 1 {
        w = w.windows(2).map(|p| p[0].lerp(p[1], t)).collect();
    }
    w.first().copied().unwrap_or(Vec3::ZERO)
}

fn dedup(p: &[Vec2]) -> Vec<Vec2> {
    let mut v: Vec<Vec2> = Vec::new();
    for q in p {
        if v.last().is_none_or(|l| l.dist(*q) > 1e-9) {
            v.push(*q);
        }
    }
    if v.len() > 1 && v.first().zip(v.last()).is_some_and(|(a, b)| a.dist(*b) < 1e-9) {
        v.pop();
    }
    v
}

/// How a flat edge of a bend zone folds: along the bend to a line, across it to an arc, or
/// any other way (slanted lines and curves) to a curve on the bend's cylinder.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ZoneEdge {
    Along,
    Across,
    Curved,
}

/// How far (mm) a flat edge's ends may stray from a line along or across its bend and still
/// fold as that line or arc: scripts round coordinates, so the test is in millimetres, not
/// relative to the edge's length.
const ZONE_SQUARE_TOL: f64 = 1e-6;

fn zone_edge_kind(f: &SheetFlange, e: &Seg2) -> ZoneEdge {
    let Seg2::Line { a, b } = *e else { return ZoneEdge::Curved };
    let dlt = b - a;
    if dlt.dot(f.n).abs() <= ZONE_SQUARE_TOL {
        ZoneEdge::Along
    } else if dlt.dot(f.d).abs() <= ZONE_SQUARE_TOL {
        ZoneEdge::Across
    } else {
        ZoneEdge::Curved
    }
}

/// A bend for the bend table.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BendInfo {
    pub index: usize,
    pub angle_deg: f64,
    pub radius: f64,
    pub up: bool,
    pub allowance: f64,
    pub length: f64,
    /// Centre line in flat coordinates.
    pub start: Vec2,
    pub end: Vec2,
}

fn rot_about(o: Vec3, k: Vec3, a: f64) -> Mat {
    crate::rigid(Vec3::ZERO, o, k, a)
}

fn translate(t: Vec3) -> Mat {
    let mut m = IDENTITY;
    m[3] = [t.x, t.y, t.z, 1.0];
    m
}

fn v3(v: Vec2) -> Vec3 {
    Vec3::new(v.x, v.y, 0.0)
}

fn plane_of(m: &Mat, o: Vec3, x: Vec3, y: Vec3) -> Result<Plane> {
    Plane::new(apply_point(m, o), apply_vector(m, x), apply_vector(m, y)).ok_or_else(|| DocError::Invalid("sheet: degenerate plane".into()))
}

fn rect(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Region2 {
    let mut lp = Loop2::polygon(&[a, b, c, d]);
    if lp.signed_area() < 0.0 {
        lp = lp.reversed();
    }
    Region2 { outer: lp, holes: Vec::new() }
}

/// Round to a 1e-9 mm grid (keeps trimmed edges exactly where the kernel expects them).
fn snap(v: Vec2) -> Vec2 {
    let r = |x: f64| (x * 1e9).round() / 1e9;
    Vec2::new(r(v.x), r(v.y))
}

/// Clip a polygon to the half-plane (p − o)·n ≥ 0.
fn clip(poly: &[Vec2], o: Vec2, n: Vec2) -> Vec<Vec2> {
    let mut out = Vec::new();
    let m = poly.len();
    for i in 0..m {
        let (Some(&a), Some(&b)) = (poly.get(i), poly.get((i + 1) % m)) else { continue };
        let (da, db) = ((a - o).dot(n), (b - o).dot(n));
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(snap(a + (b - a) * t));
        }
    }
    out
}

/// [`clip`] to the half-plane (p − o)·n ≥ 0 (`n` a unit vector) at a trim line: a corner within
/// the model's tolerance of the line lies on it (it stays, moved onto the line), and an edge
/// crosses the line only between corners clear of it on either side, so no corner comes out
/// twice. Also says whether a corner lay on the line: the polygon may then run along the line
/// out and back (a ledge beside the edge that ends on the line, or a hair either side of it,
/// all of it in the strip that goes), which is no material.
fn clip_at_line(poly: &[Vec2], o: Vec2, n: Vec2) -> (Vec<Vec2>, bool) {
    let side = |p: Vec2| {
        let v = (p - o).dot(n);
        if v.abs() <= LINEAR_TOL { 0.0 } else { v }
    };
    let (mut out, mut on) = (Vec::new(), false);
    let m = poly.len();
    for i in 0..m {
        let (Some(&a), Some(&b)) = (poly.get(i), poly.get((i + 1) % m)) else { continue };
        let (da, db) = (side(a), side(b));
        if da > 0.0 {
            out.push(a);
        } else if da == 0.0 {
            out.push(snap(a - n * (a - o).dot(n)));
            on = true;
        }
        if da * db < 0.0 {
            out.push(snap(a + (b - a) * (da / (da - db))));
        }
    }
    (out, on)
}

/// The smallest gap closed corners are given: closer than this the flanges' ends would touch.
const MIN_CORNER_GAP: f64 = 0.01;

/// The smallest corner relief (across) that is cut: a smaller one would leave hairline edges.
const MIN_CORNER_RELIEF: f64 = 0.01;

/// The narrowest bend relief that is cut: a narrower one would leave hairline edges.
const MIN_BEND_RELIEF: f64 = 0.01;

/// Two bends on the base meeting at a corner (see [`SheetBody::corners`]): flanges `a` and `b`,
/// which end of each (false = the end at `p`), and the corner point (flat coordinates).
#[derive(Clone, Copy, Debug)]
struct Corner {
    a: usize,
    a_end: bool,
    b: usize,
    b_end: bool,
    c: Vec2,
}

/// The corners of a straight-edged region (counter-clockwise).
fn corners_of(r: &Region2) -> Vec<Vec2> {
    dedup(&r.outer.ccw().polyline(1e-3))
}

/// The area two convex counter-clockwise polygons share.
fn convex_overlap(a: &[Vec2], b: &[Vec2]) -> f64 {
    let mut poly = a.to_vec();
    let m = b.len();
    for i in 0..m {
        let (Some(&p), Some(&q)) = (b.get(i), b.get((i + 1) % m)) else { continue };
        let d = q - p;
        poly = clip(&poly, p, Vec2::new(-d.y, d.x));
        if poly.len() < 3 {
            return 0.0;
        }
    }
    signed_area(&poly).abs()
}

/// How wide a strip two flat pieces may share and still only touch. A flange's bend starts on
/// its parent's far edge at a point snapped to the 1e-9 mm grid, so where that edge runs at a
/// slant to the axes the two share a sliver whose area grows with the edge's length.
const FLAT_SLIVER: f64 = 1e-6;

/// Whether two convex counter-clockwise flat pieces overlap by more than a sliver along an edge.
fn pieces_overlap(a: &[Vec2], b: &[Vec2]) -> bool {
    let perimeter = |p: &[Vec2]| (0..p.len()).filter_map(|i| Some(p.get(i)?.dist(*p.get((i + 1) % p.len())?))).sum::<f64>();
    convex_overlap(a, b) > FLAT_SLIVER * perimeter(a).max(perimeter(b)).max(1.0)
}

/// How far inside a panel (mm) its edge is tested for what cut-outs have left of it: a cut-out
/// whose own edge lies along the panel's takes the edge there, and one that only touches it from
/// outside does not.
const EDGE_INSET: f64 = 1e-6;

/// Whether `p` lies in the triangle (a, b, c), its edges included (within 1e-6 mm).
fn in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let side = |u: Vec2, v: Vec2| {
        let e = v - u;
        e.cross(p - u) / e.len().max(1e-12)
    };
    let (x, y, z) = (side(a, b), side(b, c), side(c, a));
    (x >= -1e-6 && y >= -1e-6 && z >= -1e-6) || (x <= 1e-6 && y <= 1e-6 && z <= 1e-6)
}

/// The least over 0 ≤ φ ≤ `th` (at most π) of `h`, a function of the form
/// a + b·φ + c·cos φ + d·sin φ: at an end, or where its slope b − c·sin φ + d·cos φ vanishes.
fn sinusoid_least(h: &dyn Fn(f64) -> f64, th: f64) -> f64 {
    use std::f64::consts::{FRAC_PI_2, PI, TAU};
    let (h0, h90, h180, h360) = (h(0.0), h(FRAC_PI_2), h(PI), h(TAU));
    let b = (h360 - h0) / TAU;
    let a = (h0 + h180 - b * PI) / 2.0;
    let (c, d) = (h0 - a, h90 - a - b * FRAC_PI_2);
    let mut least = h0.min(h(th));
    // c·sin φ − d·cos φ = r·sin(φ − ψ) = b.
    let r = c.hypot(d);
    if r > 1e-12 && b.abs() <= r {
        let (psi, x) = (d.atan2(c), (b / r).asin());
        for phi in [psi + x, psi + PI - x] {
            let phi = phi.rem_euclid(TAU);
            if phi > 0.0 && phi < th {
                least = least.min(h(phi));
            }
        }
    }
    least
}

/// Two flanges on the far edges of two flanges that meet at a corner of the base (see
/// [`SheetBody::close_return_corners`]): flanges `a` and `b`, which end of each (false = the end
/// at `p`), and where each one's bend ends there (flat coordinates; apart, each on its own parent).
#[derive(Clone, Copy, Debug)]
struct ReturnCorner {
    a: usize,
    a_end: bool,
    b: usize,
    b_end: bool,
    ca: Vec2,
    cb: Vec2,
}

impl SheetBody {
    /// Bend allowance of a flange.
    pub fn allowance(&self, f: &SheetFlange) -> f64 {
        f.angle.abs() * (f.radius + self.k * self.t)
    }

    /// Fold transform of flange `i`'s panel (flat 3D coordinates → folded, in the flat frame).
    pub fn fold(&self, i: usize) -> Mat {
        let mut chain = Vec::new();
        let mut cur = Some(i);
        for _ in 0..1000 {
            let Some(c) = cur else { break };
            chain.push(c);
            cur = self.flanges.get(c).and_then(|f| f.parent);
        }
        let mut m = IDENTITY;
        for c in chain.into_iter().rev() {
            if let Some(f) = self.flanges.get(c) {
                m = mat_mul(&m, &self.bend_motion(f));
            }
        }
        m
    }

    /// The fold of one bend: drop the bend zone, then turn about the bend axis.
    fn bend_motion(&self, f: &SheetFlange) -> Mat {
        let ba = self.allowance(f);
        let (axis_o, k, a) = self.bend_axis(f);
        mat_mul(&rot_about(axis_o, k, a), &translate(v3(f.n) * -ba))
    }

    /// Bend axis point, direction and signed turn (flat coordinates).
    fn bend_axis(&self, f: &SheetFlange) -> (Vec3, Vec3, f64) {
        let up = f.angle >= 0.0;
        let z = if up { self.t + f.radius } else { -f.radius };
        let o = v3(f.p) + Vec3::Z * z;
        let k = v3(f.n).cross(Vec3::Z);
        (o, k, if up { f.angle.abs() } else { -f.angle.abs() })
    }

    /// Flat region of a flange's panel.
    pub fn panel(&self, f: &SheetFlange) -> Region2 {
        if let Some(lp) = &f.exact {
            return Region2 { outer: lp.ccw(), holes: Vec::new() };
        }
        if let Some(sh) = &f.shape {
            return Region2 { outer: Loop2::polygon(sh).ccw(), holes: Vec::new() };
        }
        let ba = self.allowance(f);
        let a = f.p + f.n * ba - f.d * f.ext.0;
        let b = f.p + f.n * ba + f.d * (f.len + f.ext.1);
        rect(a, b, b + f.n * f.leg + f.d * (f.slope.1 * f.leg), a + f.n * f.leg - f.d * (f.slope.0 * f.leg))
    }

    /// Where a flange panel's far edge (`leg` past the bend zone) starts and ends, along the bend
    /// from its start `p`: a closed corner runs the panel's end on or cuts it back there (`ext`,
    /// `slope`).
    fn far_edge(&self, f: &SheetFlange) -> (f64, f64) {
        (-f.ext.0 - f.slope.0 * f.leg, f.len + f.ext.1 + f.slope.1 * f.leg)
    }

    /// Flat region of a flange's bend zone: square across the bend at both ends, but at a torn
    /// end its far edge ends where the panel's does.
    pub fn zone(&self, f: &SheetFlange) -> Region2 {
        let ba = self.allowance(f);
        let (u0, u1) = self.zone_span(f);
        let (a, b) = (f.p + f.d * u0, f.p + f.d * u1);
        // The panel's own corners (`panel`), so that the two share the whole edge.
        let far_a = if f.tear.0 { f.p + f.n * ba - f.d * f.ext.0 } else { a + f.n * ba };
        let far_b = if f.tear.1 { f.p + f.n * ba + f.d * (f.len + f.ext.1) } else { b + f.n * ba };
        rect(a, b, far_b, far_a)
    }

    /// Where a bend zone's far edge (on its panel's side) starts and ends, along the bend from
    /// `p`: where its start line does (`zone_span`), or at a torn end where the panel's end starts.
    fn zone_ends(&self, f: &SheetFlange) -> (f64, f64) {
        let (u0, u1) = self.zone_span(f);
        (if f.tear.0 { -f.ext.0 } else { u0 }, if f.tear.1 { f.len + f.ext.1 } else { u1 })
    }

    /// Folded (in the flat frame, as `fold`), the point of a bend's end (the one at `p`, or the
    /// other one when `at_end`) at the turn `phi` of the bend and the height `z` in the sheet. A
    /// torn end runs evenly along the bend while the bend turns, from its start line's end to
    /// where the panel's end starts.
    fn bend_end(&self, f: &SheetFlange, at_end: bool, phi: f64, z: f64) -> Vec3 {
        let ((u0, u1), (e0, e1)) = (self.zone_span(f), self.zone_ends(f));
        let (from, to) = if at_end { (u1, e1) } else { (u0, e0) };
        let u = from + (to - from) * phi / f.angle.abs().max(1e-12);
        let parent = f.parent.map_or(IDENTITY, |i| self.fold(i));
        let (axis_o, k, turn) = self.bend_axis(f);
        apply_point(&mat_mul(&parent, &rot_about(axis_o, k, turn.signum() * phi)), v3(f.p + f.d * u) + Vec3::Z * z)
    }

    /// The part of a bend along its parent's edge: where a later flange trimmed that edge back
    /// (a corner) or a corner relief cut into it, the bend stops and the flange panel runs on
    /// alone.
    pub fn zone_span(&self, f: &SheetFlange) -> (f64, f64) {
        if f.parent.is_some() {
            return (0.0, f.len);
        }
        let pts = self.base.outer.ccw().polyline(1e-3);
        let m = pts.len();
        let mut span: Option<(f64, f64)> = None;
        for k in 0..m {
            let (Some(&a), Some(&b)) = (pts.get(k), pts.get((k + 1) % m)) else { continue };
            let on = (a - f.p).dot(f.n).abs() < 1e-6 && (b - f.p).dot(f.n).abs() < 1e-6;
            if !on {
                continue;
            }
            let (ua, ub) = ((a - f.p).dot(f.d), (b - f.p).dot(f.d));
            let (lo, hi) = (ua.min(ub).max(0.0), ua.max(ub).min(f.len));
            if hi > lo + 1e-9 {
                span = Some(span.map_or((lo, hi), |(x, y)| (x.min(lo), y.max(hi))));
            }
        }
        span.unwrap_or((0.0, f.len))
    }

    fn frame_mat(&self) -> Mat {
        let fr = &self.frame;
        let n = fr.normal();
        [[fr.x.x, fr.x.y, fr.x.z, 0.0], [fr.y.x, fr.y.y, fr.y.z, 0.0], [n.x, n.y, n.z, 0.0], [fr.origin.x, fr.origin.y, fr.origin.z, 1.0]]
    }

    /// Flat coordinates (x, y, z) → world, through the fold of flange `i` (None = base).
    pub fn world(&self, i: Option<usize>) -> Mat {
        let f = match i {
            Some(i) if !self.flat => self.fold(i),
            _ => IDENTITY,
        };
        mat_mul(&self.frame_mat(), &f)
    }

    /// Where a flat point of a piece lands (folded), at height `z` in the sheet.
    fn map3(&self, piece: Piece, q: Vec2, z: f64) -> Vec3 {
        match piece {
            Piece::Panel(i) => apply_point(&self.world(i), v3(q) + Vec3::Z * z),
            Piece::Zone(i) => {
                let Some(f) = self.flanges.get(i) else { return Vec3::ZERO };
                let s = (q - f.p).dot(f.n);
                let q0 = q - f.n * s;
                let neutral = (f.radius + self.k * self.t).max(1e-12);
                let phi = (s / neutral).clamp(0.0, f.angle.abs());
                let (axis_o, k, turn) = self.bend_axis(f);
                let m = mat_mul(&self.world(f.parent), &rot_about(axis_o, k, turn.signum() * phi));
                apply_point(&m, v3(q0) + Vec3::Z * z)
            }
        }
    }

    /// A flat point of flange `f`'s bend zone folded onto the bend at height `z` (not clamped
    /// to the zone), with its first three derivatives along the zone's flat `n` direction and
    /// its first along `d` (along `d` the bend is straight: the higher ones are zero, and so are
    /// the mixed ones).
    fn zone_point(&self, f: &SheetFlange, q: Vec2, z: f64) -> (Vec3, [Vec3; 3], Vec3) {
        let s = (q - f.p).dot(f.n);
        let u = (q - f.p).dot(f.d);
        let neutral = (f.radius + self.k * self.t).max(1e-12);
        let phi = s / neutral;
        let (axis_o, k, turn) = self.bend_axis(f);
        let sigma = if turn < 0.0 { -1.0 } else { 1.0 };
        // From the axis to the bend's start line at height z, and that turned a right angle.
        let ez = Vec3::Z * (z - axis_o.z);
        let ek = k.cross(ez) * sigma;
        let (sn, cs) = phi.sin_cos();
        let w = self.world(f.parent);
        let local = axis_o + v3(f.d) * u + ez * cs + ek * sn;
        // Each derivative along n turns the last a right angle about the axis and divides it by
        // the neutral radius.
        let ds = (ez * -sn + ek * cs) * (1.0 / neutral);
        let dss = (ez * cs + ek * sn) * (-1.0 / (neutral * neutral));
        let dsss = ds * (-1.0 / (neutral * neutral));
        let along_n = [ds, dss, dsss].map(|v| apply_vector(&w, v));
        (apply_point(&w, local), along_n, apply_vector(&w, v3(f.d)))
    }

    /// A flat edge of flange `f`'s bend zone, folded at height `z`, at parameter `t`: the point
    /// and its first three derivatives.
    fn zone_edge_at(&self, f: &SheetFlange, e: &Seg2, z: f64, t: f64) -> [Vec3; 4] {
        let (p, [g1, g2, g3], du) = self.zone_point(f, e.point_at(t), z);
        let [q1, q2, q3] = match e.derivatives(t, 3).as_deref() {
            Some(&[q1, q2, q3]) => [q1, q2, q3],
            _ => [e.derivative(t), Vec2::ZERO, Vec2::ZERO],
        };
        let ([s1, s2, s3], [u1, u2, u3]) = ([q1, q2, q3].map(|q| q.dot(f.n)), [q1, q2, q3].map(|q| q.dot(f.d)));
        // The chain rule (Faà di Bruno) through s(t) along n and u(t) along d.
        [p, g1 * s1 + du * u1, g2 * (s1 * s1) + g1 * s2 + du * u2, g3 * (s1 * s1 * s1) + g2 * (3.0 * s1 * s2) + g1 * s3 + du * u3]
    }

    /// The control points of the Hermite piece of degree 7 of a folded bend edge from `t0` to
    /// `t1`: it has the edge's point and first three derivatives at both ends. (The k-th
    /// derivative of a Bézier of degree 7 at its start is 7!/(7 − k)! times the k-th forward
    /// difference of its first control points, and the edge's k-th derivative scales by
    /// (t1 − t0)^k to the piece's parameter; at its end the same, backwards.)
    fn zone_piece(&self, f: &SheetFlange, e: &Seg2, z: f64, t0: f64, t1: f64) -> [Vec3; BEND_CHAIN_DEGREE + 1] {
        let h = t1 - t0;
        let ([p0, a1, a2, a3], [p1, b1, b2, b3]) = (self.zone_edge_at(f, e, z, t0), self.zone_edge_at(f, e, z, t1));
        let (k1, k2, k3) = (h / 7.0, h * h / 42.0, h * h * h / 210.0);
        [
            p0,
            p0 + a1 * k1,
            p0 + a1 * (2.0 * k1) + a2 * k2,
            p0 + a1 * (3.0 * k1) + a2 * (3.0 * k2) + a3 * k3,
            p1 - b1 * (3.0 * k1) + b2 * (3.0 * k2) - b3 * k3,
            p1 - b1 * (2.0 * k1) + b2 * k2,
            p1 - b1 * k1,
            p1,
        ]
    }

    /// Into how many equal pieces (of its parameter) a bend zone's flat edge is folded, so that
    /// the Hermite pieces are within [`BEND_CHAIN_TOL`] of the folded edge on both faces. The
    /// count depends on the flat edge only, so the top face, bottom face and wall of a cut-out
    /// share one chain per height; and as the pieces are equal, the chain's parameter runs evenly
    /// with the edge's, and the pieces join with equal derivatives up to the third.
    fn zone_pieces(&self, f: &SheetFlange, e: &Seg2) -> usize {
        // The largest distance between the pieces and the folded edge, at points inside each.
        let worst = |n: usize| {
            let mut worst = 0.0f64;
            for k in 0..n {
                let (t0, t1) = (k as f64 / n as f64, (k + 1) as f64 / n as f64);
                for z in [0.0, self.t] {
                    let c = self.zone_piece(f, e, z, t0, t1);
                    for j in 1..8 {
                        let t = t0 + (t1 - t0) * j as f64 / 8.0;
                        worst = worst.max(bezier_at(&c, j as f64 / 8.0).dist(self.zone_edge_at(f, e, z, t)[0]));
                    }
                }
            }
            worst
        };
        let mut n = 1;
        loop {
            let off = worst(n);
            if off <= BEND_CHAIN_TOL || !off.is_finite() || n >= BEND_CHAIN_PIECES {
                return n;
            }
            // The distance falls as the eighth power of the pieces' length.
            let guess = (n as f64 * (off / BEND_CHAIN_TOL).powf(1.0 / 8.0)).ceil();
            n = (guess as usize).clamp(n + 1, BEND_CHAIN_PIECES);
        }
    }

    /// A bend zone's flat edge folded at height `z`: a chain of Hermite pieces of degree 7.
    fn zone_chain(&self, f: &SheetFlange, e: &Seg2, z: f64) -> EdgeSpec {
        let n = self.zone_pieces(f, e);
        let mut ctrl = Vec::with_capacity(BEND_CHAIN_DEGREE * n + 1);
        for k in 0..n {
            let c = self.zone_piece(f, e, z, k as f64 / n as f64, (k + 1) as f64 / n as f64);
            ctrl.extend_from_slice(if k == 0 { &c } else { &c[1..] });
        }
        EdgeSpec::Chain { degree: BEND_CHAIN_DEGREE, ctrl: ctrl.into() }
    }

    /// A flat edge of a piece as a 3D edge at height `z`.
    fn edge3(&self, piece: Piece, e: &Seg2, z: f64) -> Result<EdgeSpec> {
        let (a, b) = (e.start(), e.end());
        let (pa, pb) = (self.map3(piece, a, z), self.map3(piece, b, z));
        match (piece, e) {
            (Piece::Panel(_), Seg2::Line { .. }) => Ok(EdgeSpec::Line { a: pa, b: pb }),
            (Piece::Panel(_), Seg2::Arc { .. }) => Ok(EdgeSpec::Arc { a: pa, b: pb, mid: self.map3(piece, e.point_at(0.5), z) }),
            (Piece::Zone(i), _) => {
                let f = self.flanges.get(i).ok_or_else(|| DocError::Invalid("bend".into()))?;
                match zone_edge_kind(f, e) {
                    ZoneEdge::Along => Ok(EdgeSpec::Line { a: pa, b: pb }),
                    ZoneEdge::Across => Ok(EdgeSpec::Arc { a: pa, b: pb, mid: self.map3(piece, (a + b) * 0.5, z) }),
                    // Slanted lines fold to helices, curves to curves on the bend's cylinder:
                    // neither has a line, arc or conic form.
                    ZoneEdge::Curved => Ok(self.zone_chain(f, e, z)),
                }
            }
            (_, _) => Err(DocError::Invalid("not supported yet: spline or conic cut-outs in sheet metal".into())),
        }
    }

    /// The pieces of the sheet as cells: base and flange panels and bend zones, each less
    /// the combined cut-out material. Planar clipping retains holes and disconnected islands.
    fn cells_with_work(&self, work: &mut FlatWork) -> Result<Vec<Cell>> {
        if self.flanges.len().saturating_mul(2).saturating_add(1) > MAX_FLAT_PIECES || self.holes.len() > MAX_FLAT_PIECES {
            return Err(flat_budget_error());
        }
        let raw = self
            .flanges
            .iter()
            .fold(self.base.outer.segs.len(), |n, f| n.saturating_add(f.shape.as_ref().map_or(4, Vec::len)))
            .saturating_add(self.holes.iter().fold(0usize, |n, h| n.saturating_add(h.outer.segs.len())));
        if raw > MAX_FLAT_POINTS {
            return Err(flat_budget_error());
        }
        let mut pieces: Vec<(Piece, Region2)> = self.panels().into_iter().map(|(pi, r)| (Piece::Panel(pi), r)).collect();
        for (i, f) in self.flanges.iter().enumerate() {
            pieces.push((Piece::Zone(i), self.zone(f)));
        }
        // The folded solid is sewn from lines and arcs: free-form outlines and cut-outs follow
        // their fitted arcs there (the flat pattern keeps them exact).
        let mut cuts = Vec::new();
        let mut cut_points = 0usize;
        for hole in &self.holes {
            let region = fitted_region(&planar_region(hole)?)?;
            cut_points = cut_points.saturating_add(region_complexity(std::slice::from_ref(&region))?.1);
            if cut_points > MAX_FLAT_POINTS {
                return Err(flat_budget_error());
            }
            cuts.push(region);
        }
        for pocket in self.through_pocket_regions(work)? {
            cuts.push(fitted_region(&pocket)?);
        }
        let cuts = material_union(cuts, work)?;
        let cut_boxes = cuts.iter().map(|c| loop_box(&c.outer)).collect::<Result<Vec<_>>>()?;
        let mut out = Vec::new();
        let mut points = 0usize;
        for (piece, region) in pieces {
            let panel = fitted_region(&planar_region(&region)?)?;
            // Each piece meets only the cut-outs near it: arranging every cut-out of the sheet (a
            // label's letters on its base) against each flange and bend zone too would charge the
            // budget for all of them once per piece.
            let (lo, hi) = loop_box(&panel.outer)?;
            let near: Vec<Region2> = cuts
                .iter()
                .zip(&cut_boxes)
                .filter(|(_, (a, b))| a.x <= hi.x && b.x >= lo.x && a.y <= hi.y && b.y >= lo.y)
                .map(|(c, _)| c.clone())
                .collect();
            let material = if near.is_empty() { vec![panel] } else { material_boolean(&[panel], &near, kernel::BoolOp::Cut, work)? };
            for region in material {
                points = points.saturating_add(region_complexity(std::slice::from_ref(&region))?.1);
                if points > MAX_FLAT_POINTS || out.len() >= MAX_FLAT_PIECES {
                    return Err(flat_budget_error());
                }
                // A piece shorter than the model's tolerance is no edge: a cut-out's corner a hair
                // beside a bend line comes out of the booleans as corners a hair apart, on the
                // panel and on the bend, joined by such an edge or by a loop of them on its own.
                // The sew welds the corners left on either side.
                let hairless = |l: Loop2| -> Vec<Seg2> { l.segs.into_iter().filter(|e| e.length() >= LINEAR_TOL).collect() };
                let outer = hairless(region.outer);
                if outer.is_empty() {
                    continue;
                }
                let holes = region.holes.into_iter().map(hairless).filter(|h| !h.is_empty()).collect();
                out.push(Cell { piece, outer, holes });
            }
        }
        Ok(out)
    }
    /// Faces of the solid made by `cells` between heights `z0` and `z1`.
    fn faces(&self, cells: &[Cell], z0: f64, z1: f64, work: &mut FlatWork) -> Result<Vec<FaceSpec>> {
        // Split straight edges where other cells' corners touch them, so neighbours share edges.
        let edges = cells
            .iter()
            .fold(0usize, |n, c| n.saturating_add(c.outer.len()).saturating_add(c.holes.iter().fold(0usize, |m, h| m.saturating_add(h.len()))));
        if edges > MAX_FLAT_POINTS || cells.len() > MAX_FLAT_PIECES {
            return Err(flat_budget_error());
        }
        work.charge(edges.saturating_mul(edges).saturating_mul(4))?;
        let corners: Vec<Vec2> = cells.iter().flat_map(|c| c.outer.iter().chain(c.holes.iter().flatten()).map(Seg2::start)).collect();
        let mut split_edges = 0usize;
        let mut split_loop = |segs: &[Seg2]| -> Result<Vec<Seg2>> {
            let mut v = Vec::new();
            for e in segs {
                let Seg2::Line { a, b } = *e else {
                    split_edges += 1;
                    if split_edges > MAX_FLAT_POINTS {
                        return Err(flat_budget_error());
                    }
                    v.push(*e);
                    continue;
                };
                let d = b - a;
                let (l2, l) = (d.len2(), d.len());
                // Not within the model's tolerance of an end or of each other: a corner that near
                // an end is that end (sewing welds the two), and a piece that short is no edge. A
                // short edge beside a bend's end (the side of a ledge a little taller than the
                // setback) used to be cut where the bend's corner lies, under a nanometre from
                // its own.
                let mut ts: Vec<f64> = corners
                    .iter()
                    .filter_map(|p| {
                        let t = (*p - a).dot(d) / l2.max(1e-30);
                        ((t * l > LINEAR_TOL && (1.0 - t) * l > LINEAR_TOL) && (a + d * t).dist(*p) < 1e-7).then_some(t)
                    })
                    .collect();
                ts.sort_by(|x, y| x.total_cmp(y));
                ts.dedup_by(|x, y| (*x - *y).abs() * l < LINEAR_TOL);
                let mut prev = a;
                for t in ts {
                    split_edges += 1;
                    if split_edges > MAX_FLAT_POINTS {
                        return Err(flat_budget_error());
                    }
                    let q = a + d * t;
                    v.push(Seg2::Line { a: prev, b: q });
                    prev = q;
                }
                split_edges += 1;
                if split_edges > MAX_FLAT_POINTS {
                    return Err(flat_budget_error());
                }
                v.push(Seg2::Line { a: prev, b });
            }
            Ok(v)
        };
        let mut split_cells = Vec::new();
        for c in cells {
            let outer = split_loop(&c.outer)?;
            let holes = c.holes.iter().map(|h| split_loop(h)).collect::<Result<_>>()?;
            split_cells.push(Cell { piece: c.piece, outer, holes });
        }
        // Reserve the complete adjacency scan before visiting any wall.
        work.charge(split_edges.saturating_mul(split_edges).saturating_mul(2))?;
        let cells = split_cells;
        let near = |p: Vec2, q: Vec2| p.dist(q) < 1e-6;
        // Shared with a neighbour: the same flat edge the other way, landing in the same place
        // (flat neighbours across a corner of two bends do not).
        let shared = |ci: usize, e: &Seg2| {
            let Some(me) = cells.get(ci) else { return false };
            let here = self.map3(me.piece, e.point_at(0.5), z0);
            cells.iter().enumerate().any(|(cj, c)| {
                cj != ci
                    && c.outer.iter().chain(c.holes.iter().flatten()).any(|g| {
                        near(g.start(), e.end())
                            && near(g.end(), e.start())
                            && matches!((g, e), (Seg2::Line { .. }, Seg2::Line { .. }))
                            && self.map3(c.piece, g.point_at(0.5), z0).dist(here) < 1e-6
                    })
            })
        };
        let mut faces = Vec::new();
        for (ci, c) in cells.iter().enumerate() {
            // Top (z1, outward +z) and bottom (z0, outward −z, loops reversed).
            let loops = |z: f64, rev: bool| -> Result<Vec<Vec<EdgeSpec>>> {
                std::iter::once(&c.outer)
                    .chain(c.holes.iter())
                    .map(|l| {
                        let mut v: Vec<EdgeSpec> = l.iter().map(|e| self.edge3(c.piece, e, z)).collect::<Result<_>>()?;
                        if rev {
                            v = v.into_iter().rev().map(rev_edge).collect();
                        }
                        Ok(v)
                    })
                    .collect()
            };
            let centre = c.outer.iter().map(Seg2::start).fold(Vec2::default(), |a, b| a + b) * (1.0 / c.outer.len().max(1) as f64);
            for (z, rev, sign) in [(z1, false, 1.0), (z0, true, -1.0)] {
                let surface = match c.piece {
                    Piece::Panel(_) => SurfSpec::Plane,
                    Piece::Zone(i) => {
                        let f = self.flanges.get(i).ok_or_else(|| DocError::Invalid("bend".into()))?;
                        // The surface must hold the whole boundary: curved cut-out edges may
                        // bulge past the cell's corners, so they are sampled (and the surface
                        // padded a little, never past the zone's own width).
                        let curved = c.outer.iter().any(|e| zone_edge_kind(f, e) == ZoneEdge::Curved);
                        let pts: Vec<Vec2> =
                            if curved { c.outer.iter().flat_map(|e| e.polyline(1e-4)).collect() } else { c.outer.iter().map(Seg2::start).collect() };
                        let ss: Vec<f64> = pts.iter().map(|q| (*q - f.p).dot(f.n)).collect();
                        let (mut s0, mut s1) = (ss.iter().cloned().fold(f64::MAX, f64::min), ss.iter().cloned().fold(f64::MIN, f64::max));
                        let us: Vec<f64> = pts.iter().map(|q| (*q - f.p).dot(f.d)).collect();
                        let (mut u0, mut u1) = (us.iter().cloned().fold(f64::MAX, f64::min), us.iter().cloned().fold(f64::MIN, f64::max));
                        if curved {
                            let pad = 1e-3;
                            (s0, s1) = ((s0 - pad).max(0.0), (s1 + pad).min(self.allowance(f)));
                            (u0, u1) = (u0 - pad, u1 + pad);
                        }
                        let neutral = (f.radius + self.k * self.t).max(1e-12);
                        let (axis_o, k, turn) = self.bend_axis(f);
                        let wp = self.world(f.parent);
                        let q = |u: f64| f.p + f.n * s0 + f.d * u;
                        SurfSpec::Revolved {
                            a: self.map3(c.piece, q(u0), z),
                            b: self.map3(c.piece, q(u1), z),
                            axis_origin: apply_point(&wp, axis_o),
                            axis: apply_vector(&wp, k),
                            angle: turn.signum() * (s1 - s0) / neutral,
                        }
                    }
                };
                let p = self.map3(c.piece, centre, z);
                let outward = (self.map3(c.piece, centre, z + 1.0) - p) * sign;
                faces.push(FaceSpec { loops: loops(z, rev)?, surface, probe: Some((p, outward)) });
            }
            // Walls along edges no other cell shares.
            for e in c.outer.iter().chain(c.holes.iter().flatten()) {
                if shared(ci, e) {
                    continue;
                }
                let (a, b) = (e.start(), e.end());
                let bottom = self.edge3(c.piece, e, z0)?;
                let top_ab = self.edge3(c.piece, e, z1)?;
                let up_b = EdgeSpec::Line { a: self.map3(c.piece, b, z0), b: self.map3(c.piece, b, z1) };
                let down_a = EdgeSpec::Line { a: self.map3(c.piece, a, z1), b: self.map3(c.piece, a, z0) };
                let surface = match (c.piece, e) {
                    (Piece::Panel(_), Seg2::Arc { .. }) => {
                        let (pa, pb, pm) = (self.map3(c.piece, a, z0), self.map3(c.piece, b, z0), self.map3(c.piece, e.point_at(0.5), z0));
                        SurfSpec::Extruded { a: pa, b: pb, mid: pm, dir: self.map3(c.piece, a, z1) - pa }
                    }
                    // A cut-out wall across a bend at an angle: square to the sheet, so its
                    // straight lines run radially from the bottom face's edge to the top's.
                    (Piece::Zone(_), _) if matches!(bottom, EdgeSpec::Chain { .. }) => SurfSpec::Ruled { a: bottom.clone(), b: top_ab.clone() },
                    _ => SurfSpec::Plane,
                };
                let top = rev_edge(top_ab);
                let mid = e.point_at(0.5);
                let tdir = e.point_at(0.51) - e.point_at(0.49);
                let right = Vec2::new(tdir.y, -tdir.x);
                let pm = self.map3(c.piece, mid, (z0 + z1) / 2.0);
                let outward = self.map3(c.piece, mid + right * 1e-3, (z0 + z1) / 2.0) - pm;
                faces.push(FaceSpec { loops: vec![vec![bottom, up_b, top, down_a]], surface, probe: Some((pm, outward)) });
            }
        }
        Ok(faces)
    }

    /// The folded (or, when unfolded, flat) solid, sewn from its faces.
    pub fn solid(&self) -> Result<Body> {
        if self.flat {
            return self.flat_solid();
        }
        let mut work = FlatWork::new();
        let cells = self.cells_with_work(&mut work)?;
        // Validate the bounded material first, then build close to the sheet's own
        // origin. Fitting planes after a distant component transfer loses precision.
        let mut local = self.clone();
        local.frame = Plane::XY;
        let body = kernel::sew_touching(&local.faces(&cells, 0.0, self.t, &mut work)?)?;
        let body = local.machine_pockets(body, &mut work)?;
        let body = local.machine_bores(body, &mut work)?;
        self.place_local_solid(body)
    }

    /// The flat pattern as one extrusion (outline with its cut-outs).
    pub fn flat_solid(&self) -> Result<Body> {
        let mut regions = self.flat_regions()?;
        if !self.pockets.is_empty() {
            // Pockets are machined by the prism booleans, which read walls of planes and
            // cylinders: the blank they cut follows its fitted arcs then.
            regions = regions.iter().map(fitted_region).collect::<Result<_>>()?;
        }
        let mut local = self.clone();
        local.frame = Plane::XY;
        local.flat = true;
        let body = kernel::extrude_planar_regions(&Plane::XY, &regions, 0.0, self.t)?;
        let mut work = FlatWork::new();
        let body = local.machine_pockets(body, &mut work)?;
        let body = local.machine_bores(body, &mut work)?;
        self.place_local_solid(body)
    }

    fn place_local_solid(&self, body: Body) -> Result<Body> {
        if self.frame == Plane::XY { Ok(body) } else { Ok(kernel::transform_matrix(&body, self.frame_mat())?) }
    }

    /// Retain bounded derived machining records, without duplicating identical replay work.
    pub(crate) fn record_pocket(&mut self, region: Region2, z0: f64, z1: f64, work: &mut FlatWork) -> Result<()> {
        if !z0.is_finite() || !z1.is_finite() || z0 < 0.0 || z1 > self.t || z0 >= z1 {
            return Err(DocError::Invalid("invalid sheet pocket depth".into()));
        }
        if self.pockets.len() > 1000 {
            return Err(flat_budget_error());
        }
        let pocket = SheetPocket { region, z0, z1 };
        let (old_edges, old_points) = regions_complexity(self.pockets.iter().map(|p| &p.region))?;
        let (new_edges, new_points) = region_complexity(std::slice::from_ref(&pocket.region))?;
        work.charge(
            old_edges.saturating_add(old_points).saturating_add(new_edges.saturating_add(new_points).saturating_mul(self.pockets.len().max(1))),
        )?;
        if self.pockets.contains(&pocket) {
            return Ok(());
        }
        if self.pockets.len() >= 1000 {
            return Err(flat_budget_error());
        }
        if old_edges.saturating_add(new_edges) > MAX_FLAT_POINTS || old_points.saturating_add(new_points) > MAX_FLAT_POINTS {
            return Err(flat_budget_error());
        }
        self.pockets.push(pocket);
        Ok(())
    }

    /// Certify whether a planar panel has material outside a retained thickness interval.
    /// Classification uses exact planar arrangements at every pocket depth boundary.
    pub(crate) fn material_outside_depth(&self, panel: &Region2, z0: f64, z1: f64, work: &mut FlatWork) -> Result<bool> {
        if !z0.is_finite() || !z1.is_finite() || z0 < 0.0 || z1 > self.t || z0 >= z1 {
            return Err(DocError::Invalid("invalid sheet retained depth".into()));
        }
        if self.pockets.len() > 1000 {
            return Err(flat_budget_error());
        }
        regions_complexity(self.pockets.iter().map(|p| &p.region))?;
        let mut levels = vec![0.0, self.t, z0, z1];
        for p in &self.pockets {
            if !p.z0.is_finite() || !p.z1.is_finite() || p.z0 < 0.0 || p.z1 > self.t || p.z0 >= p.z1 {
                return Err(DocError::Invalid("invalid sheet pocket depth".into()));
            }
            levels.extend([p.z0, p.z1]);
        }
        levels.sort_by(f64::total_cmp);
        levels.dedup();
        work.charge(levels.len().saturating_mul(self.pockets.len().max(1)))?;
        for zs in levels.windows(2) {
            if zs[0] >= z0 && zs[1] <= z1 {
                continue;
            }
            let active: Vec<_> = self.pockets.iter().filter(|p| p.z0 <= zs[0] && p.z1 >= zs[1]).map(|p| p.region.clone()).collect();
            let removed = material_union(active, work)?;
            if !material_boolean(std::slice::from_ref(panel), &removed, kernel::BoolOp::Cut, work)?.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Replay exact planar machining after forming the full-thickness material.
    /// Cut the bores (see [`SheetBore`]) from a solid built in the sheet's local frame.
    fn machine_bores(&self, mut body: Body, work: &mut FlatWork) -> Result<Body> {
        if self.bores.is_empty() {
            return Ok(body);
        }
        if self.bores.len() > MAX_BORES {
            return Err(flat_budget_error());
        }
        let panels = self.material_panels(None, work)?;
        for bore in &self.bores {
            let cells = panels.iter().filter(|(pi, _)| *pi == bore.panel).map(|(_, r)| fitted_region(r)).collect::<Result<Vec<_>>>()?;
            if cells.is_empty() {
                continue;
            }
            // BRep replay is costlier than planar clipping; share the command budget.
            work.charge(100_000)?;
            // The bore's own panel, flat: the bore never reaches a bend or another panel, in
            // the folded part or in its flat pattern.
            let slab = kernel::extrude_planar_regions(&Plane::XY, &cells, 0.0, self.t)?;
            let drill = kernel::cylinder(bore.base, bore.axis, bore.radius, bore.height)?;
            let Some(inside) = kernel::boolean(&drill, &slab, kernel::BoolOp::Intersect)? else { continue };
            // Cut with the drill clipped to the panel. Where its ends, flush with the panel's
            // rims, leave coincident faces the cut can't resolve, cut with the drill as long as
            // the panel's material along it and a hair past it instead (the same material).
            let place = |tool: Body| if self.flat { Ok(tool) } else { kernel::transform_matrix(&tool, self.world(bore.panel)) };
            let mut last = None;
            for longer in [false, true] {
                let tool = if longer {
                    let axis = bore.axis.normalized().ok_or_else(|| DocError::Invalid("invalid sheet bore".into()))?;
                    let mesh = inside.tessellate((bore.radius * 0.1).max(1e-4))?;
                    let (lo, hi) =
                        mesh.positions.iter().map(|p| (*p - bore.base).dot(axis)).fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)));
                    if !(lo <= hi) {
                        break;
                    }
                    let past = (bore.radius * 0.05).max(1e-3);
                    kernel::cylinder(bore.base + axis * (lo - past), axis, bore.radius, hi - lo + 2.0 * past)?
                } else {
                    inside.clone()
                };
                match kernel::boolean(&body, &place(tool)?, kernel::BoolOp::Cut) {
                    Ok(Some(cut)) => {
                        body = cut;
                        last = None;
                        break;
                    }
                    Ok(None) => last = Some(DocError::Invalid("a sheet bore removes all sheet material".into())),
                    Err(e) => last = Some(e.into()),
                }
            }
            if let Some(e) = last {
                return Err(e);
            }
        }
        Ok(body)
    }

    /// Record a round bore drilled along the sheet (a hole into a rim): the world cylinder from
    /// `base` along `axis`, one record per panel whose material it meets. Ok(false) when it
    /// meets no material; an error when it meets a bend, or crosses a panel other than along
    /// its plane (that machining has no exact place in the flat pattern).
    pub(crate) fn add_bore(&mut self, base: Vec3, axis: Vec3, radius: f64, height: f64) -> Result<bool> {
        let axis = axis.normalized().ok_or_else(|| DocError::Invalid("bore axis".into()))?;
        if ![base.x, base.y, base.z, radius, height].iter().all(|v| v.is_finite()) || !(radius > 0.0) || height == 0.0 {
            return Err(DocError::Invalid("invalid sheet bore".into()));
        }
        if self.bores.len() >= MAX_BORES {
            return Err(flat_budget_error());
        }
        let mut work = FlatWork::new();
        let drill = kernel::cylinder(base, axis, radius, height)?;
        let mut found = Vec::new();
        for cell in self.cells_with_work(&mut work)? {
            work.charge(100_000)?;
            let solid = if self.flat {
                let region = fitted_region(&Region2 {
                    outer: Loop2 { segs: cell.outer.clone() },
                    holes: cell.holes.iter().map(|h| Loop2 { segs: h.clone() }).collect(),
                })?;
                kernel::extrude_planar_regions(&self.frame, &[region], 0.0, self.t)?
            } else {
                kernel::sew_touching(&self.faces(std::slice::from_ref(&cell), 0.0, self.t, &mut work)?)?
            };
            if kernel::boolean(&solid, &drill, kernel::BoolOp::Intersect)?.is_none() {
                continue;
            }
            let Piece::Panel(pi) = cell.piece else {
                return Err(DocError::Invalid(
                    "this hole runs into a bend; drill it into a flat panel, or unfold the sheet before machining that material".into(),
                ));
            };
            let w = self.world(if self.flat { None } else { pi });
            if apply_vector(&w, Vec3::Z).dot(axis).abs() > 1e-6 {
                return Err(DocError::Invalid(
                    "this hole crosses a sheet panel at an angle; drill it square to the panel, or along it into a rim".into(),
                ));
            }
            let inv = mat_inverse(&w).ok_or_else(|| DocError::Invalid("invalid sheet panel frame".into()))?;
            let bore = SheetBore { panel: pi, base: apply_point(&inv, base), axis: apply_vector(&inv, axis), radius, height };
            if !found.contains(&bore) {
                found.push(bore);
            }
        }
        if found.is_empty() {
            return Ok(false);
        }
        if self.bores.len().saturating_add(found.len()) > MAX_BORES {
            return Err(flat_budget_error());
        }
        self.bores.extend(found);
        Ok(true)
    }

    fn machine_pockets(&self, mut body: Body, work: &mut FlatWork) -> Result<Body> {
        if self.pockets.is_empty() {
            return Ok(body);
        }
        if self.pockets.len() > 1000 {
            return Err(flat_budget_error());
        }
        let outlines = self.pockets.iter().map(|p| chorded_region(&p.region)).collect::<Result<Vec<_>>>()?;
        regions_complexity(outlines.iter())?;
        let panels = if self.flat {
            vec![(None, self.flat_regions()?)]
        } else {
            self.material_panels(None, work)?.into_iter().map(|(pi, r)| (pi, vec![r])).collect()
        };
        for (pocket, outline) in self.pockets.iter().zip(&outlines) {
            if !pocket.z0.is_finite() || !pocket.z1.is_finite() || pocket.z0 < 0.0 || pocket.z1 > self.t || pocket.z0 >= pocket.z1 {
                return Err(DocError::Invalid("invalid sheet pocket depth".into()));
            }
            if !self.flat {
                for flange in &self.flanges {
                    if flat_regions_overlap(&pocket.region, &self.zone(flange), work)? {
                        return Err(DocError::Invalid(
                            "a sheet pocket crosses a bend zone; planar pocket machining cannot yet preserve curved bend material".into(),
                        ));
                    }
                }
            }
            for (pi, panel) in &panels {
                if material_boolean(panel, std::slice::from_ref(outline), kernel::BoolOp::Intersect, work)?.is_empty() {
                    continue;
                }
                // The tool spans the cut-outs inside it (a counterbore's hole, whichever was made
                // first): a wall of its own along a cut-out's wall would only touch it, which the
                // cut cannot resolve. An island of the pocket that holds material stays.
                let whole: Vec<Region2> = panel.iter().map(|r| Region2 { outer: r.outer.clone(), holes: Vec::new() }).collect();
                let mut spanning = Region2 { outer: outline.outer.clone(), holes: Vec::new() };
                for island in &outline.holes {
                    let inside = Region2 { outer: island.ccw(), holes: Vec::new() };
                    if !material_boolean(panel, std::slice::from_ref(&inside), kernel::BoolOp::Intersect, work)?.is_empty() {
                        spanning.holes.push(island.clone());
                    }
                }
                let clipped = material_boolean(&whole, std::slice::from_ref(&spanning), kernel::BoolOp::Intersect, work)?;
                // What the tool takes over from a free-form outline of the panel follows its
                // fitted arcs: walls of planes and cylinders, which the prism booleans take.
                let clipped = clipped.iter().map(fitted_region).collect::<Result<Vec<_>>>()?;
                let w = if self.flat { self.frame_mat() } else { self.world(*pi) };
                let plane = plane_of(&w, Vec3::ZERO, Vec3::X, Vec3::Y)?;
                // BRep replay is costlier than planar clipping; share the command budget.
                work.charge(100_000)?;
                let tool = kernel::extrude_planar_regions(&plane, &clipped, pocket.z0, pocket.z1)?;
                body = kernel::boolean(&body, &tool, kernel::BoolOp::Cut)?
                    .ok_or_else(|| DocError::Invalid("sheet pocket removes all sheet material".into()))?;
            }
        }
        Ok(body)
    }

    /// Actual curved material, used to certify unsupported folded machining before mutation:
    /// one solid per bend zone or nonparallel panel. Each is sewn on its own, because the
    /// selected pieces need not form a manifold solid together (two bends meeting at an open
    /// corner relief touch only along an edge).
    pub(crate) fn unmodeled_solids(&self, plane: &Plane) -> Result<Vec<Body>> {
        if self.flat || self.flanges.is_empty() {
            return Ok(Vec::new());
        }
        let mut work = FlatWork::new();
        let cells: Vec<_> = self
            .cells_with_work(&mut work)?
            .into_iter()
            .filter(|c| match c.piece {
                Piece::Zone(_) => true,
                Piece::Panel(pi) => apply_vector(&self.world(pi), Vec3::Z).cross(plane.normal()).len() > 1e-6,
            })
            .collect();
        cells.iter().map(|c| Ok(kernel::sew_touching(&self.faces(std::slice::from_ref(c), 0.0, self.t, &mut work)?)?)).collect()
    }

    fn through_pocket_regions(&self, work: &mut FlatWork) -> Result<Vec<Region2>> {
        if self.pockets.is_empty() {
            return Ok(Vec::new());
        }
        if self.pockets.len() > 1000 {
            return Err(flat_budget_error());
        }
        regions_complexity(self.pockets.iter().map(|p| &p.region))?;
        let mut levels = vec![0.0, self.t];
        for p in &self.pockets {
            if !p.z0.is_finite() || !p.z1.is_finite() || p.z0 < 0.0 || p.z1 > self.t || p.z0 >= p.z1 {
                return Err(DocError::Invalid("invalid sheet pocket depth".into()));
            }
            levels.extend([p.z0, p.z1]);
        }
        levels.sort_by(f64::total_cmp);
        levels.dedup();
        work.charge(levels.len().saturating_mul(self.pockets.len()))?;
        // Most pockets leave an untouched thickness layer. Certify that cheap case
        // before arranging hundreds of unrelated profiles at every depth level.
        if levels.windows(2).any(|zs| !self.pockets.iter().any(|p| p.z0 <= zs[0] && p.z1 >= zs[1])) {
            return Ok(Vec::new());
        }
        let mut through: Option<Vec<Region2>> = None;
        for zs in levels.windows(2) {
            let active: Vec<_> = self.pockets.iter().filter(|p| p.z0 <= zs[0] && p.z1 >= zs[1]).map(|p| p.region.clone()).collect();
            if active.is_empty() {
                return Ok(Vec::new());
            }
            let removed = material_union(active, work)?;
            through = Some(match through {
                None => removed,
                Some(previous) => material_boolean(&previous, &removed, kernel::BoolOp::Intersect, work)?,
            });
            if through.as_ref().is_some_and(Vec::is_empty) {
                return Ok(Vec::new());
            }
        }
        Ok(through.unwrap_or_default())
    }

    /// The text the sheet carries: what [`SheetBody::labels`] records, ready to tell whether a
    /// loop of the flat pattern is a letter.
    pub fn letters(&self) -> Letters {
        let regions = self
            .labels
            .iter()
            .map(|r| {
                let (mut lo, mut hi) = (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
                for p in r.outer.polyline(1e-3) {
                    lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
                    hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
                }
                // A polyline of 0.001 mm chords is within 0.001 mm of the curve.
                let pad = Vec2::new(0.01, 0.01);
                (lo - pad, hi + pad, std::iter::once(&r.outer).chain(&r.holes).flat_map(|lp| lp.segs.iter().copied()).collect())
            })
            .collect();
        Letters { regions }
    }

    /// Pocket operations clipped to the surviving flat material, with exact depth intervals.
    pub fn machining(&self) -> Result<Vec<SheetPocket>> {
        let material = self.flat_regions()?;
        let mut work = FlatWork::new();
        let mut out = Vec::new();
        for pocket in &self.pockets {
            for region in material_boolean(&material, std::slice::from_ref(&pocket.region), kernel::BoolOp::Intersect, &mut work)? {
                if out.len() >= 1000 {
                    return Err(flat_budget_error());
                }
                out.push(SheetPocket { region, z0: pocket.z0, z1: pocket.z1 });
            }
        }
        Ok(out)
    }

    /// Actual flat material boundaries after cut-outs. The same panel and cut regions build
    /// the folded solid; planar unions remove panel/bend seams and turn edge-crossing cuts
    /// into notches. Lines and circular arcs remain exact through clipping and merging; a
    /// free-form loop that comes out whole is the curve it was drawn as, one that is clipped
    /// follows it with tangent arcs.
    pub fn flat_regions(&self) -> Result<Vec<Region2>> {
        let regions = self.flat_material()?;
        if regions.is_empty() {
            return Err(DocError::Invalid("empty flat pattern".into()));
        }
        Ok(regions)
    }

    pub(crate) fn flat_material(&self) -> Result<Vec<Region2>> {
        let mut work = FlatWork::new();
        // Merge structural pieces before cutting: clipping each panel first introduces
        // coincident notch endpoints at seams, while a single material cut has no seams.
        if self.flanges.len().saturating_mul(2).saturating_add(1) > MAX_FLAT_PIECES || self.holes.len() > MAX_FLAT_PIECES {
            return Err(flat_budget_error());
        }
        let raw = self.flanges.iter().fold(self.base.outer.segs.len(), |n, f| n.saturating_add(f.shape.as_ref().map_or(4, Vec::len)));
        if raw > MAX_FLAT_POINTS {
            return Err(flat_budget_error());
        }
        let mut regions = Vec::new();
        let mut panel_points = 0usize;
        for (_, r) in self.panels() {
            let r = planar_region(&r)?;
            panel_points = panel_points.saturating_add(region_complexity(std::slice::from_ref(&r))?.1);
            if panel_points > MAX_FLAT_POINTS {
                return Err(flat_budget_error());
            }
            regions.push(r);
        }
        for f in &self.flanges {
            regions.push(planar_region(&self.zone(f))?);
            region_complexity(&regions)?;
        }
        let regions = material_union(regions, &mut work)?;
        let mut cuts = Vec::new();
        let mut cut_points = 0usize;
        for hole in &self.holes {
            let region = planar_region(hole)?;
            cut_points = cut_points.saturating_add(region_complexity(std::slice::from_ref(&region))?.1);
            if cut_points > MAX_FLAT_POINTS {
                return Err(flat_budget_error());
            }
            cuts.push(region);
        }
        cuts.extend(self.through_pocket_regions(&mut work)?);
        let cuts = material_union(cuts, &mut work)?;
        let regions = if cuts.is_empty() { regions } else { material_boolean(&regions, &cuts, kernel::BoolOp::Cut, &mut work)? };
        Ok(regions)
    }

    /// The pieces of flat material that a cut parted from the base: `after` is this sheet's
    /// [`Self::flat_material`] after the cut, `before` the sheet before it. A piece is parted when
    /// it holds none of the base panel, lies in a piece that held some before the cut, and no
    /// other piece surrounds it: a flange cut off along its bend (with any flange on it), the
    /// part of a ring's inside that lies on a flange's edge, the tip of a flange beyond a cut
    /// through a closed corner. A ring's island and a letter's counter, inside other material,
    /// are not counted; nor are pieces an earlier cut parted, nor anything once no piece holds
    /// the base.
    pub(crate) fn parted_from_base(&self, after: &[Region2], before: &SheetBody) -> Result<Vec<Region2>> {
        if after.len() < 2 {
            return Ok(Vec::new());
        }
        let mut work = FlatWork::new();
        work.charge(after.len().saturating_mul(after.len()))?;
        let base = [planar_region(&self.base)?];
        let holds_base = |piece: &Region2, work: &mut FlatWork| -> Result<bool> {
            let common = material_boolean(std::slice::from_ref(piece), &base, kernel::BoolOp::Intersect, work)?;
            Ok(common.iter().map(Region2::area).sum::<f64>() > TOUCH_AREA)
        };
        // The cheap test first: a piece inside another's outline is an island, whatever it holds.
        let outlines: Vec<(Vec<Vec2>, Vec2, Vec2)> = after
            .iter()
            .map(|r| {
                let pts = r.outer.polyline(1e-3);
                let lo = pts.iter().fold(Vec2::new(f64::MAX, f64::MAX), |a, p| Vec2::new(a.x.min(p.x), a.y.min(p.y)));
                let hi = pts.iter().fold(Vec2::new(f64::MIN, f64::MIN), |a, p| Vec2::new(a.x.max(p.x), a.y.max(p.y)));
                (pts, lo, hi)
            })
            .collect();
        let (mut open, mut islands) = (Vec::new(), Vec::new());
        for (i, piece) in after.iter().enumerate() {
            let p = piece.interior_point();
            let inside = |(pts, lo, hi): &(Vec<Vec2>, Vec2, Vec2)| (lo.x..=hi.x).contains(&p.x) && (lo.y..=hi.y).contains(&p.y) && point_in(pts, p);
            if outlines.iter().enumerate().any(|(j, o)| j != i && inside(o)) {
                islands.push(piece);
            } else {
                open.push(piece);
            }
        }
        let (mut joined, mut loose) = (false, Vec::new());
        for piece in open {
            if holds_base(piece, &mut work)? {
                joined = true;
            } else {
                loose.push(piece);
            }
        }
        if loose.is_empty() {
            return Ok(Vec::new());
        }
        // The base may be an island itself (a cut all round it).
        for piece in islands {
            if joined {
                break;
            }
            joined = holds_base(piece, &mut work)?;
        }
        if !joined {
            return Ok(Vec::new());
        }
        // A cut only takes material away: each piece lies in the piece it was part of.
        let mut held = Vec::new();
        for piece in before.flat_material()? {
            if holds_base(&piece, &mut work)? {
                held.push(piece);
            }
        }
        Ok(loose
            .into_iter()
            .filter(|piece| {
                let p = piece.interior_point();
                held.iter().any(|h| h.contains(p))
            })
            .cloned()
            .collect())
    }

    /// Outline loops of the flat pattern (flat coordinates): the pieces' edges, shared ones left out.
    pub fn outline(&self) -> Result<Vec<Vec<Vec2>>> {
        if self.flanges.len().saturating_mul(2).saturating_add(1) > MAX_FLAT_PIECES {
            return Err(flat_budget_error());
        }
        let mut polys: Vec<Vec<Vec2>> = vec![dedup(&flat_polyline(&self.base.outer.ccw())?)];
        for f in &self.flanges {
            polys.push(dedup(&flat_polyline(&self.zone(f).outer.ccw())?));
            if f.leg > 1e-9 {
                polys.push(dedup(&flat_polyline(&self.panel(f).outer.ccw())?));
            }
            flat_points(&polys)?;
        }
        let loops = merge_loops(&polys, &mut FlatWork::new())?;
        if loops.is_empty() {
            return Err(DocError::Invalid("empty flat pattern".into()));
        }
        Ok(loops)
    }

    /// What is cut out of the flat material: the cut-outs, and the pockets that together go
    /// through the whole thickness.
    fn cut_outs(&self) -> Vec<Region2> {
        let mut cuts = self.holes.clone();
        cuts.extend(self.through_pocket_regions(&mut FlatWork::new()).unwrap_or_default());
        cuts
    }

    /// The bends: angle, radius, direction, allowance and centre line (flat coordinates). A
    /// bend is listed where its centre line has material: one entry for every stretch that the
    /// cut-outs leave of it, all with the bend's `index`, and none for a bend they take whole.
    pub fn bends(&self) -> Vec<BendInfo> {
        let cuts = self.cut_outs();
        let mut out = Vec::new();
        for (i, f) in self.flanges.iter().enumerate() {
            let ba = self.allowance(f);
            // The centre line runs between the middles of the bend's two ends (a torn end is askew).
            let ((u0, u1), (e0, e1)) = (self.zone_span(f), self.zone_ends(f));
            let (u0, u1) = ((u0 + e0) / 2.0, (u1 + e1) / 2.0);
            let a = f.p + f.n * (ba / 2.0) + f.d * u0;
            for (s0, s1) in clear_of(a, f.d, u1 - u0, &cuts) {
                out.push(BendInfo {
                    index: i + 1,
                    angle_deg: f.angle.abs().to_degrees(),
                    radius: f.radius,
                    up: f.angle >= 0.0,
                    allowance: ba,
                    length: s1 - s0,
                    start: if s0 > 0.0 { a + f.d * s0 } else { a },
                    end: a + f.d * s1,
                });
            }
        }
        out
    }

    /// The flat bend zones, one per bend.
    pub fn bend_zones(&self) -> Vec<Region2> {
        self.flanges.iter().map(|f| self.zone(f)).collect()
    }

    /// Hole centres of a perforation field (#732), flat coordinates: a staggered (60°, odd rows shifted half a pitch)
    /// or square grid from the flat bounding box's lower-left corner plus margin + radius; a hole is kept where it is
    /// at least margin clear of the material's edges and cut-outs and of every bend zone.
    pub fn perforation_holes(&self, p: &SheetPerforation) -> Result<Vec<Vec2>> {
        let regions = self.flat_regions()?;
        let zones = self.bend_zones();
        let clear = p.diameter / 2.0 + p.margin;
        let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
        let mut add = |l: &Loop2| {
            let pts = l.polyline(1e-3);
            for k in 0..pts.len() {
                if let (Some(&a), Some(&b)) = (pts.get(k), pts.get((k + 1) % pts.len())) {
                    segs.push((a, b));
                }
            }
        };
        for r in &regions {
            add(&r.outer);
            r.holes.iter().for_each(&mut add);
        }
        zones.iter().for_each(|z| add(&z.outer));
        let (mut lo, mut hi) = (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
        for (a, _) in &segs {
            lo = Vec2::new(lo.x.min(a.x), lo.y.min(a.y));
            hi = Vec2::new(hi.x.max(a.x), hi.y.max(a.y));
        }
        if !(lo.x <= hi.x) || !(p.pitch > 0.0) {
            return Ok(Vec::new());
        }
        // Segments bucketed in a grid, so each hole only checks the edges near it.
        let cell = (2.0 * clear).max(p.pitch).max(1.0);
        let (nx, ny) = (((hi.x - lo.x) / cell) as usize + 1, ((hi.y - lo.y) / cell) as usize + 1);
        let mut grid: Vec<Vec<usize>> = vec![Vec::new(); nx.saturating_mul(ny)];
        let idx = |x: f64, y: f64| -> (usize, usize) {
            (((x - lo.x) / cell).floor().clamp(0.0, (nx - 1) as f64) as usize, ((y - lo.y) / cell).floor().clamp(0.0, (ny - 1) as f64) as usize)
        };
        for (i, (a, b)) in segs.iter().enumerate() {
            let (i0, j0) = idx(a.x.min(b.x) - clear, a.y.min(b.y) - clear);
            let (i1, j1) = idx(a.x.max(b.x) + clear, a.y.max(b.y) + clear);
            for j in j0..=j1 {
                for ii in i0..=i1 {
                    if let Some(c) = grid.get_mut(j * nx + ii) {
                        c.push(i);
                    }
                }
            }
        }
        let dist = |c: Vec2, (a, b): (Vec2, Vec2)| -> f64 {
            let (d, l) = (b - a, (b - a).dot(b - a));
            let t = if l > 0.0 { ((c - a).dot(d) / l).clamp(0.0, 1.0) } else { 0.0 };
            c.dist(a + d * t)
        };
        let dy = if p.square { p.pitch } else { p.pitch * 3f64.sqrt() / 2.0 };
        let mut out = Vec::new();
        let mut row = 0usize;
        loop {
            let y = lo.y + clear + row as f64 * dy;
            if y > hi.y - clear + 1e-9 {
                break;
            }
            let mut x = lo.x + clear + if !p.square && row % 2 == 1 { p.pitch / 2.0 } else { 0.0 };
            while x <= hi.x - clear + 1e-9 {
                let c = Vec2::new(x, y);
                let (i, j) = idx(x, y);
                let near = grid.get(j * nx + i).map(Vec::as_slice).unwrap_or(&[]);
                if regions.iter().any(|r| r.contains(c))
                    && !zones.iter().any(|z| z.contains(c))
                    && near.iter().all(|&k| segs.get(k).is_none_or(|&s| dist(c, s) >= clear - 1e-9))
                {
                    out.push(c);
                }
                x += p.pitch;
            }
            row += 1;
        }
        Ok(out)
    }

    /// Open area of all perforation fields (mm²): what they take out of the sheet's faces.
    pub fn perforated_area(&self) -> f64 {
        self.perforations
            .iter()
            .map(|p| self.perforation_holes(p).map_or(0.0, |h| h.len() as f64) * std::f64::consts::PI * (p.diameter / 2.0).powi(2))
            .sum()
    }

    /// Topology counted as Fusion does: a round hole is one face bounded by two closed circular
    /// edges (one vertex each), where the sewn solid splits it into arcs. Returns the change to
    /// the sewn (faces, edges, vertices).
    pub fn round_hole_correction(&self) -> (i64, i64, i64) {
        let (mut df, mut de, mut dv) = (0i64, 0i64, 0i64);
        let Ok(regions) = self.flat_regions() else { return (0, 0, 0) };
        for h in regions.iter().flat_map(|r| &r.holes) {
            let arcs: Vec<(Vec2, f64)> =
                h.segs.iter().filter_map(|s| if let Seg2::Arc { center, radius, .. } = s { Some((*center, *radius)) } else { None }).collect();
            let n = h.segs.len() as i64;
            let full =
                !arcs.is_empty() && arcs.len() == h.segs.len() && arcs.iter().all(|(c, r)| c.dist(arcs[0].0) < 1e-9 && (r - arcs[0].1).abs() < 1e-9);
            if full && n > 1 {
                df += 1 - n;
                de += 2 - 3 * n;
                dv += 2 - 2 * n;
            }
        }
        (df, de, dv)
    }

    /// Bounding size of the flat pattern (x, y in the flat frame).
    pub fn flat_size(&self) -> Result<(f64, f64)> {
        let loops: Vec<Vec<Vec2>> = self.flat_regions()?.iter().map(|r| r.outer.polyline(1e-3)).collect();
        let (mut lo, mut hi) = (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
        for p in loops.iter().flatten() {
            lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        Ok((hi.x - lo.x, hi.y - lo.y))
    }

    /// Panels with their regions and world placements: (flange index or None, region).
    fn panels(&self) -> Vec<(Option<usize>, Region2)> {
        let mut v = vec![(None, self.base.clone())];
        for (i, f) in self.flanges.iter().enumerate() {
            if f.leg > 1e-9 {
                v.push((Some(i), self.panel(f)));
            }
        }
        v
    }

    /// Where a world point of this sheet lay in `before`, the same sheet before a feature
    /// changed its folds: the point goes back to flat coordinates through the panel it lies on
    /// and is folded again the way `before` folded that material. A panel a fold made was part of
    /// the panel it was folded out of. `None` off every panel (on a bend).
    pub(crate) fn point_before(&self, before: &SheetBody, p: Vec3) -> Option<Vec3> {
        const TOL: f64 = 1e-4;
        for (pi, r) in self.panels() {
            let Some(inv) = mat_inverse(&self.world(pi)) else { continue };
            let q = apply_point(&inv, p);
            if q.z < -TOL || q.z > self.t + TOL {
                continue;
            }
            let q2 = Vec2::new(q.x, q.y);
            let pts = dedup(&r.outer.ccw().polyline(1e-3));
            if !point_in(&pts, q2) && !near_outline(&pts, q2, TOL) {
                continue;
            }
            // Panels `before` did not have yet came out of their parent's (bounded walk).
            let mut owner = pi;
            for _ in 0..self.flanges.len() {
                match owner {
                    Some(i) if i >= before.flanges.len() => owner = self.flanges.get(i).and_then(|f| f.parent),
                    _ => break,
                }
            }
            if owner.is_some_and(|i| i >= before.flanges.len()) {
                return None;
            }
            return Some(apply_point(&before.world(owner), q));
        }
        None
    }

    /// The panel edge nearest a world point: (panel, edge a, edge b in flat coordinates, on the
    /// top face?, distance).
    pub fn edge_at(&self, p: Vec3) -> Option<(Option<usize>, Vec2, Vec2, bool, f64)> {
        let mut best: Option<(Option<usize>, Vec2, Vec2, bool, f64)> = None;
        for (pi, r) in self.panels() {
            let w = self.world(pi);
            let Some(inv) = mat_inverse(&w) else { continue };
            let q = apply_point(&inv, p);
            let pts = r.outer.ccw().polyline(1e-3);
            let m = pts.len();
            for i in 0..m {
                let (Some(&a), Some(&b)) = (pts.get(i), pts.get((i + 1) % m)) else { continue };
                if a.dist(b) < 1e-9 {
                    continue;
                }
                for (z, top) in [(0.0, false), (self.t, true)] {
                    let q2 = Vec2::new(q.x, q.y);
                    let d = b - a;
                    let t = ((q2 - a).dot(d) / d.len2()).clamp(0.0, 1.0);
                    let dist = ((a + d * t) - q2).len().hypot(q.z - z);
                    if best.as_ref().is_none_or(|x| dist < x.4) {
                        best = Some((pi, a, b, top, dist));
                    }
                }
            }
        }
        best
    }

    /// What `cuts` (the sheet's cut-outs) leave of a panel's edge a→b: the stretch nearest `pick`
    /// (a world point), as distances from `a` along the edge; of two as near, the first. `None`
    /// where they have taken the edge whole.
    fn edge_left(&self, panel: Option<usize>, a: Vec2, b: Vec2, pick: Vec3, cuts: &[Region2]) -> Option<(f64, f64)> {
        let d = (b - a).normalized()?;
        if cuts.is_empty() {
            return Some((0.0, a.dist(b)));
        }
        let left = clear_of(a + Vec2::new(-d.y, d.x) * EDGE_INSET, d, a.dist(b), cuts);
        let at = mat_inverse(&self.world(panel)).map_or(0.0, |inv| {
            let q = apply_point(&inv, pick);
            (Vec2::new(q.x, q.y) - a).dot(d)
        });
        let away = |s: &(f64, f64)| (s.0 - at).max(at - s.1).max(0.0);
        left.into_iter().min_by(|x, y| away(x).total_cmp(&away(y)))
    }

    /// Trim a panel's edge a→b back by `by` (toward the panel interior). Where a→b is what a
    /// cut-out left of a longer edge, `cut_at` says at which of its ends (start, end) the
    /// cut-out is: the base keeps its edge beyond such an end. A flange panel's far edge moves
    /// whole.
    fn trim(&mut self, panel: Option<usize>, a: Vec2, b: Vec2, by: f64, cut_at: (bool, bool)) -> Result<()> {
        if by.abs() < 1e-12 {
            return Ok(());
        }
        let d = (b - a).normalized().ok_or_else(|| DocError::Invalid("edge".into()))?;
        // Interior side of a counter-clockwise loop is to the left.
        let inward = Vec2::new(-d.y, d.x);
        match panel {
            None => {
                let pts = self.base.outer.ccw().polyline(1e-3);
                let o = a + inward * by;
                let (mut cut, on_line) = clip_at_line(&pts, o, inward);
                // Only the material along this edge goes: other parts of the outline past the
                // line (tabs side by side on one line) stay, and so does the base where it runs
                // on past the edge's own line (beyond an inside corner at the edge's end).
                let mut work = FlatWork::new();
                let off = merge_loops(&[clip_at_line(&pts, o, inward * -1.0).0], &mut work)?;
                let mid = (a + b) * 0.5;
                let touches = |p: &Vec<Vec2>| {
                    let m = p.len();
                    (0..m).filter_map(|i| Some((*p.get(i)?, *p.get((i + 1) % m)?))).any(|(p0, p1)| {
                        let e = p1 - p0;
                        let t = if e.len2() > 1e-18 { ((mid - p0).dot(e) / e.len2()).clamp(0.0, 1.0) } else { 0.0 };
                        (p0 + e * t).dist(mid) < 1e-6
                    })
                };
                let mut keep = Vec::new();
                // What goes, as strips to cut out of an outline with arcs or curves.
                let mut taken = Vec::new();
                for (k, p) in off.iter().enumerate() {
                    if touches(p) {
                        let (kept, between) = kept_past_edge(p, a, b, cut_at, &mut work)?;
                        keep.extend(kept);
                        taken.push(taken_strip(&off, k, a, b, by, between));
                    } else {
                        keep.push(p.clone());
                    }
                }
                // Merged, a run along the trimmed line out and back cancels: the base ends on
                // the line there, as it does beside a lower ledge.
                if !keep.is_empty() || on_line {
                    keep.insert(0, cut);
                    cut = match merge_loops(&keep, &mut work)?.as_slice() {
                        [one] => one.clone(),
                        [] => Vec::new(),
                        _ => return Err(DocError::Invalid("not supported yet: a flange setback that splits the sheet".into())),
                    };
                }
                if cut.len() < 3 {
                    return Err(DocError::Invalid("the flange's setback removes the whole panel".into()));
                }
                // An outline of lines is its own polyline. One with arcs or curves keeps them: the
                // strips are cut out of the outline as drawn.
                let outer = self.base.outer.ccw();
                let exact = if outer.segs.iter().all(|s| matches!(s, Seg2::Line { .. })) {
                    None
                } else {
                    taken.into_iter().collect::<Option<Vec<_>>>().and_then(|strips| trimmed_exactly(&outer, &strips, &cut))
                };
                self.base = Region2 { outer: exact.unwrap_or_else(|| Loop2::polygon(&cut).ccw()), holes: self.base.holes.clone() };
            }
            Some(i) => {
                // A flange panel: its far edge shortens the leg; side edges are not trimmed.
                let Some(f) = self.flanges.get_mut(i) else { return Err(DocError::Invalid("flange".into())) };
                if inward.dot(f.n * -1.0) > 0.99 {
                    f.leg -= by;
                    if f.leg < -1e-9 {
                        return Err(DocError::Invalid("the flange's setback is longer than the panel".into()));
                    }
                } else {
                    return Err(DocError::Invalid("not supported yet: a flange on the side of another flange".into()));
                }
            }
        }
        Ok(())
    }

    /// Add an edge flange on the panel edge nearest `pick` (world): `height` to the outer face
    /// from the virtual sharp, `angle` (0…180°), inside / outside / middle bend position.
    pub fn add_flange(&mut self, pick: Vec3, height: f64, angle: f64, radius: f64, position: &str, flip: bool) -> Result<usize> {
        self.add_flanges(&[pick], height, angle, radius, position, flip)?.into_iter().next().ok_or_else(|| DocError::Invalid("flange".into()))
    }

    /// Edge flanges on several edges at once: every edge is trimmed back first, so flanges on
    /// adjacent edges stop at each other's bends; the corners between them are then closed with
    /// the rule's gap and given the rule's relief (`close_corners`).
    pub fn add_flanges(&mut self, picks: &[Vec3], height: f64, angle: f64, radius: f64, position: &str, flip: bool) -> Result<Vec<usize>> {
        self.add_flanges_noted(picks, height, angle, radius, position, flip).map(|(made, _)| made)
    }

    /// [`SheetBody::add_flanges`], also returning notes on corners that could not be closed
    /// (they are left open, as the bends leave them) or relieved (they get no relief).
    pub fn add_flanges_noted(
        &mut self,
        picks: &[Vec3],
        height: f64,
        angle: f64,
        radius: f64,
        position: &str,
        flip: bool,
    ) -> Result<(Vec<usize>, Vec<String>)> {
        let each: Vec<EdgeFlange> = picks.iter().map(|&pick| EdgeFlange { pick, height, angle }).collect();
        self.add_flanges_each(&each, radius, position, flip, ReliefOverride::default())
    }

    /// Edge flanges that each have their own height and angle, in one go: every edge is trimmed
    /// back by its own setback first, then the flanges are added and the corners between them
    /// are closed. The closing works from the two flanges it joins (each one's own fold and
    /// leg), so unequal flanges close like equal ones. A flange on another flange's far edge (a
    /// return) runs along that edge end to end, following its parent's closed end; two of them
    /// whose parents meet at a corner are closed toward each other as well, and one alone at a
    /// corner is kept clear of the flange beside its parent (`close_return_corners`). A bend on
    /// the base that ends beside more of the base gets the rule's bend relief there
    /// (`relieve_bend_ends`). Where cut-outs have taken part of a picked edge away, the flange
    /// runs along the stretch they leave nearest the pick and takes its setback off that stretch
    /// only; an edge they took whole is refused. The corners between these flanges are relieved
    /// as the rule says, except where `relief` names another shape or size.
    pub fn add_flanges_each(
        &mut self,
        edges: &[EdgeFlange],
        radius: f64,
        position: &str,
        flip: bool,
        relief: ReliefOverride,
    ) -> Result<(Vec<usize>, Vec<String>)> {
        if edges.is_empty() || edges.len() > 1000 {
            return Err(DocError::Invalid("pick 1…1000 edges".into()));
        }
        // With several edges a message says which one it is about.
        let on = |k: usize| if edges.len() > 1 { format!("edge {}: ", k + 1) } else { String::new() };
        // Each edge's setback and leg (the panel past the bend) follow its own angle and height.
        let mut sizes = Vec::with_capacity(edges.len());
        for (k, e) in edges.iter().enumerate() {
            if !(e.angle > 1e-6 && e.angle <= std::f64::consts::PI - 1e-6) {
                return Err(DocError::Invalid(format!("{}the flange angle must be between 0 and 180°", on(k))));
            }
            let sharp = (radius + self.t) * (e.angle / 2.0).tan();
            let setback = match position {
                "inside" | "" => sharp,
                "outside" => 0.0,
                "middle" | "mid" => sharp / 2.0,
                other => return Err(DocError::Invalid(format!("unknown bend position `{other}` (inside, outside or middle)"))),
            };
            let leg = e.height - sharp;
            if !(leg >= -1e-9) {
                return Err(DocError::Invalid(format!("{}the flange height must be at least {sharp:.3} mm for this bend", on(k))));
            }
            sizes.push((setback, leg));
        }
        // Find every edge before anything moves: of an edge that cut-outs have taken part of, the
        // stretch they leave nearest the pick, and at which of its ends a cut-out is.
        let cuts = self.cut_outs();
        let mut found = Vec::new();
        for (k, (e, &(setback, leg))) in edges.iter().zip(&sizes).enumerate() {
            let (panel, a, b, top, dist) = self.edge_at(e.pick).ok_or_else(|| DocError::Invalid("no sheet edge there".into()))?;
            if dist > self.t * 2.0 + 1e-3 {
                return Err(DocError::Invalid("pick an edge of the sheet's top or bottom face".into()));
            }
            let d = (b - a).normalized().ok_or_else(|| DocError::Invalid("edge".into()))?;
            let (s0, s1) = self
                .edge_left(panel, a, b, e.pick, &cuts)
                .ok_or_else(|| DocError::Invalid(format!("{}a cut-out has taken that edge away; pick an edge the sheet still has", on(k))))?;
            let cut = (s0 > 1e-9, s1 < a.dist(b) - 1e-9);
            let (a, b) = (if cut.0 { a + d * s0 } else { a }, if cut.1 { a + d * s1 } else { b });
            found.push((panel, a, b, d, top, setback, leg, e.angle, cut));
        }
        for (panel, a, b, _, _, setback, _, _, cut) in &found {
            self.trim(*panel, *a, *b, *setback, *cut)?;
        }
        let mut made = Vec::new();
        // Whether a cut-out has freed each new bend's ends (start, end) of the base beside them.
        let mut freed = Vec::new();
        for (panel, a, b, d, top, setback, leg, angle, cut) in found {
            let outward = Vec2::new(d.y, -d.x);
            let line_o = a - outward * setback;
            // The trimmed edge: along d, on the moved line.
            let (start, len) = match panel {
                None => {
                    let pts = self.base.outer.ccw().polyline(1e-3);
                    let m = pts.len();
                    let on_line: Vec<(Vec2, f64)> = (0..m)
                        .filter_map(|k| {
                            let (p0, p1) = (*pts.get(k)?, *pts.get((k + 1) % m)?);
                            let e = p1 - p0;
                            let on = (p0 - line_o).dot(outward).abs() < 1e-6 && (p1 - line_o).dot(outward).abs() < 1e-6;
                            (on && e.dot(d) > 0.0).then_some((p0, e.len()))
                        })
                        .collect();
                    // Of several edges on the line (tabs side by side), the one the pick was on.
                    let (ua, ub) = (a.dot(d), b.dot(d));
                    on_line
                        .iter()
                        .find(|(p0, len)| (p0.dot(d) + len).min(ub) - p0.dot(d).max(ua) > 1e-9)
                        .or_else(|| on_line.first())
                        .copied()
                        .ok_or_else(|| DocError::Invalid("the flange's edge was trimmed away".into()))?
                }
                Some(i) => {
                    let f = self.flanges.get(i).ok_or_else(|| DocError::Invalid("flange".into()))?;
                    let tip = f.p + f.n * (self.allowance(f) + f.leg);
                    if f.shape.is_some() {
                        let u0 = (line_o - f.p).dot(f.d).clamp(0.0, f.len);
                        (tip + f.d * u0, f.len)
                    } else {
                        // The panel's far edge, end to end: where a closed corner ran the panel on
                        // or cut it back (slanted or not), the flange on it follows.
                        let (lo, hi) = self.far_edge(f);
                        if !(hi - lo > 1e-9) {
                            return Err(DocError::Invalid("the flange's edge was trimmed away".into()));
                        }
                        (tip + f.d * lo, hi - lo)
                    }
                }
            };
            // Where a cut-out ends the edge, the flange ends with it.
            let (start, len) = if cut.0 || cut.1 {
                let u = (start - a).dot(d);
                let (u0, u1) = (if cut.0 { u.max(0.0) } else { u }, if cut.1 { (u + len).min(a.dist(b)) } else { u + len });
                if !(u1 - u0 > 1e-9) {
                    return Err(DocError::Invalid("the flange's edge was trimmed away".into()));
                }
                (start + d * (u0 - u), u1 - u0)
            } else {
                (start, len)
            };
            // Beside such an end the base keeps the strip between its edge and the bend line: the
            // bend is free of it where the cut-outs took all of that strip there.
            let taken = |from: Vec2| clear_of(from, outward * -1.0, setback, &cuts).is_empty();
            freed.push((cut.0 && taken(a - d * 1e-4), cut.1 && taken(b + d * 1e-4)));
            let up = top != flip;
            self.flanges.push(SheetFlange {
                parent: panel,
                p: snap(start),
                d,
                len,
                n: outward,
                angle: if up { angle } else { -angle },
                radius,
                leg: leg.max(0.0),
                shape: None,
                exact: None,
                tag: String::new(),
                ext: (0.0, 0.0),
                slope: (0.0, 0.0),
                tear: (false, false),
            });
            made.push(self.flanges.len() - 1);
        }
        let mut notes = Vec::new();
        for (&i, &free) in made.iter().zip(&freed) {
            notes.extend(self.relieve_bend_ends(i, free));
        }
        notes.extend(self.close_corners(&made, relief));
        notes.extend(self.close_return_corners(&made));
        Ok((made, notes))
    }

    /// Where the bend of flange `i` on the base ends beside more of the base (the flange's edge
    /// ends at an inside corner and the base runs on past it), cut the rule's bend relief into
    /// the base there: a slot `relief_width` wide beside the bend's end, from `relief_depth`
    /// before the bend line to `relief_width` past the flange's far edge in the flat pattern,
    /// so that the bend and the flange come free of the base, folded and flat. An end that a
    /// cut-out has freed already (`free`: start, end) gets none. Returns a note for every other
    /// such end left as it is.
    fn relieve_bend_ends(&mut self, i: usize, free: (bool, bool)) -> Vec<String> {
        let Some(f) = self.flanges.get(i).cloned() else { return Vec::new() };
        if f.parent.is_some() {
            return Vec::new();
        }
        let (width, depth) = (self.bend_relief.0, self.bend_relief.1.max(0.0));
        let reach = self.allowance(&f) + f.leg;
        let (u0, u1) = self.zone_span(&f);
        let mut notes = Vec::new();
        for (u, s, free) in [(u0, -1.0, free.0), (u1, 1.0, free.1)] {
            // `du` past this end of the bend, `dv` past the bend line (into the flange).
            let at = |du: f64, dv: f64| f.p + f.d * (u + s * du) + f.n * dv;
            if free || !self.base.contains(at(1e-4, 1e-4)) {
                continue;
            }
            let cut = if !(width >= MIN_BEND_RELIEF) {
                Err(format!("the relief width {width:.4} mm is below {MIN_BEND_RELIEF} mm"))
            } else if !self.base.contains(at(width + 1e-4, 1e-4)) {
                Err(format!("a relief {width:.3} mm wide would take away all of the base beside the bend"))
            } else {
                let relief = rect(at(0.0, -depth), at(width, -depth), at(width, reach + width), at(0.0, reach + width));
                planar_region(&self.base)
                    .and_then(|base| material_boolean(&[base], &[relief], kernel::BoolOp::Cut, &mut FlatWork::new()))
                    .map_err(|e| e.to_string())
                    .and_then(|mut left| match (left.pop(), left.is_empty()) {
                        (Some(base), true) if base.holes.is_empty() => Ok(base),
                        _ => Err("it would not leave the base in one piece".to_string()),
                    })
            };
            match cut {
                Ok(base) => self.base = base,
                Err(why) => {
                    let w = apply_point(&self.frame_mat(), v3(at(0.0, 0.0)));
                    notes.push(format!("no bend relief at ({:.3}, {:.3}, {:.3}): {why}", w.x, w.y, w.z));
                }
            }
        }
        notes
    }

    /// Flanges on the base that meet at a convex corner of it, at least one of them among `among`:
    /// (a, a_end, b, b_end, the corner). Their bend lines cross at the corner and both bends end
    /// there, or both the same distance short of it: where a corner relief was cut, or where the
    /// base's outline cuts the corner off.
    fn base_pairs(&self, among: &[usize]) -> Vec<(usize, bool, usize, bool, Vec2)> {
        let limit = std::f64::consts::PI - 1e-6;
        // Every bend end on the base: (flange, which end, the point, the way into the bend from it).
        let ends: Vec<(usize, bool, Vec2, Vec2)> = self
            .flanges
            .iter()
            .enumerate()
            .filter(|(_, f)| f.parent.is_none() && f.shape.is_none() && f.angle.abs() < limit)
            .flat_map(|(i, f)| {
                let (u0, u1) = self.zone_span(f);
                [(i, false, f.p + f.d * u0, f.d), (i, true, f.p + f.d * u1, f.d * -1.0)]
            })
            .collect();
        let mut out = Vec::new();
        for (k, &(a, a_end, za, ea)) in ends.iter().enumerate() {
            for &(b, b_end, zb, eb) in ends.iter().skip(k + 1) {
                if a == b || !(among.contains(&a) || among.contains(&b)) {
                    continue;
                }
                let (Some(fa), Some(fb)) = (self.flanges.get(a), self.flanges.get(b)) else { continue };
                let cross = ea.cross(eb);
                // Parallel bends don't cross; at an inside corner the flanges turn away from each other.
                if cross.abs() < 1e-9 || ea.dot(fb.n) > -1e-9 || eb.dot(fa.n) > -1e-9 {
                    continue;
                }
                // The corner c where the bend lines cross: za = c + ea·δa, zb = c + eb·δb.
                let r = za - zb;
                let (da, db) = (r.cross(eb) / cross, r.cross(ea) / cross);
                if !(da > -1e-6 && (da - db).abs() < 1e-6) {
                    continue;
                }
                let c = za - ea * da;
                if da > 1e-6 {
                    // Short of the crossing. Where both flanges' own edges run to it, a corner relief
                    // took the bends' ends away after the flanges were closed there: their corner,
                    // however deep the relief. Otherwise the base is cut off between the bends, which
                    // is a corner only while the cut is no deeper than the flanges reach and no other
                    // flange's bend ends in it; else it is an edge of the base (the top of a trapezoid
                    // whose sides cross above it) and the flanges on its two sides don't meet.
                    let whole = |f: &SheetFlange, at_end: bool| (f.p + f.d * if at_end { f.len } else { 0.0 }).dist(c) < 1e-6;
                    let relieved = whole(fa, a_end) && whole(fb, b_end);
                    let reach = |f: &SheetFlange| self.allowance(f) + f.leg;
                    let between = || ends.iter().any(|&(g, _, z, _)| g != a && g != b && in_triangle(z, za, c, zb));
                    if !relieved && (da > reach(fa).min(reach(fb)) || between()) {
                        continue;
                    }
                }
                out.push((a, a_end, b, b_end, c));
            }
        }
        out
    }

    /// The flange of `made` on flange `i`'s far edge whose bend ends at that edge's end (the one
    /// at `p`, or the other one when `at_end`), with the point where its bend ends there.
    fn on_far_end(&self, i: usize, at_end: bool, made: &[usize]) -> Option<(usize, Vec2)> {
        let limit = std::f64::consts::PI - 1e-6;
        let f = self.flanges.get(i).filter(|f| f.shape.is_none())?;
        let (lo, hi) = self.far_edge(f);
        let tip = self.allowance(f) + f.leg;
        made.iter().find_map(|&j| {
            let g = self.flanges.get(j).filter(|g| g.parent == Some(i) && g.shape.is_none() && g.angle.abs() < limit && g.d.dot(f.d) > 1.0 - 1e-9)?;
            let c = if at_end { g.p + g.d * g.len } else { g.p };
            let (s, u) = ((c - f.p).dot(f.n), (c - f.p).dot(f.d));
            ((s - tip).abs() < 1e-6 && (u - if at_end { hi } else { lo }).abs() < 1e-6).then_some((j, c))
        })
    }

    /// Finish the new flanges that stand on the far edges of flanges meeting at a convex corner of
    /// the base (returns on the walls of a tray), where their bends reach their parents' ends.
    /// Two of them meeting at a corner are closed toward each other ([`SheetBody::close_returns`]);
    /// as on the base, a flange of an earlier feature is left as it is. One that is alone at a
    /// corner, or passes the other, is kept clear of the flange beside its parent
    /// ([`SheetBody::clear_of_neighbour`]). Returns a note for every corner left open.
    fn close_return_corners(&mut self, made: &[usize]) -> Vec<String> {
        let mut parents: Vec<usize> = made.iter().filter_map(|&j| self.flanges.get(j)?.parent).collect();
        parents.sort_unstable();
        parents.dedup();
        let mut notes = Vec::new();
        if parents.is_empty() {
            return notes;
        }
        // A point of flange `on`'s panel, in the part's coordinates, for a note.
        let at = |s: &SheetBody, on: usize, c: Vec2| {
            let w = apply_point(&s.world(Some(on)), v3(c));
            format!("({:.3}, {:.3}, {:.3})", w.x, w.y, w.z)
        };
        for (a, a_end, b, b_end, c) in self.base_pairs(&parents) {
            let (x, y) = (self.on_far_end(a, a_end, made), self.on_far_end(b, b_end, made));
            if let (Some((x, ca)), Some((y, cb))) = (x, y) {
                match self.close_returns(&ReturnCorner { a: x, a_end, b: y, b_end, ca, cb }) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(why) => {
                        notes.push(format!("the flanges at {} are left open: {why}", at(self, a, ca)));
                        continue;
                    }
                }
            }
            for (on, at_end, own, other) in [(x, a_end, a, b), (y, b_end, b, a)] {
                let Some((i, ci)) = on else { continue };
                if let Err(why) = self.clear_of_neighbour(i, at_end, ci, own, other, c) {
                    notes.push(format!("the flange at {} is not clear of the flange beside it: {why}", at(self, own, ci)));
                }
            }
        }
        notes
    }

    /// Close two flanges on flanges that meet at a corner (returns on two walls that meet): mitre
    /// them on the plane halfway between them, the rule's gap apart. That plane runs through the
    /// point where the two flanges' inner start lines cross (seen along their panels' mean normal),
    /// along that normal, and bisects the two bend lines; returns lying in one plane get the
    /// vertical mitre of a picture frame, whatever their parents' angles. Each panel ends square
    /// to the sheet on a line in the flat, slanted as the mitre makes it, so that all of it keeps
    /// gap/2 from the plane. A bend can't end at a slant like that (cut that way in the flat, its
    /// folded end is not a plane), so each bend stops square where all of it keeps gap/2 from the
    /// plane, never past its parent's end; a short stretch of the parent's edge can stay bare there.
    /// Returns whether the two met: one turned in and one turned out pass each other untouched.
    fn close_returns(&mut self, k: &ReturnCorner) -> std::result::Result<bool, String> {
        let (Some(fa), Some(fb)) = (self.flanges.get(k.a).cloned(), self.flanges.get(k.b).cloned()) else {
            return Err("a flange is missing".into());
        };
        let (wa, wb) = (self.fold(k.a), self.fold(k.b));
        // Inner faces (toward the bends' axes) and their normals, pointing from the outer face in.
        let inner = |f: &SheetFlange| if f.angle >= 0.0 { (self.t, 1.0) } else { (0.0, -1.0) };
        let ((za, sa), (zb, sb)) = (inner(&fa), inner(&fb));
        let (na, nb) = (apply_vector(&wa, Vec3::Z) * sa, apply_vector(&wb, Vec3::Z) * sb);
        let Some(up) = (na + nb).normalized() else { return Err("the flanges face each other".into()) };
        let across = |v: Vec3| (v - up * v.dot(up)).normalized();
        let ea = if k.a_end { fa.d * -1.0 } else { fa.d };
        let eb = if k.b_end { fb.d * -1.0 } else { fb.d };
        let (Some(ea3), Some(eb3), Some(ta), Some(tb)) = (
            across(apply_vector(&wa, v3(ea))),
            across(apply_vector(&wb, v3(eb))),
            across(apply_vector(&wa, v3(fa.n))),
            across(apply_vector(&wb, v3(fb.n))),
        ) else {
            return Err("a flange stands square to the other one".into());
        };
        // They meet when both panels lie toward the other's bend (inward returns overlap) or both
        // away from it (outward ones leave a V). One each way: they pass each other, nothing to do.
        if ta.dot(eb3) * tb.dot(ea3) < -1e-12 {
            return Ok(false);
        }
        let Some(m) = (ea3 - eb3).normalized() else { return Err("the flanges are parallel".into()) };
        let gap = self.gap;
        if !(gap >= MIN_CORNER_GAP) {
            return Err(format!("the flange gap {gap:.4} mm is below {MIN_CORNER_GAP} mm"));
        }
        // Where the inner start lines cross, seen along `up` (the midpoint if they run in line).
        let start = |f: &SheetFlange, w: &Mat, c: Vec2, z: f64| apply_point(w, v3(c + f.n * self.allowance(f)) + Vec3::Z * z);
        let (pa, pb) = (start(&fa, &wa, k.ca, za), start(&fb, &wb, k.cb, zb));
        let side = up.cross(ea3);
        let den = eb3.dot(side);
        let x = if den.abs() > 1e-9 { pb + eb3 * ((pa - pb).dot(side) / den) } else { (pa + pb) * 0.5 };
        let mut new = [fa.clone(), fb.clone()];
        for (f, (w, c, e, at_end, sign)) in new.iter_mut().zip([(&wa, k.ca, ea, k.a_end, 1.0), (&wb, k.cb, eb, k.b_end, -1.0)]) {
            // Signed distance from the mitre plane, positive on this flange's side. It is affine in
            // (u, s, z): u from the bend's end along the bend, s from the bend's start across it.
            let dist = |p: Vec3| sign * (p - x).dot(m);
            let at = |u: f64, s: f64, z: f64| dist(apply_point(w, v3(c + e * u + f.n * s) + Vec3::Z * z));
            let c0 = at(0.0, 0.0, 0.0);
            let (cu, cs, cz) = (at(1.0, 0.0, 0.0) - c0, at(0.0, 1.0, 0.0) - c0, at(0.0, 0.0, 1.0) - c0);
            if !(cu > 1e-6) || ![c0, cs, cz].iter().all(|v| v.is_finite()) {
                return Err("the flanges don't meet across the corner".into());
            }
            let ba = self.allowance(f);
            // The bend: along it the distance grows as on the panel; at its end take the least over
            // the bend's cross-section.
            let u_bend = ((gap / 2.0 - self.zone_least(f, c, &dist)) / cu).max(0.0);
            let u_panel = (gap / 2.0 - c0 - (cz * self.t).min(0.0) - cs * ba) / cu;
            if !(u_bend.is_finite() && u_panel.is_finite()) {
                return Err("the flanges don't meet across the corner".into());
            }
            // Panel and bend ending together stay one straight edge.
            let u_panel = if (u_panel - u_bend).abs() < 1e-9 { u_bend } else { u_panel };
            let slope = if (cs / cu).abs() < 1e-12 { 0.0 } else { cs / cu };
            if !at_end {
                f.p += f.d * u_bend;
            }
            f.len -= u_bend;
            if !(f.len > 1e-6) {
                return Err("the gap would cut a bend away".into());
            }
            // The panel runs on past its bend's (new) end toward the corner, or is cut back.
            if at_end {
                f.ext.1 = u_bend - u_panel;
                f.slope.1 = slope;
            } else {
                f.ext.0 = u_bend - u_panel;
                f.slope.0 = slope;
            }
            let bottom = f.len + f.ext.0 + f.ext.1;
            let top = bottom + (f.slope.0 + f.slope.1) * f.leg;
            if f.leg > 1e-9 && !(bottom > 1e-6 && top > 1e-6) {
                return Err("the gap would cut a panel away".into());
            }
        }
        // The flat pattern must stay one sheet without overlaps.
        let [new_a, new_b] = new;
        let pieces = |s: &SheetBody, f: &SheetFlange| {
            let mut v = vec![corners_of(&s.zone(f))];
            if f.leg > 1e-9 {
                v.push(corners_of(&s.panel(f)));
            }
            v
        };
        let mine = [pieces(self, &new_a), pieces(self, &new_b)];
        let overlap = |p: &[Vec<Vec2>], q: &[Vec<Vec2>]| p.iter().any(|a| q.iter().any(|b| pieces_overlap(a, b)));
        let [mine_a, mine_b] = &mine;
        let clash = overlap(mine_a, mine_b)
            || self.flanges.iter().enumerate().any(|(j, g)| {
                j != k.a && j != k.b && g.shape.is_none() && {
                    let other = pieces(self, g);
                    mine.iter().any(|p| overlap(p, &other))
                }
            });
        if clash {
            return Err("closed, the flanges would overlap in the flat pattern".into());
        }
        if let Some(f) = self.flanges.get_mut(k.a) {
            *f = new_a;
        }
        if let Some(f) = self.flanges.get_mut(k.b) {
            *f = new_b;
        }
        Ok(true)
    }

    /// Keep flange `i`, which stands on the far edge of flange `own` and reaches its end at a
    /// corner of the base, clear of the flange `other` that `own` meets there (`c`: where their
    /// bend lines cross; `ci`: where flange `i`'s bend ends). A wall that leans in over the base
    /// also leans in over a return on the wall beside it. `own`'s end keeps half the gap from the
    /// plane halfway between the two walls' inner faces, which leaves its nearest edge
    /// |nb − na|·gap/2 from `other`'s inner face (na, nb: the inner faces' normals). Flange `i` is
    /// cut back until what has passed `own`'s inner face keeps that distance too: its panel, its
    /// bend's inner face, and its bend's outer face from the turn where that face crosses the
    /// plane (the rest lies in `own`'s thickness, on `own`'s side of the corner as `own`'s end
    /// does). Its bend stops square and its panel ends on a line in the flat; it is never run
    /// on. Only a flange that turns the way its parent did lies on that side of it.
    fn clear_of_neighbour(&mut self, i: usize, at_end: bool, ci: Vec2, own: usize, other: usize, c: Vec2) -> std::result::Result<(), String> {
        let (Some(mut f), Some(fa), Some(fb)) = (self.flanges.get(i).cloned(), self.flanges.get(own), self.flanges.get(other)) else {
            return Err("a flange is missing".into());
        };
        let up = fa.angle >= 0.0;
        if (fb.angle >= 0.0) != up || (f.angle >= 0.0) != up {
            return Ok(());
        }
        let (z_in, sign) = if up { (self.t, 1.0) } else { (0.0, -1.0) };
        let (wb, w) = (self.fold(other), self.fold(i));
        let (na, nb) = (apply_vector(&self.fold(own), Vec3::Z) * sign, apply_vector(&wb, Vec3::Z) * sign);
        let pb = apply_point(&wb, v3(c) + Vec3::Z * z_in);
        let keep = (nb - na).len() * self.gap.max(0.0) / 2.0;
        // How far a point is clear: affine in (u, s, z), u from the bend's end along the bend, s
        // from the bend's start across it.
        let clear = |p: Vec3| nb.dot(p - pb) - keep;
        let e = if at_end { f.d * -1.0 } else { f.d };
        let at = |u: f64, s: f64, z: f64| clear(apply_point(&w, v3(ci + e * u + f.n * s) + Vec3::Z * z));
        let c0 = at(0.0, 0.0, 0.0);
        let (cu, cs, cz) = (at(1.0, 0.0, 0.0) - c0, at(0.0, 1.0, 0.0) - c0, at(0.0, 0.0, 1.0) - c0);
        if !(cu > 1e-6) || ![c0, cs, cz].iter().all(|v| v.is_finite()) {
            return Ok(());
        }
        let ba = self.allowance(&f);
        // The bend's outer face is R + T from the axis and starts T outside `own`'s inner face: it
        // crosses that plane at the turn φ with (R + T)·(1 − cos φ) = T, or not before its end.
        let th = f.angle.abs();
        let from = (f.radius / (f.radius + self.t)).acos().min(th);
        let least = self.arc_least(&f, ci, z_in, (0.0, th), &clear).min(self.arc_least(&f, ci, self.t - z_in, (from, th), &clear));
        let u_bend = (-least / cu).max(0.0);
        // The panel's end is one line in the flat: through what its start and its far edge need
        // (on the face that comes nearer), neither less than nothing.
        let worst = c0 + (cz * self.t).min(0.0);
        let (u0, u1) = ((-(worst + cs * ba) / cu).max(0.0), (-(worst + cs * (ba + f.leg)) / cu).max(0.0));
        if !(u_bend > 1e-9 || u0 > 1e-9 || u1 > 1e-9) {
            return Ok(());
        }
        // Panel and bend ending together stay one straight edge.
        let u0 = if (u0 - u_bend).abs() < 1e-9 { u_bend } else { u0 };
        let slope = if f.leg > 1e-9 && (u0 - u1).abs() > 1e-12 { (u0 - u1) / f.leg } else { 0.0 };
        if !at_end {
            f.p += f.d * u_bend;
        }
        f.len -= u_bend;
        if !(f.len > 1e-6) {
            return Err("that would cut its bend away".into());
        }
        if at_end {
            f.ext.1 = u_bend - u0;
            f.slope.1 = slope;
        } else {
            f.ext.0 = u_bend - u0;
            f.slope.0 = slope;
        }
        let bottom = f.len + f.ext.0 + f.ext.1;
        let top = bottom + (f.slope.0 + f.slope.1) * f.leg;
        if f.leg > 1e-9 && !(bottom > 1e-6 && top > 1e-6) {
            return Err("that would cut its panel away".into());
        }
        if let Some(g) = self.flanges.get_mut(i) {
            *g = f;
        }
        Ok(())
    }

    /// The least of `g` (an affine function of a folded point) over the cross-section of flange
    /// `f`'s bend at `q0` on its start line: over its two faces, from the bend's start to its end.
    fn zone_least(&self, f: &SheetFlange, q0: Vec2, g: &dyn Fn(Vec3) -> f64) -> f64 {
        let whole = (0.0, f.angle.abs());
        self.arc_least(f, q0, 0.0, whole, g).min(self.arc_least(f, q0, self.t, whole, g))
    }

    /// The least of `g` (an affine function of a folded point) along the arc that the point `z`
    /// up in the sheet at `q0`, on the start line of flange `f`'s bend, runs through while the
    /// bend turns it from `turns.0` to `turns.1`. The bend turns the point about the bend axis,
    /// so `g` is a + b·cos φ + c·sin φ in the turn φ: its least is at an end of the arc or where
    /// that sinusoid bottoms out.
    fn arc_least(&self, f: &SheetFlange, q0: Vec2, z: f64, turns: (f64, f64), g: &dyn Fn(Vec3) -> f64) -> f64 {
        let parent = f.parent.map_or(IDENTITY, |i| self.fold(i));
        let (axis_o, k, turn) = self.bend_axis(f);
        let at = |phi: f64| g(apply_point(&mat_mul(&parent, &rot_about(axis_o, k, turn.signum() * phi)), v3(q0) + Vec3::Z * z));
        let (v0, v90, v180) = (at(0.0), at(std::f64::consts::FRAC_PI_2), at(std::f64::consts::PI));
        let (a, b) = ((v0 + v180) / 2.0, (v0 - v180) / 2.0);
        let c = v90 - a;
        let mut least = at(turns.0).min(at(turns.1));
        let mut phi = c.atan2(b) + std::f64::consts::PI;
        if phi >= std::f64::consts::TAU {
            phi -= std::f64::consts::TAU;
        }
        if phi > turns.0 && phi < turns.1 {
            least = least.min(a - b.hypot(c));
        }
        least
    }

    /// Corners where two of the new flanges' bends meet on the base: both bend zones end at the
    /// corner point. Hems and fold panels are left out, and so is a corner with a flange of an
    /// earlier feature: that one keeps its length and the new flange ends at its bend, as before.
    fn corners(&self, made: &[usize]) -> Vec<Corner> {
        let limit = std::f64::consts::PI - 1e-6;
        let ends: Vec<(usize, bool, Vec2)> = self
            .flanges
            .iter()
            .enumerate()
            .filter(|(_, f)| f.parent.is_none() && f.shape.is_none() && f.angle.abs() < limit)
            .flat_map(|(i, f)| {
                let (u0, u1) = self.zone_span(f);
                [(i, false, f.p + f.d * u0), (i, true, f.p + f.d * u1)]
            })
            .collect();
        let mut out = Vec::new();
        for (k, &(a, a_end, c)) in ends.iter().enumerate() {
            for &(b, b_end, c2) in ends.iter().skip(k + 1) {
                if a != b && c.dist(c2) < 1e-6 && made.contains(&a) && made.contains(&b) {
                    out.push(Corner { a, a_end, b, b_end, c });
                }
            }
        }
        out
    }

    /// Close the corners the new flanges make, then cut the relief at each (or tear it): the
    /// rule's, but for the shape or size `relief` names. Returns a note for every corner left
    /// open, without its relief or not torn.
    fn close_corners(&mut self, made: &[usize], relief: ReliefOverride) -> Vec<String> {
        let corners = self.corners(made);
        let mut notes = Vec::new();
        let mut closed = Vec::with_capacity(corners.len());
        for k in &corners {
            match self.close_corner(k) {
                Ok(()) => closed.push(*k),
                Err(why) => notes.push(format!("the flanges at {} are left open: {why}", self.shown(k.c))),
            }
        }
        notes.extend(self.relieve_corners(&corners, &closed, relief));
        notes
    }

    /// A point of the flat frame, in the part's coordinates, for a note.
    fn shown(&self, c: Vec2) -> String {
        let w = apply_point(&self.frame_mat(), v3(c));
        format!("({:.3}, {:.3}, {:.3})", w.x, w.y, w.z)
    }

    /// Cut the relief at each of `corners`, or tear those of them that are `closed`: the rule's,
    /// but for the shape or size `relief` names. Returns a note for every corner without its
    /// relief or not torn.
    fn relieve_corners(&mut self, corners: &[Corner], closed: &[Corner], relief: ReliefOverride) -> Vec<String> {
        let mut notes = Vec::new();
        let (shape, size) = (relief.shape.unwrap_or(self.relief), relief.size.unwrap_or(self.relief_size));
        match shape {
            ReliefShape::Notch => {}
            // The reliefs come last: they move the bends' ends, which is how the corners were found.
            ReliefShape::Round | ReliefShape::Rectangular => {
                for k in corners {
                    if let Err(why) = self.relieve_corner(k, corners, shape, size) {
                        notes.push(format!("no corner relief at {}: {why}", self.shown(k.c)));
                    }
                }
            }
            // A tear cuts to the flanges' closed ends, so a corner left open gets none.
            ReliefShape::Tear => {
                for k in closed {
                    if let Err(why) = self.tear_corner(k) {
                        notes.push(format!("the corner at {} keeps its notch: {why}", self.shown(k.c)));
                    }
                }
            }
        }
        notes
    }

    /// The plane halving the inner faces (toward the bends' axes) of the two flanges at a corner:
    /// the signed distance from it of a folded point, positive on flange a's side. `None` where
    /// the faces are parallel.
    fn halving_plane(&self, k: &Corner) -> Option<impl Fn(Vec3) -> f64 + use<>> {
        let (wa, wb) = (self.fold(k.a), self.fold(k.b));
        // The inner faces, with normals pointing from the outer face in.
        let (z_in, sign) = if self.flanges.get(k.a)?.angle >= 0.0 { (self.t, 1.0) } else { (0.0, -1.0) };
        let na = apply_vector(&wa, Vec3::Z) * sign;
        let nb = apply_vector(&wb, Vec3::Z) * sign;
        let pa = apply_point(&wa, v3(k.c) + Vec3::Z * z_in);
        let pb = apply_point(&wb, v3(k.c) + Vec3::Z * z_in);
        let m = (nb - na).len();
        (m > 1e-6).then_some(move |p: Vec3| (nb.dot(p - pb) - na.dot(p - pa)) / m)
    }

    /// Run two flanges' panels on toward each other (or cut them back) until their ends meet the
    /// plane halfway between the flanges with the rule's gap between them. The ends stay square
    /// to the sheet and follow lines in the flat, slanted when the flanges aren't at 90°, so that
    /// the edges nearest each other (on the inner faces) are `gap` apart along their length. Any
    /// angles, also unequal ones, at any convex corner of the base.
    fn close_corner(&mut self, k: &Corner) -> std::result::Result<(), String> {
        let (Some(fa), Some(fb)) = (self.flanges.get(k.a).cloned(), self.flanges.get(k.b).cloned()) else {
            return Err("a flange is missing".into());
        };
        let ea = if k.a_end { fa.d * -1.0 } else { fa.d };
        let eb = if k.b_end { fb.d * -1.0 } else { fb.d };
        // At an inside corner of the base the flanges turn away from each other.
        if ea.dot(fb.n) > -1e-9 || eb.dot(fa.n) > -1e-9 {
            return Ok(());
        }
        if fa.leg <= 1e-9 || fb.leg <= 1e-9 {
            // No panel past a bend: the bends meet at the corner, there is nothing to close.
            return Ok(());
        }
        if (fa.angle >= 0.0) != (fb.angle >= 0.0) {
            return Err("the flanges bend opposite ways".into());
        }
        let gap = self.gap;
        if !(gap >= MIN_CORNER_GAP) {
            return Err(format!("the flange gap {gap:.4} mm is below {MIN_CORNER_GAP} mm"));
        }
        let (wa, wb) = (self.fold(k.a), self.fold(k.b));
        // Signed distance from the plane bisecting the inner faces, positive on flange a's side.
        let Some(bisector) = self.halving_plane(k) else { return Err("the flanges are parallel".into()) };
        // A flange's end: its panel keeps u ≥ u_end(s) (u from the corner along its bend, s from
        // the bend's start across it), so that every point of the end face is gap/2 or more from
        // the bisecting plane. The distance is affine in (u, s, z), so the end is a line in the
        // flat: returns u_end at the panel's bend edge and how much further the panel runs on per
        // unit of height.
        let end = |f: &SheetFlange, w: &Mat, e: Vec2, sign: f64| -> std::result::Result<(f64, f64), String> {
            let at = |u: f64, s: f64, z: f64| sign * bisector(apply_point(w, v3(k.c + e * u + f.n * s) + Vec3::Z * z));
            let c0 = at(0.0, 0.0, 0.0);
            let (cu, cs, cz) = (at(1.0, 0.0, 0.0) - c0, at(0.0, 1.0, 0.0) - c0, at(0.0, 0.0, 1.0) - c0);
            if !(cu > 1e-6) || ![c0, cs, cz].iter().all(|x| x.is_finite()) {
                return Err("the flanges don't meet across the corner".into());
            }
            let ba = self.allowance(f);
            // At 90° the end is square to the bend: keep the panel an exact rectangle.
            let slope = if (cs / cu).abs() < 1e-12 { 0.0 } else { cs / cu };
            Ok(((gap / 2.0 - c0 - (cz * self.t).min(0.0) - cs * ba) / cu, slope))
        };
        let mut new = [fa.clone(), fb.clone()];
        for (f, (e, at_end, sign)) in new.iter_mut().zip([(ea, k.a_end, 1.0), (eb, k.b_end, -1.0)]) {
            let w = if sign > 0.0 { &wa } else { &wb };
            let (u_end, slope) = end(f, w, e, sign)?;
            let tc = (k.c - f.p).dot(f.d);
            if at_end {
                f.ext.1 = tc - u_end - f.len;
                f.slope.1 = slope;
            } else {
                f.ext.0 = -(tc + u_end);
                f.slope.0 = slope;
            }
            let bottom = f.len + f.ext.0 + f.ext.1;
            let top = bottom + (f.slope.0 + f.slope.1) * f.leg;
            if !(bottom > 1e-6 && top > 1e-6) {
                return Err("the gap would cut a panel away".into());
            }
        }
        // The flat pattern must stay one sheet without overlaps.
        let [new_a, new_b] = new;
        let (qa, qb) = (corners_of(&self.panel(&new_a)), corners_of(&self.panel(&new_b)));
        if convex_overlap(&qa, &qb) > 1e-9
            || convex_overlap(&qa, &corners_of(&self.zone(&fb))) > 1e-9
            || convex_overlap(&qb, &corners_of(&self.zone(&fa))) > 1e-9
        {
            return Err("closed, the flanges would overlap in the flat pattern".into());
        }
        if let Some(f) = self.flanges.get_mut(k.a) {
            *f = new_a;
        }
        if let Some(f) = self.flanges.get_mut(k.b) {
            *f = new_b;
        }
        Ok(())
    }

    /// Tear a closed corner (the rule's `tear`): nothing is cut out of the base, both bends
    /// keep running to the corner where their start lines meet, and each one's end is cut
    /// straight, in the flat, from that corner to where its panel's closed end starts (`zone`),
    /// instead of square across the bend and on along the panel's edge. The panels keep the ends
    /// `close_corner` gave them. Folded, such an end is a helix (`ZoneEdge::Curved`): the two
    /// bends touch at the corner and open to the rule's gap where the panels start. Refused, with
    /// the reason, when the bends would overlap in the flat pattern, or when folded a bend's end
    /// would cross the plane square to the base that halves the corner (into the other bend) or
    /// come nearer the plane halving the flanges' inner faces than a straight taper from where
    /// the corner lies to the half gap the panels keep; the corner then stays as it was closed.
    fn tear_corner(&mut self, k: &Corner) -> std::result::Result<(), String> {
        let (Some(mut fa), Some(mut fb)) = (self.flanges.get(k.a).cloned(), self.flanges.get(k.b).cloned()) else {
            return Err("a flange is missing".into());
        };
        let ea = if k.a_end { fa.d * -1.0 } else { fa.d };
        let eb = if k.b_end { fb.d * -1.0 } else { fb.d };
        // As for the closing: flanges at an inside corner of the base turn away from each other,
        // and a bend without a panel has no end to be cut to.
        if ea.dot(fb.n) > -1e-9 || eb.dot(fa.n) > -1e-9 || fa.leg <= 1e-9 || fb.leg <= 1e-9 {
            return Ok(());
        }
        for (f, at_end) in [(&mut fa, k.a_end), (&mut fb, k.b_end)] {
            if at_end {
                f.tear.1 = true;
            } else {
                f.tear.0 = true;
            }
        }
        let (za, zb) = (corners_of(&self.zone(&fa)), corners_of(&self.zone(&fb)));
        if pieces_overlap(&za, &zb) || pieces_overlap(&za, &corners_of(&self.panel(&fb))) || pieces_overlap(&zb, &corners_of(&self.panel(&fa))) {
            return Err("a tear would make the bends overlap in the flat pattern".into());
        }
        let (Some(half), Some(bisector)) = (v3(ea - eb).normalized(), self.halving_plane(k)) else {
            return Err("the flanges are parallel".into());
        };
        let (c, half_gap) = (v3(k.c), self.gap / 2.0);
        for (f, at_end, sign) in [(&fa, k.a_end, 1.0), (&fb, k.b_end, -1.0)] {
            // On a torn end a signed distance is a + b·φ + c·cos φ + d·sin φ in the turn φ, on
            // either face of the sheet: its least less a taper from `from` to `to` is exact.
            let th = f.angle.abs();
            let least = |g: &dyn Fn(Vec3) -> f64, from: f64, to: f64| {
                let over = |z: f64| sinusoid_least(&|phi: f64| g(self.bend_end(f, at_end, phi, z)) - (from + (to - from) * phi / th), th);
                over(0.0).min(over(self.t))
            };
            if least(&|p: Vec3| sign * (p - c).dot(half), 0.0, 0.0) < -1e-9 {
                return Err("a tear would run a bend into the other one".into());
            }
            let own = |p: Vec3| sign * bisector(p);
            let start = own(self.bend_end(f, at_end, 0.0, 0.0)).min(own(self.bend_end(f, at_end, 0.0, self.t))).min(half_gap);
            if least(&own, start, half_gap) < -1e-9 {
                return Err("a tear would bring the bends nearer each other than the flanges' ends are".into());
            }
        }
        if let Some(f) = self.flanges.get_mut(k.a) {
            *f = fa;
        }
        if let Some(f) = self.flanges.get_mut(k.b) {
            *f = fb;
        }
        Ok(())
    }

    /// Cut a relief `size` across where two bends meet at a corner of the base, `size / 2`
    /// from the corner along both edges: a round about the corner through the angle the base
    /// fills there, or the square with that side (a kite where the corner isn't square, its
    /// cuts square to the two edges). The base's edges then start that far from the corner, so
    /// both bends stop there, cut straight across (`zone_span`): their ends stay exact arcs,
    /// the round runs on into them tangentially, and the bend table gets the shorter bends.
    /// The flange panels keep the ends `close_corner` gave them. A relief too small to cut, one
    /// that would leave nothing of an edge it starts on (with the relief at that edge's other
    /// end, where there is one), or one reaching as far from the corner as another edge of the
    /// base is, is refused with the reason.
    fn relieve_corner(&mut self, k: &Corner, corners: &[Corner], shape: ReliefShape, size: f64) -> std::result::Result<(), String> {
        if !(size >= MIN_CORNER_RELIEF) {
            return Err(format!("the relief size {size:.4} mm is below {MIN_CORNER_RELIEF} mm"));
        }
        let r = size / 2.0;
        let segs = self.base.outer.ccw().segs;
        let n = segs.len();
        // The base's edges at the corner: `i` ends there and the next one starts there.
        let at_corner =
            |i: usize| segs.get(i).is_some_and(|s| s.end().dist(k.c) < 1e-6) && segs.get((i + 1) % n).is_some_and(|s| s.start().dist(k.c) < 1e-6);
        let Some(i) = (0..n).find(|&i| at_corner(i)) else { return Err("the bends don't meet at a corner of the base".into()) };
        let j = (i + 1) % n;
        let (Some(&Seg2::Line { a: pa, b: c }), Some(&Seg2::Line { b: pb, .. })) = (segs.get(i), segs.get(j)) else {
            return Err("the base's edges at the corner are not straight".into());
        };
        let (Some(da), Some(db)) = ((pa - c).normalized(), (pb - c).normalized()) else {
            return Err("an edge of the base at the corner has no length".into());
        };
        // The angle the base fills at the corner.
        let alpha = db.cross(da).atan2(db.dot(da));
        if !(alpha > 1e-6 && alpha < std::f64::consts::PI - 1e-6) {
            // An inside corner of the base: the flanges turn away from each other.
            return Ok(());
        }
        // Both bends must keep some length. An edge whose far end is another of the corners has
        // to hold a relief there too, so the reliefs at its two ends fit or are left out together.
        let takes = |far: Vec2| if corners.iter().any(|q| q.c.dist(far) < 1e-6) { 2.0 * r } else { r };
        if takes(pa) >= pa.dist(c) - 1e-6 || takes(pb) >= pb.dist(c) - 1e-6 {
            return Err(format!("a relief {size:.3} mm across would take a whole bend away"));
        }
        // The relief from the edge that ends at the corner to the one that starts there, and
        // how far from the corner it reaches.
        let (relief, reach) = match shape {
            ReliefShape::Notch | ReliefShape::Tear => return Ok(()),
            ReliefShape::Round => (vec![Seg2::Arc { center: c, radius: r, start: da.angle(), sweep: -alpha }], r),
            ReliefShape::Rectangular => {
                let x = snap(c + (da + db) * (r / (1.0 + da.dot(db))));
                (vec![Seg2::Line { a: snap(c + da * r), b: x }, Seg2::Line { a: x, b: snap(c + db * r) }], x.dist(c))
            }
        };
        let (Some(start), Some(end)) = (relief.first().map(Seg2::start), relief.last().map(Seg2::end)) else { return Ok(()) };
        // The rest of the outline must stay clear of it.
        let clear = segs.iter().enumerate().filter(|(e, _)| *e != i && *e != j).all(|(_, s)| {
            s.polyline(1e-3).windows(2).all(|w| {
                let d = w[1] - w[0];
                let t = if d.len2() > 1e-30 { ((c - w[0]).dot(d) / d.len2()).clamp(0.0, 1.0) } else { 0.0 };
                c.dist(w[0] + d * t) > reach + 1e-6
            })
        });
        if !clear {
            return Err(format!("a relief {size:.3} mm across reaches {reach:.3} mm from the corner, where another edge of the base is nearer"));
        }
        let mut outer = Vec::with_capacity(n + relief.len());
        for (e, s) in segs.iter().enumerate() {
            if e == i {
                outer.push(Seg2::Line { a: pa, b: start });
                outer.extend(relief.iter().copied());
            } else if e == j {
                outer.push(Seg2::Line { a: end, b: pb });
            } else {
                outer.push(*s);
            }
        }
        self.base = Region2 { outer: Loop2 { segs: outer }, holes: self.base.holes.clone() };
        Ok(())
    }

    /// A flat hem: a 180° bend with a tiny radius at the panel edge, `length` to its outer face.
    pub fn add_hem(&mut self, pick: Vec3, length: f64, gap: f64, flip: bool) -> Result<usize> {
        let (panel, a, b, top, dist) = self.edge_at(pick).ok_or_else(|| DocError::Invalid("no sheet edge there".into()))?;
        if dist > self.t * 2.0 + 1e-3 {
            return Err(DocError::Invalid("pick an edge of the sheet".into()));
        }
        let d = (b - a).normalized().ok_or_else(|| DocError::Invalid("edge".into()))?;
        let outward = Vec2::new(d.y, -d.x);
        let leg = length - (gap + self.t);
        if leg < 0.0 {
            return Err(DocError::Invalid("the hem is shorter than its bend".into()));
        }
        // A hem folds back over the face it starts from (toward the bottom unless flipped).
        let up = top != flip;
        let up = !up;
        self.flanges.push(SheetFlange {
            parent: panel,
            p: a,
            d,
            len: a.dist(b),
            n: outward,
            angle: if up { std::f64::consts::PI } else { -std::f64::consts::PI },
            radius: gap,
            leg,
            ext: (0.0, 0.0),
            slope: (0.0, 0.0),
            tear: (false, false),
            shape: None,
            exact: None,
            tag: String::new(),
        });
        Ok(self.flanges.len() - 1)
    }

    /// Fold a panel (the base or a flange's) along a line on its top or bottom face (world
    /// points `a`, `b`). The bend zone (width BA = θ·(R + K·T)) is taken out of the existing
    /// sheet, so the flat pattern keeps its size; `position` puts the line at the zone's centre
    /// (`centerline`), start or end (`start`/`end`, from the fixed side), or on the mould line
    /// (`mould`: where the outer faces meet, so the fixed side measures the same to it folded as
    /// flat). On the base the side holding `fixed` (default: the larger side) stays put; on a
    /// flange the side toward its own bend does. `flip` folds the other way. A `fixed` point that
    /// touches no material still picks the side it lies on, and sets `warning` (see
    /// [`FOLD_FIXED_OFF_SHEET`]).
    #[allow(clippy::too_many_arguments)]
    pub fn add_fold(
        &mut self,
        a: Vec3,
        b: Vec3,
        angle: f64,
        radius: f64,
        position: &str,
        flip: bool,
        fixed: Option<Vec3>,
        warning: &mut Option<String>,
    ) -> Result<usize> {
        match self.fold_side(a, b, angle, radius, position, flip, fixed, warning, false) {
            // The tabs the line folds can't take the side beyond them along, since it joins
            // the side that stays through other material (a tab on an inside edge of an L,
            // the fixed point on the far leg): the tabs' own ends fold instead, and the
            // fixed point's material still stays (#639).
            Err(DocError::Invalid(m)) if m == FOLD_TABS_REJOIN => self.fold_side(a, b, angle, radius, position, flip, fixed, warning, true),
            r => r,
        }
    }

    /// [`Self::add_fold`]; `other_side` moves the side the fixed point would keep.
    #[allow(clippy::too_many_arguments)]
    fn fold_side(
        &mut self,
        a: Vec3,
        b: Vec3,
        angle: f64,
        radius: f64,
        position: &str,
        flip: bool,
        fixed: Option<Vec3>,
        warning: &mut Option<String>,
        other_side: bool,
    ) -> Result<usize> {
        if !(angle > 1e-6 && angle <= std::f64::consts::PI - 1e-6) {
            return Err(DocError::Invalid("the fold angle must be between 0 and 180°".into()));
        }
        if !(radius >= 0.0) {
            return Err(DocError::Invalid("the bend radius must not be negative".into()));
        }
        // The panel the line lies on: both ends on its top or bottom face, the middle inside it.
        let on_face = |q: Vec3| q.z.abs() < 1e-4 || (q.z - self.t).abs() < 1e-4;
        let mut found = None;
        for (pi, r) in self.panels() {
            let Some(inv) = mat_inverse(&self.world(pi)) else { continue };
            let (fa, fb) = (apply_point(&inv, a), apply_point(&inv, b));
            if !(on_face(fa) && on_face(fb) && (fa.z - fb.z).abs() < 1e-4) {
                continue;
            }
            let pts = dedup(&r.outer.ccw().polyline(1e-3));
            let (a2, b2) = (Vec2::new(fa.x, fa.y), Vec2::new(fb.x, fb.y));
            if point_in(&pts, (a2 + b2) * 0.5) || polys_cross(&pts, &[a2, b2, (a2 + b2) * 0.5]) {
                found = Some((pi, inv, a2, b2, pts));
                break;
            }
        }
        let (panel, inv, a2, b2, pts) = found.ok_or_else(|| DocError::Invalid("the fold line must lie on a flat face of the sheet".into()))?;
        let d = (b2 - a2).normalized().ok_or_else(|| DocError::Invalid("the fold line has no length".into()))?;
        let mut n = Vec2::new(-d.y, d.x);
        let l0 = a2.dot(n);
        // Which side moves. A flange keeps the side its own bend is on.
        let on_line = n * l0;
        let side_area = |m: Vec2| signed_area(&clip(&pts, on_line, m)).abs();
        let moves_n = match (panel, fixed) {
            (Some(j), _) => {
                let f = self.flanges.get(j).ok_or_else(|| DocError::Invalid("flange".into()))?;
                let ba = self.allowance(f);
                // The panel's attaching edge must stay whole on the fixed side.
                let (e0, e1) = (f.p + f.n * ba - f.d * f.ext.0, f.p + f.n * ba + f.d * (f.len + f.ext.1));
                let (s0, s1) = (e0.dot(n) - l0, e1.dot(n) - l0);
                if s0 * s1 < -1e-12 {
                    return Err(DocError::Invalid("a fold on a flange must run across it, clear of the flange's own bend".into()));
                }
                (s0 + s1) < 0.0
            }
            (None, Some(p)) => {
                let q = apply_point(&inv, p);
                let q2 = Vec2::new(q.x, q.y);
                // A point in the panel's slab must be on material: inside the outline and
                // outside every cut-out (a side face of either counts as material).
                let near = |poly: &[Vec2]| {
                    let m = poly.len();
                    (0..m).any(|i| match (poly.get(i), poly.get((i + 1) % m)) {
                        (Some(&a), Some(&b)) => {
                            let d = b - a;
                            let t = if d.len2() > 1e-18 { ((q2 - a).dot(d) / d.len2()).clamp(0.0, 1.0) } else { 0.0 };
                            (a + d * t).dist(q2) < 1e-3
                        }
                        _ => false,
                    })
                };
                let in_slab = q.z > -1e-4 && q.z < self.t + 1e-4;
                let in_hole = self.holes.iter().any(|h| h.contains(q2) && !near(&h.outer.ccw().polyline(1e-3)));
                if in_slab && ((!point_in(&pts, q2) && !near(&pts)) || in_hole) {
                    *warning =
                        Some(format!("{FOLD_FIXED_OFF_SHEET} at ({:.3}, {:.3}, {:.3}); the side of the fold line it lies on stays", p.x, p.y, p.z));
                }
                q2.dot(n) < l0
            }
            (None, None) => side_area(n) <= side_area(n * -1.0),
        };
        if moves_n == other_side {
            n = n * -1.0;
        }
        let l = a2.dot(n);
        let probe = SheetFlange {
            parent: panel,
            p: a2,
            d,
            len: 1.0,
            n,
            angle,
            radius,
            leg: 0.0,
            ext: (0.0, 0.0),
            slope: (0.0, 0.0),
            tear: (false, false),
            shape: None,
            exact: None,
            tag: String::new(),
        };
        let ba = self.allowance(&probe);
        let s0 = match position {
            "centerline" | "center" | "centre" => l - ba / 2.0,
            "start" => l,
            "end" => l - ba,
            "mould" | "mold" => l - (radius + self.t) * (angle / 2.0).tan(),
            o => return Err(DocError::Invalid(format!("unknown fold position `{o}` (centerline, start, end or mould)"))),
        };
        let fixed_part = clip(&pts, n * s0, n * -1.0);
        let moving = clip(&pts, n * (s0 + ba), n);
        let strip = clip(&clip(&pts, n * s0, n), n * (s0 + ba), n * -1.0);
        if signed_area(&fixed_part).abs() < 1e-9 || signed_area(&moving).abs() < 1e-9 {
            return Err(DocError::Invalid("the fold line must cross the sheet with material on both sides of the bend".into()));
        }
        // Separate tabs on one line (the zone falls apart into several pieces): fold the ones
        // the line's points span, each as its own bend.
        let strips = merge_loops(std::slice::from_ref(&strip), &mut FlatWork::new())?;
        // A zone clipped across the root of a tab folded before on the same line keeps a sliver
        // of no width there: one real piece is the zone (#638).
        let strip = match strips.as_slice() {
            [one] => one.clone(),
            _ => strip,
        };
        if strips.len() > 1 {
            let span = (a2.dot(d).min(b2.dot(d)), a2.dot(d).max(b2.dot(d)));
            let angle = if flip { -angle } else { angle };
            return self.fold_tabs(panel, &pts, (n, d), (s0, ba), span, (angle, radius), &strips);
        }
        // The bend zone must be a rectangle across the panel (a slanted or curved edge would
        // run across the bend as a helix, which the sheet's faces can't carry yet).
        let us: Vec<f64> = strip.iter().map(|q| q.dot(d)).collect();
        let (u0, u1) = (us.iter().cloned().fold(f64::MAX, f64::min), us.iter().cloned().fold(f64::MIN, f64::max));
        if ((u1 - u0) * ba - signed_area(&strip).abs()).abs() > 1e-6 * (u1 - u0).max(1.0) * ba.max(1.0) {
            // A corner of the outline inside the zone (e.g. a fold right at a tab's root): name
            // it and how far the line has to move to clear it.
            let corners: Vec<(Vec2, f64)> = pts.iter().map(|q| (*q, q.dot(n) - s0)).filter(|(_, s)| *s > 1e-6 && *s < ba - 1e-6).collect();
            if let Some(&(c, _)) = corners.first() {
                let on = corners.iter().fold(0.0_f64, |m, (_, s)| m.max(*s));
                let back = corners.iter().fold(0.0_f64, |m, (_, s)| m.max(ba - *s));
                let w = apply_point(&self.world(panel), Vec3::new(c.x, c.y, self.t));
                return Err(DocError::Invalid(format!(
                    "the bend zone ({ba:.3} mm wide) reaches a corner of the sheet's outline at ({:.3}, {:.3}, {:.3}): \
                     move the fold line at least {on:.3} mm toward the side that folds or {back:.3} mm toward the side that stays",
                    w.x, w.y, w.z
                )));
            }
            return Err(DocError::Invalid("not supported yet: folds whose bend zone crosses a slanted or curved edge of the sheet".into()));
        }
        let up = !flip;
        let leg = moving.iter().map(|q| q.dot(n) - s0 - ba).fold(0.0, f64::max);
        let p0 = n * s0 + d * u0;
        let ccw = |mut v: Vec<Vec2>| {
            if signed_area(&v) < 0.0 {
                v.reverse();
            }
            v.into_iter().map(snap).collect::<Vec<_>>()
        };
        match panel {
            None => self.base = Region2 { outer: Loop2::polygon(&fixed_part).ccw(), holes: self.base.holes.clone() },
            Some(j) => {
                if let Some(f) = self.flanges.get_mut(j) {
                    f.shape = Some(ccw(fixed_part.clone()));
                    f.exact = None;
                }
            }
        }
        self.flanges.push(SheetFlange {
            parent: panel,
            p: snap(p0),
            d,
            len: u1 - u0,
            n,
            angle: if up { angle } else { -angle },
            radius,
            leg,
            ext: (0.0, 0.0),
            slope: (0.0, 0.0),
            tear: (false, false),
            shape: Some(ccw(moving)),
            exact: None,
            tag: String::new(),
        });
        let i = self.flanges.len() - 1;
        // Flanges on the moving part's edges now hang off the fold and move with it.
        for k in 0..i {
            let Some(f) = self.flanges.get(k) else { continue };
            if f.parent != panel {
                continue;
            }
            let (e0, e1) = ((f.p - p0).dot(n), (f.p + f.d * f.len - p0).dot(n));
            if e0.min(e1) >= ba - 1e-9 {
                if let Some(f) = self.flanges.get_mut(k) {
                    f.parent = Some(i);
                }
            } else if e0.max(e1) > 1e-9 {
                return Err(DocError::Invalid("not supported yet: a fold through a flange's bend".into()));
            }
        }
        // Cut-outs may cross the bend where their edges run along or across it (the faces
        // check the rest when the sheet is built).
        Ok(i)
    }

    /// A fold whose bend zone falls apart into several pieces (`strips`: tabs on one line). The
    /// pieces the line's points span (`span`, along `d`) bend, each with the material beyond it;
    /// the rest of the panel stays put. `n` points to the moving side, the zone is `s0..s0 + ba`.
    #[allow(clippy::too_many_arguments)]
    fn fold_tabs(
        &mut self,
        panel: Option<usize>,
        pts: &[Vec2],
        (n, d): (Vec2, Vec2),
        (s0, ba): (f64, f64),
        span: (f64, f64),
        (angle, radius): (f64, f64),
        strips: &[Vec<Vec2>],
    ) -> Result<usize> {
        const EPS: f64 = 1e-6;
        let urange = |p: &[Vec2]| p.iter().fold((f64::MAX, f64::MIN), |(lo, hi), q| (lo.min(q.dot(d)), hi.max(q.dot(d))));
        let overlap = |a: (f64, f64), b: (f64, f64)| a.1.min(b.1) - a.0.max(b.0);
        let mut work = FlatWork::new();
        // The material beyond the zone, piece by piece, and the zone piece each one hangs from.
        let beyond = merge_loops(&[clip(pts, n * (s0 + ba), n)], &mut work)?;
        let far_edges = |p: &[Vec2]| -> Vec<(f64, f64)> {
            let m = p.len();
            (0..m)
                .filter_map(|i| Some((*p.get(i)?, *p.get((i + 1) % m)?)))
                .filter(|(a, b)| (a.dot(n) - s0 - ba).abs() < EPS && (b.dot(n) - s0 - ba).abs() < EPS)
                .map(|(a, b)| (a.dot(d).min(b.dot(d)), a.dot(d).max(b.dot(d))))
                .collect()
        };
        let strip_u: Vec<(f64, f64)> = strips.iter().map(|s| urange(s)).collect();
        let mut hangs: Vec<Vec<usize>> = vec![Vec::new(); strips.len()];
        for (k, part) in beyond.iter().enumerate() {
            let edges = far_edges(part);
            let on: Vec<usize> = (0..strips.len()).filter(|&i| strip_u.get(i).is_some_and(|&u| edges.iter().any(|&e| overlap(e, u) > EPS))).collect();
            match on.as_slice() {
                [i] => {
                    if let Some(h) = hangs.get_mut(*i) {
                        h.push(k);
                    }
                }
                _ => return Err(DocError::Invalid(FOLD_TABS_REJOIN.into())),
            }
        }
        let chosen: Vec<usize> = (0..strips.len()).filter(|&i| strip_u.get(i).is_some_and(|&u| overlap(u, span) > EPS)).collect();
        if chosen.is_empty() {
            return Err(DocError::Invalid("the fold line must cross the sheet with material on both sides of the bend".into()));
        }
        // What stays: the panel short of the zone plus the zone pieces (and what hangs from
        // them) that don't fold.
        let mut keep = vec![clip(pts, n * s0, n * -1.0)];
        for i in 0..strips.len() {
            if chosen.contains(&i) {
                continue;
            }
            keep.extend(strips.get(i).cloned());
            keep.extend(hangs.get(i).into_iter().flatten().filter_map(|&k| beyond.get(k).cloned()));
        }
        let fixed = match merge_loops(&keep, &mut work)?.as_slice() {
            [one] if signed_area(one) > 1e-9 => one.clone(),
            _ => return Err(DocError::Invalid("not supported yet: a fold that splits the part that stays".into())),
        };
        let mut made = Vec::new();
        for &i in &chosen {
            let (Some(strip), Some(&(u0, u1))) = (strips.get(i), strip_u.get(i)) else { continue };
            // Each piece of the zone must be a rectangle (see `add_fold`).
            if ((u1 - u0) * ba - signed_area(strip).abs()).abs() > 1e-6 * (u1 - u0).max(1.0) * ba.max(1.0) {
                return Err(DocError::Invalid("not supported yet: folds whose bend zone crosses a slanted or curved edge of the sheet".into()));
            }
            let moving = match hangs.get(i).map(Vec::as_slice) {
                Some([k]) => beyond.get(*k).cloned().ok_or_else(|| DocError::Invalid("fold".into()))?,
                _ => return Err(DocError::Invalid("the fold line must cross the sheet with material on both sides of the bend".into())),
            };
            let leg = moving.iter().map(|q| q.dot(n) - s0 - ba).fold(0.0, f64::max);
            made.push((u0, u1, n * s0 + d * u0, leg, moving));
        }
        // Flanges already on this panel: those beyond a folding tab move with it.
        let mut moves = Vec::new();
        for k in 0..self.flanges.len() {
            let Some(f) = self.flanges.get(k) else { continue };
            if f.parent != panel {
                continue;
            }
            let (e0, e1) = (f.p.dot(n) - s0, (f.p + f.d * f.len).dot(n) - s0);
            let fu = (f.p.dot(d).min((f.p + f.d * f.len).dot(d)), f.p.dot(d).max((f.p + f.d * f.len).dot(d)));
            let crosses = Err(DocError::Invalid("not supported yet: a fold through a flange's bend".into()));
            match made.iter().position(|m| fu.0 >= m.0 - 1e-9 && fu.1 <= m.1 + 1e-9) {
                Some(j) if e0.min(e1) >= ba - 1e-9 => moves.push((k, j)),
                Some(_) if e0.max(e1) > 1e-9 => return crosses,
                None if e0.max(e1) > 1e-9 && made.iter().any(|m| overlap((m.0, m.1), fu) > EPS) => return crosses,
                _ => {}
            }
        }
        let ccw = |mut v: Vec<Vec2>| {
            if signed_area(&v) < 0.0 {
                v.reverse();
            }
            v.into_iter().map(snap).collect::<Vec<_>>()
        };
        match panel {
            None => self.base = Region2 { outer: Loop2::polygon(&fixed).ccw(), holes: self.base.holes.clone() },
            Some(j) => {
                if let Some(f) = self.flanges.get_mut(j) {
                    f.shape = Some(ccw(fixed));
                    f.exact = None;
                }
            }
        }
        let first = self.flanges.len();
        for (u0, u1, p0, leg, moving) in made {
            self.flanges.push(SheetFlange {
                parent: panel,
                p: snap(p0),
                d,
                len: u1 - u0,
                n,
                angle,
                radius,
                leg,
                ext: (0.0, 0.0),
                slope: (0.0, 0.0),
                tear: (false, false),
                shape: Some(ccw(moving)),
                exact: None,
                tag: String::new(),
            });
        }
        for (k, j) in moves {
            if let Some(f) = self.flanges.get_mut(k) {
                f.parent = Some(first + j);
            }
        }
        Ok(first)
    }

    /// Name a panel edge (flat coordinates a→b on `panel`, top or bottom face): base edges by the
    /// base sketch line they lie on, flange edges by the flange (its tag) and side.
    fn edge_name_of(&self, panel: Option<usize>, a: Vec2, b: Vec2, top: bool) -> Option<String> {
        let face = if top { "top" } else { "bottom" };
        let d = (b - a).normalized()?;
        match panel {
            None => {
                let on = |p: Vec2, s: Vec2, e: Vec2| {
                    let t = e - s;
                    t.len() > 1e-9 && ((p - s).cross(t) / t.len()).abs() < 1e-6
                };
                self.base_curves.iter().find(|(_, s, e)| on(a, *s, *e) && on(b, *s, *e)).map(|(id, _, _)| format!("base:{id}:{face}"))
            }
            Some(i) => {
                let f = self.flanges.get(i)?;
                if f.tag.is_empty() {
                    return None;
                }
                let side = if d.cross(f.d).abs() < 1e-6 {
                    // Along the bend: the far edge (or the bend side, never picked).
                    if ((a - f.p).dot(f.n)) > self.allowance(f) + 1e-6 { "far" } else { "bend" }
                } else if ((a + b) * 0.5 - f.p).dot(f.d) < f.len / 2.0 {
                    "side0"
                } else {
                    "side1"
                };
                Some(format!("{}:{side}:{face}", f.tag))
            }
        }
    }

    /// The persistent name of the sheet edge nearest a world point.
    pub fn edge_name(&self, pick: Vec3) -> Option<String> {
        let (panel, a, b, top, _) = self.edge_at(pick)?;
        self.edge_name_of(panel, a, b, top)
    }

    /// A world point on the sheet edge with this name (the one nearest `near` if several): the
    /// point of that edge nearest `near`, off its very ends, so that what is made on the edge
    /// knows where along it the pick was.
    pub fn edge_by_name(&self, name: &str, near: Vec3) -> Option<Vec3> {
        let mut best: Option<(f64, Vec3)> = None;
        for (pi, r) in self.panels() {
            let pts = r.outer.ccw().polyline(1e-3);
            let m = pts.len();
            let w = self.world(pi);
            let local = mat_inverse(&w).map(|inv| apply_point(&inv, near));
            for i in 0..m {
                let (Some(&a), Some(&b)) = (pts.get(i), pts.get((i + 1) % m)) else { continue };
                if a.dist(b) < 1e-9 {
                    continue;
                }
                for (z, top) in [(0.0, false), (self.t, true)] {
                    if self.edge_name_of(pi, a, b, top).as_deref() == Some(name) {
                        let mid = (a + b) * 0.5;
                        let q = apply_point(&w, Vec3::new(mid.x, mid.y, z));
                        if best.is_none_or(|(d, _)| q.dist(near) < d) {
                            let along = local.map(|p| (Vec2::new(p.x, p.y) - a).dot(b - a) / (b - a).len2()).filter(|t| t.is_finite());
                            let on = a + (b - a) * along.map_or(0.5, |t| t.clamp(1e-6, 1.0 - 1e-6));
                            best = Some((q.dist(near), apply_point(&w, Vec3::new(on.x, on.y, z))));
                        }
                    }
                }
            }
        }
        best.map(|(_, q)| q)
    }

    /// The persistent name of a panel: `base`, a flange's tag, or `#<index>` for an untagged
    /// flange; `flat` for the whole sheet while it is unfolded.
    fn panel_name(&self, pi: Option<usize>) -> String {
        match pi {
            _ if self.flat => "flat".into(),
            None => "base".into(),
            Some(i) => match self.flanges.get(i) {
                Some(f) if !f.tag.is_empty() => f.tag.clone(),
                _ => format!("#{i}"),
            },
        }
    }

    /// The plane of a flat face of a panel (`top`: the face at the thickness, else the bottom
    /// one), facing out of the sheet. Its coordinates are the flat pattern's (the bottom
    /// face's mirrored in y), so a sketch on it measures as the flat pattern does.
    fn panel_face_plane(&self, world: &Mat, top: bool) -> Option<Plane> {
        let (x, y) = (apply_vector(world, Vec3::X), apply_vector(world, Vec3::Y));
        Plane::new(apply_point(world, Vec3::new(0.0, 0.0, if top { self.t } else { 0.0 })), x, if top { y } else { y * -1.0 })
    }

    /// The flat face of the sheet that a point lies on (sheet coordinates, within `tol`, which
    /// stays below a quarter of the thickness): its persistent name (`<panel>:top`,
    /// `<panel>:bottom`, or a rim's `<panel>:rim.<edge>`) and its plane (see
    /// [`panel_face`](Self::panel_face)).
    pub fn panel_face_at(&self, p: Vec3, tol: f64) -> Option<(String, Plane)> {
        let tol = tol.min(self.t * 0.25);
        let flat = if self.flat { self.flat_regions().ok()? } else { Vec::new() };
        let panels = if self.flat { vec![(None, self.base.clone())] } else { self.panels() };
        for (pi, region) in panels {
            let world = self.world(pi);
            let Some(inverse) = mat_inverse(&world) else { continue };
            let q = apply_point(&inverse, p);
            let top = (q.z - self.t).abs() < q.z.abs();
            if (if top { (q.z - self.t).abs() } else { q.z.abs() }) > tol {
                continue;
            }
            let q2 = Vec2::new(q.x, q.y);
            if if self.flat { flat.iter().any(|r| r.contains(q2)) } else { region.contains(q2) } {
                return Some((format!("{}:{}", self.panel_name(pi), if top { "top" } else { "bottom" }), self.panel_face_plane(&world, top)?));
            }
        }
        self.rim_face_at(p, tol)
    }

    /// The straight edges of each panel's outline (folded sheets only) with the name of the rim
    /// face along each: `<panel>:rim.<edge>`, the edge as the sheet's edge names give it (a base
    /// sketch line's id, or `far`, `side0`, `side1` of a flange).
    fn rims(&self) -> Vec<(Option<usize>, Vec2, Vec2, String)> {
        if self.flat {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (pi, region) in self.panels() {
            let pts = dedup(&region.outer.ccw().polyline(1e-3));
            let m = pts.len();
            for i in 0..m {
                let (Some(&a), Some(&b)) = (pts.get(i), pts.get((i + 1) % m)) else { continue };
                if a.dist(b) < 1e-9 {
                    continue;
                }
                let panel = self.panel_name(pi);
                let Some(edge) = self.rim_edge(pi, a, b, i) else { continue };
                if edge.is_empty() || edge.contains(':') || edge == "bend" {
                    continue;
                }
                out.push((pi, a, b, format!("{panel}:rim.{edge}")));
            }
        }
        out
    }

    /// What names the rim along edge `a`→`b` (the `k`-th of its panel's outline): the base sketch
    /// line it lies on (else its place in the base's outline, `edge<k>`, for a base drawn by the
    /// feature itself), or a flange's `far`, `side0` or `side1` (`bend` where it meets its bend).
    fn rim_edge(&self, pi: Option<usize>, a: Vec2, b: Vec2, k: usize) -> Option<String> {
        let d = (b - a).normalized()?;
        match pi {
            None => match self.edge_name_of(None, a, b, true) {
                Some(edge) => edge.strip_prefix("base:")?.strip_suffix(":top").map(str::to_string),
                None if self.base_curves.is_empty() => Some(format!("edge{k}")),
                None => None,
            },
            Some(i) => {
                let f = self.flanges.get(i)?;
                Some(
                    if d.cross(f.d).abs() < 1e-6 {
                        if ((a - f.p).dot(f.n)) > self.allowance(f) + 1e-6 { "far" } else { "bend" }
                    } else if ((a + b) * 0.5 - f.p).dot(f.d) < f.len / 2.0 {
                        "side0"
                    } else {
                        "side1"
                    }
                    .to_string(),
                )
            }
        }
    }

    /// The flat rim face (the sheet's thickness along a straight outline edge of a panel) that a
    /// point lies on (sheet coordinates, within `tol`): its persistent name and plane (see
    /// [`panel_face`](Self::panel_face)).
    fn rim_face_at(&self, p: Vec3, tol: f64) -> Option<(String, Plane)> {
        let mut best: Option<(f64, String)> = None;
        for (pi, a, b, name) in self.rims() {
            let Some(inverse) = mat_inverse(&self.world(pi)) else { continue };
            let q = apply_point(&inverse, p);
            if q.z < -tol || q.z > self.t + tol {
                continue;
            }
            let q2 = Vec2::new(q.x, q.y);
            let d = b - a;
            let along = (q2 - a).dot(d) / d.len2();
            if !(-1e-9..=1.0 + 1e-9).contains(&along) {
                continue;
            }
            let off = ((a + d * along) - q2).len();
            if off <= tol && best.as_ref().is_none_or(|(o, _)| off < *o) {
                best = Some((off, name));
            }
        }
        let name = match best {
            Some((_, name)) => name,
            None => self.bend_side_at(p, tol)?,
        };
        let plane = self.panel_face(&name)?;
        Some((name, plane))
    }

    /// A flange's bend in its parent's flat coordinates: the parent's placement, the bend axis
    /// (a point and unit direction along the flange's edge), and the turn's start direction
    /// (from the axis to the material) and the direction it turns toward.
    fn bend_frame(&self, f: &SheetFlange) -> Option<(Mat, Vec3, Vec3, Vec3, Vec3)> {
        let (o, _, _) = self.bend_axis(f);
        let d = v3(f.d).normalized()?;
        let start = if f.angle >= 0.0 { Vec3::Z * -1.0 } else { Vec3::Z };
        let toward = v3(f.n).normalized()?;
        Some((self.world(f.parent), o, d, start, toward))
    }

    /// The flat end of a bend (where the bend stops along its flange's edge: an annular sector
    /// square to the bend axis) that a point lies on: `<flange>:rim.bend0` at its start,
    /// `rim.bend1` at its end. A torn end is a helix on the bend, no flat face.
    fn bend_side_at(&self, p: Vec3, tol: f64) -> Option<String> {
        if self.flat {
            return None;
        }
        for (i, f) in self.flanges.iter().enumerate() {
            let Some((world, o, d, start, toward)) = self.bend_frame(f) else { continue };
            let Some(inverse) = mat_inverse(&world) else { continue };
            let q = apply_point(&inverse, p) - o;
            let u = q.dot(d);
            let radial = q - d * u;
            let rho = radial.len();
            if rho < f.radius - tol || rho > f.radius + self.t + tol {
                continue;
            }
            let phi = radial.dot(toward).atan2(radial.dot(start));
            if phi < -1e-6 || phi > f.angle.abs() + 1e-6 {
                continue;
            }
            let (u0, u1) = self.zone_span(f);
            let side = if (u - u0).abs() <= tol && !f.tear.0 {
                "bend0"
            } else if (u - u1).abs() <= tol && !f.tear.1 {
                "bend1"
            } else {
                continue;
            };
            return Some(format!("{}:rim.{side}", self.panel_name(Some(i))));
        }
        None
    }

    /// The plane of a bend's flat end (see [`bend_side_at`](Self::bend_side_at)): facing out
    /// along the bend axis, its origin on the axis, its x across the sheet.
    fn bend_side(&self, i: usize, end: bool) -> Option<Plane> {
        let f = self.flanges.get(i)?;
        if if end { f.tear.1 } else { f.tear.0 } {
            return None;
        }
        let (world, o, d, _, _) = self.bend_frame(f)?;
        let (u0, u1) = self.zone_span(f);
        let (u, out) = if end { (u1, d) } else { (u0, d * -1.0) };
        let origin = o + d * u;
        Plane::new(apply_point(&world, origin), apply_vector(&world, Vec3::Z), apply_vector(&world, out.cross(Vec3::Z)))
    }

    /// The plane of the rim face named `rim` (`<edge>` of `rim.<edge>`) of panel `pi`: on the
    /// rim, facing out of the panel, its x along the outline (counterclockwise about the panel's
    /// top) and its y across the thickness, its origin where the panel's origin projects onto it.
    fn rim_face(&self, pi: Option<usize>, rim: &str) -> Option<Plane> {
        match (pi, rim) {
            (Some(i), "bend0") => return self.bend_side(i, false),
            (Some(i), "bend1") => return self.bend_side(i, true),
            _ => {}
        }
        let panel = self.panel_name(pi);
        let name = format!("{panel}:rim.{rim}");
        let (_, a, b, _) = self.rims().into_iter().find(|(p, _, _, n)| *p == pi && *n == name)?;
        let world = self.world(pi);
        let d = (b - a).normalized()?;
        let foot = a + d * (Vec2::default() - a).dot(d);
        Plane::new(apply_point(&world, Vec3::new(foot.x, foot.y, 0.0)), apply_vector(&world, Vec3::new(d.x, d.y, 0.0)), apply_vector(&world, Vec3::Z))
    }

    /// The plane of the flat face with this persistent name (see [`panel_face_at`](Self::panel_face_at)).
    /// A face named while the sheet was folded follows its panel; one named unfolded (`flat`)
    /// is the flat sheet's own plane.
    pub fn panel_face(&self, name: &str) -> Option<Plane> {
        let (panel, side) = name.rsplit_once(':')?;
        let top = match side {
            "top" => true,
            "bottom" => false,
            _ if side.starts_with("rim.") => false,
            _ => return None,
        };
        let pi = match panel {
            "flat" | "base" => None,
            tag => Some(
                self.flanges
                    .iter()
                    .position(|f| f.tag == tag)
                    .or_else(|| tag.strip_prefix('#')?.parse::<usize>().ok().filter(|i| *i < self.flanges.len()))?,
            ),
        };
        if let Some(rim) = side.strip_prefix("rim.") {
            return self.rim_face(pi, rim);
        }
        self.panel_face_plane(&self.world(pi), top)
    }

    /// Map a region in a world plane to flat coordinates through a panel's placement.
    pub fn region_to_flat(&self, panel: Option<usize>, plane: &Plane, r: &Region2) -> Result<Region2> {
        region_complexity(std::slice::from_ref(r))?;
        let inv = mat_inverse(&self.world(panel)).ok_or_else(|| DocError::Invalid("invalid sheet panel frame".into()))?;
        let map = |p: Vec2| {
            let q = apply_point(&inv, plane.to_world(p));
            Vec2::new(q.x, q.y)
        };
        // A rigid map of the plane: lines and arcs stay lines and arcs (mirrored or not).
        let (o, ex, ey) = (map(Vec2::default()), map(Vec2::new(1.0, 0.0)), map(Vec2::new(0.0, 1.0)));
        let (ax, ay) = (ex - o, ey - o);
        let mirrored = ax.cross(ay) < 0.0;
        let rot = ax.y.atan2(ax.x);
        let seg = |sg: &Seg2| -> Seg2 {
            match *sg {
                Seg2::Line { a, b } => Seg2::Line { a: map(a), b: map(b) },
                Seg2::Arc { center, radius, start, sweep } => {
                    if mirrored {
                        Seg2::Arc { center: map(center), radius, start: rot - start, sweep: -sweep }
                    } else {
                        Seg2::Arc { center: map(center), radius, start: rot + start, sweep }
                    }
                }
                // Free-form segments map exactly, through their control points.
                other => map_controls(&other, map),
            }
        };
        let map_loop = |lp: &Loop2| Loop2 { segs: lp.segs.iter().map(seg).collect() };
        Ok(Region2 { outer: map_loop(&r.outer).ccw(), holes: r.holes.iter().map(|h| map_loop(h).ccw().reversed()).collect() })
    }

    /// Panels whose plane is parallel to `plane` (world): (panel, region).
    pub fn parallel_panels(&self, plane: &Plane) -> Vec<(Option<usize>, Region2)> {
        self.panels()
            .into_iter()
            .filter(|(pi, _)| {
                let w = self.world(*pi);
                let n = apply_vector(&w, Vec3::Z);
                n.cross(plane.normal()).len() < 1e-6
            })
            .collect()
    }

    pub(crate) fn material_panels(&self, plane: Option<&Plane>, work: &mut FlatWork) -> Result<Vec<(Option<usize>, Region2)>> {
        Ok(self
            .cells_with_work(work)?
            .into_iter()
            .filter_map(|c| {
                let Piece::Panel(pi) = c.piece else {
                    return None;
                };
                if plane.is_some_and(|p| apply_vector(&self.world(pi), Vec3::Z).cross(p.normal()).len() > 1e-6) {
                    return None;
                }
                Some((pi, Region2 { outer: Loop2 { segs: c.outer }, holes: c.holes.into_iter().map(|segs| Loop2 { segs }).collect() }))
            })
            .collect())
    }
}

fn point_in(poly: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    let m = poly.len();
    for i in 0..m {
        let (Some(&a), Some(&b)) = (poly.get(i), poly.get((i + m - 1) % m)) else { continue };
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// The boundary of a union of counter-clockwise polygons that meet edge to edge: edges are
/// split where corners touch them, edges running both ways cancel, the rest chain into loops
/// (outer loops counter-clockwise, holes clockwise; straight runs merged).
fn merge_loops(polys: &[Vec<Vec2>], work: &mut FlatWork) -> Result<Vec<Vec<Vec2>>> {
    let n = flat_points(polys)?;
    work.charge(n.saturating_mul(n).saturating_mul(2))?;
    let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
    for p in polys {
        let m = p.len();
        for i in 0..m {
            if let (Some(&a), Some(&b)) = (p.get(i), p.get((i + 1) % m)) {
                segs.push((a, b));
            }
        }
    }
    let pts: Vec<Vec2> = segs.iter().flat_map(|(a, b)| [*a, *b]).collect();
    let mut split: Vec<(Vec2, Vec2)> = Vec::new();
    for (a, b) in &segs {
        let d = *b - *a;
        let l2 = d.len2();
        if l2 < 1e-18 {
            continue;
        }
        let mut ts: Vec<f64> = pts
            .iter()
            .filter_map(|p| {
                let t = (*p - *a).dot(d) / l2;
                ((t > 1e-9 && t < 1.0 - 1e-9) && (*a + d * t).dist(*p) < 1e-7).then_some(t)
            })
            .collect();
        ts.push(0.0);
        ts.push(1.0);
        ts.sort_by(|x, y| x.total_cmp(y));
        ts.dedup_by(|x, y| (*x - *y).abs() < 1e-12);
        for w in ts.windows(2) {
            if split.len() >= MAX_FLAT_POINTS {
                return Err(flat_budget_error());
            }
            split.push((*a + d * w[0], *a + d * w[1]));
        }
    }
    work.charge(split.len().saturating_mul(split.len()).saturating_mul(2))?;
    let same = |p: Vec2, q: Vec2| p.dist(q) < 1e-6;
    let keep: Vec<(Vec2, Vec2)> = split
        .iter()
        .enumerate()
        .filter(|(i, (a, b))| !split.iter().enumerate().any(|(j, (c, d))| j != *i && same(*a, *d) && same(*b, *c)))
        .map(|(_, s)| *s)
        .collect();
    let mut loops = Vec::new();
    let mut used = vec![false; keep.len()];
    for s0 in 0..keep.len() {
        if used.get(s0).copied().unwrap_or(true) {
            continue;
        }
        let mut lp = Vec::new();
        let mut cur = s0;
        for _ in 0..keep.len() + 1 {
            if let Some(u) = used.get_mut(cur) {
                *u = true;
            }
            let Some(&(a, b)) = keep.get(cur) else { break };
            lp.push(a);
            match (0..keep.len()).find(|j| !used.get(*j).copied().unwrap_or(true) && keep.get(*j).is_some_and(|(c, _)| same(*c, b))) {
                Some(n) => cur = n,
                None => break,
            }
        }
        let m = lp.len();
        let clean: Vec<Vec2> = (0..m)
            .filter_map(|i| {
                let (p, q, r) = (*lp.get((i + m - 1) % m)?, *lp.get(i)?, *lp.get((i + 1) % m)?);
                ((q - p).cross(r - q).abs() > 1e-9 * (q - p).len().max(1.0) * (r - q).len().max(1.0)).then_some(q)
            })
            .collect();
        if clean.len() >= 3 {
            loops.push(clean);
        }
    }
    Ok(loops)
}

/// Whether `q` lies within `tol` of the closed polygon `poly`'s outline.
fn near_outline(poly: &[Vec2], q: Vec2, tol: f64) -> bool {
    let m = poly.len();
    (0..m).any(|i| match (poly.get(i), poly.get((i + 1) % m)) {
        (Some(&a), Some(&b)) => {
            let d = b - a;
            let t = if d.len2() > 1e-18 { ((q - a).dot(d) / d.len2()).clamp(0.0, 1.0) } else { 0.0 };
            (a + d * t).dist(q) < tol
        }
        _ => false,
    })
}

fn signed_area(p: &[Vec2]) -> f64 {
    let m = p.len();
    (0..m).filter_map(|i| Some(p.get(i)?.cross(*p.get((i + 1) % m)?))).sum::<f64>() / 2.0
}

/// Where the line from `a` along the unit vector `d` meets a boundary segment, as distances from
/// `a`: lines and arcs exactly, free-form curves on their 0.0001 mm chords.
fn line_hits(a: Vec2, d: Vec2, e: &Seg2, out: &mut Vec<f64>) {
    match *e {
        Seg2::Line { a: p, b: q } => {
            let r = q - p;
            let den = d.cross(r);
            if den.abs() > 1e-12 * r.len() {
                let t = (p - a).cross(d) / den;
                if (-1e-9..=1.0 + 1e-9).contains(&t) {
                    out.push((p - a).cross(r) / den);
                }
            }
        }
        Seg2::Arc { center, radius, start, sweep } => {
            let (along, off) = ((center - a).dot(d), (center - a).cross(d));
            if off.abs() <= radius {
                let half = ((radius - off) * (radius + off)).max(0.0).sqrt();
                for s in [along - half, along + half] {
                    let angle = (a + d * s - center).angle();
                    let turned = if sweep >= 0.0 { angle - start } else { start - angle }.rem_euclid(std::f64::consts::TAU);
                    if turned <= sweep.abs() + 1e-9 || turned >= std::f64::consts::TAU - 1e-9 {
                        out.push(s);
                    }
                }
            }
        }
        _ => {
            for w in e.polyline(1e-4).windows(2) {
                if let [p, q] = *w {
                    line_hits(a, d, &Seg2::Line { a: p, b: q }, out);
                }
            }
        }
    }
}

/// The stretches of the line from `a` along the unit vector `d`, `len` long, that lie in none of
/// the `cuts` (flat regions), as distances from `a` in order. A line that no cut reaches comes
/// back whole, as (0, `len`); stretches under a thousandth of a micron are no material.
fn clear_of(a: Vec2, d: Vec2, len: f64, cuts: &[Region2]) -> Vec<(f64, f64)> {
    if !len.is_finite() || len <= 0.0 {
        return Vec::new();
    }
    let b = a + d * len;
    let (lo, hi) = (Vec2::new(a.x.min(b.x) - 1e-6, a.y.min(b.y) - 1e-6), Vec2::new(a.x.max(b.x) + 1e-6, a.y.max(b.y) + 1e-6));
    // Only cut-outs whose box the line enters can hold or cross it.
    let near: Vec<&Region2> = cuts
        .iter()
        .filter(|r| {
            let pts = r.outer.polyline(1e-3);
            let (mut p0, mut p1) = (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
            for p in &pts {
                (p0, p1) = (Vec2::new(p0.x.min(p.x), p0.y.min(p.y)), Vec2::new(p1.x.max(p.x), p1.y.max(p.y)));
            }
            p0.x <= hi.x && lo.x <= p1.x && p0.y <= hi.y && lo.y <= p1.y
        })
        .collect();
    let mut at = vec![0.0, len];
    for e in near.iter().flat_map(|r| std::iter::once(&r.outer).chain(&r.holes)).flat_map(|l| &l.segs) {
        line_hits(a, d, e, &mut at);
    }
    at.retain(|s| *s >= 0.0 && *s <= len);
    at.sort_by(f64::total_cmp);
    let mut out: Vec<(f64, f64)> = Vec::new();
    for w in at.windows(2) {
        let [s0, s1] = *w else { continue };
        if s1 - s0 < 1e-9 || near.iter().any(|r| r.contains(a + d * ((s0 + s1) / 2.0))) {
            continue;
        }
        match out.last_mut() {
            Some(last) if s0 - last.1 < 1e-9 => last.1 = s1,
            _ => out.push((s0, s1)),
        }
    }
    out.retain(|(s0, s1)| s1 - s0 > 1e-6);
    out
}

/// What stays of `piece` (counter-clockwise), the part of a base that trimming its edge a→b back
/// along the edge's whole line would take: where the base runs on past the edge's own line
/// (beyond an inside corner at an end of the edge), that material and the strip beneath it, up
/// to the corner; and the strip beyond an end of a→b where a cut-out is (`cut_at`: start, end),
/// a→b being what the cut-out left of a longer edge. Nothing where the base ends at the line.
/// With it comes the stretch of the edge's line between what stays, as distances from `a`:
/// infinite toward an end beyond which nothing stays.
fn kept_past_edge(piece: &[Vec2], a: Vec2, b: Vec2, cut_at: (bool, bool), work: &mut FlatWork) -> Result<(Vec<Vec<Vec2>>, (f64, f64))> {
    let Some(d) = (b - a).normalized() else { return Ok((Vec::new(), (f64::NEG_INFINITY, f64::INFINITY))) };
    let outward = Vec2::new(d.y, -d.x);
    let mut kept = merge_loops(&[clip(piece, a, outward)], work)?;
    // Where that material stands on the edge's line, nearest the edge's middle on either side.
    let um = a.dist(b) / 2.0;
    let (mut lo, mut hi) = (if cut_at.0 { 0.0 } else { f64::NEG_INFINITY }, if cut_at.1 { a.dist(b) } else { f64::INFINITY });
    for q in &kept {
        let m = q.len();
        for i in 0..m {
            let (Some(&p0), Some(&p1)) = (q.get(i), q.get((i + 1) % m)) else { continue };
            if (p0 - a).dot(outward).abs() > 1e-6 || (p1 - a).dot(outward).abs() > 1e-6 {
                continue;
            }
            let (u0, u1) = ((p0 - a).dot(d), (p1 - a).dot(d));
            if u0.max(u1) <= um {
                lo = lo.max(u0.max(u1));
            } else if u0.min(u1) >= um {
                hi = hi.min(u0.min(u1));
            }
        }
    }
    let strip = clip(piece, a, outward * -1.0);
    if lo.is_finite() {
        kept.push(clip(&strip, a + d * lo, d * -1.0));
    }
    if hi.is_finite() {
        kept.push(clip(&strip, a + d * hi, d));
    }
    Ok((kept, (lo, hi)))
}

/// How far (mm) outside the edge's line the strip a trim takes reaches round an end of the edge
/// beyond which nothing stays, and how far (per mm of outline) what an exact trim leaves may
/// differ from the trimmed polyline: an outline's curves lie up to 0.001 mm off its chords,
/// and the arcs that follow a free-form curve as far again.
const TRIM_PAD: f64 = 2e-3;

/// The strip that trimming the edge a→b back by `by` takes of piece `k` of `pieces` (the base
/// past the trimmed line, as polygons), as a polygon to cut out of the outline as drawn: from
/// the edge's line to the trimmed line, over `between` (the stretch [`kept_past_edge`] keeps
/// nothing of, as distances from `a`), square at a finite end. Toward an end beyond which
/// nothing stays it runs [`TRIM_PAD`] outside the edge's line and 1 mm past the piece, or
/// half-way to the next piece along the line (tabs side by side), so that it crosses the
/// outline at the trimmed line only. None where another piece lies beside this one across the
/// line.
fn taken_strip(pieces: &[Vec<Vec2>], k: usize, a: Vec2, b: Vec2, by: f64, (lo, hi): (f64, f64)) -> Option<Vec<Vec2>> {
    let d = (b - a).normalized()?;
    let inward = Vec2::new(-d.y, d.x);
    let at = |u: f64, w: f64| a + d * u + inward * w;
    let span = |p: &Vec<Vec2>| p.iter().map(|q| (*q - a).dot(d)).fold((f64::MAX, f64::MIN), |(u0, u1), u| (u0.min(u), u1.max(u)));
    let (p0, p1) = span(pieces.get(k)?);
    let (mut u0, mut u1) = (p0 - 1.0, p1 + 1.0);
    for (j, q) in pieces.iter().enumerate() {
        let (q0, q1) = span(q);
        if j == k {
            continue;
        } else if q1 < p0 - TRIM_PAD {
            u0 = u0.max((q1 + p0) / 2.0);
        } else if q0 > p1 + TRIM_PAD {
            u1 = u1.min((q0 + p1) / 2.0);
        } else {
            return None;
        }
    }
    if !lo.is_finite() && !hi.is_finite() {
        return Some(vec![at(u0, -TRIM_PAD), at(u1, -TRIM_PAD), at(u1, by), at(u0, by)]);
    }
    // Along the trimmed line, back down the far end, along the edge's line (stepping outside it
    // past an end beyond which nothing stays), and up again.
    let len = a.dist(b);
    let mut strip = vec![at(if lo.is_finite() { lo } else { u0 }, by), at(if hi.is_finite() { hi } else { u1 }, by)];
    if hi.is_finite() {
        strip.push(at(hi, 0.0));
    } else {
        strip.extend([at(u1, -TRIM_PAD), at(len, -TRIM_PAD), at(len, 0.0)]);
    }
    if lo.is_finite() {
        strip.push(at(lo, 0.0));
    } else {
        strip.extend([at(0.0, 0.0), at(0.0, -TRIM_PAD), at(u0, -TRIM_PAD)]);
    }
    Some(strip)
}

/// The outline `outer` (counter-clockwise) less the `strips` a trim takes ([`taken_strip`]), cut
/// as drawn: an arc stays an arc, ending where the trimmed line crosses it, and a free-form
/// curve that is cut is followed by tangent arcs. `cut` is what the same trim leaves of the
/// outline's polyline. None unless that leaves one piece holding the polygon's material to
/// within the chords (the trim keeps the polygon then).
fn trimmed_exactly(outer: &Loop2, strips: &[Vec<Vec2>], cut: &[Vec2]) -> Option<Loop2> {
    let strips: Vec<Region2> = strips.iter().map(|s| Region2 { outer: Loop2::polygon(s).ccw(), holes: Vec::new() }).collect();
    let base = Region2 { outer: outer.clone(), holes: Vec::new() };
    let mut left = material_boolean(&[base], &strips, kernel::BoolOp::Cut, &mut FlatWork::new()).ok()?;
    let exact = left.pop().filter(|r| left.is_empty() && r.holes.is_empty())?.outer.ccw();
    let outline: f64 = outer.segs.iter().map(Seg2::length).sum();
    ((exact.signed_area() - signed_area(cut).abs()).abs() <= TRIM_PAD * outline).then_some(exact)
}

/// Do two polygons' edges cross?
fn polys_cross(a: &[Vec2], b: &[Vec2]) -> bool {
    let seg = |p: &[Vec2], i: usize| -> Option<(Vec2, Vec2)> { Some((*p.get(i)?, *p.get((i + 1) % p.len())?)) };
    (0..a.len()).filter_map(|i| seg(a, i)).any(|(p, q)| {
        (0..b.len()).filter_map(|j| seg(b, j)).any(|(r, s)| {
            let d1 = (q - p).cross(r - p);
            let d2 = (q - p).cross(s - p);
            let d3 = (s - r).cross(p - r);
            let d4 = (s - r).cross(q - r);
            d1 * d2 < -1e-12 && d3 * d4 < -1e-12
        })
    })
}

/// Whether every segment of `lp` lies on one of `curves` (its ends and its middle, within
/// 0.0001 mm): the loop is made of those curves, or of parts of them.
pub fn loop_lies_on(lp: &Loop2, curves: &[Seg2]) -> bool {
    const TOL: f64 = 1e-4;
    let near = |p: Vec2| curves.iter().any(|c| c.point_at(c.closest_param(p)).dist(p) <= TOL);
    !lp.segs.is_empty() && lp.segs.iter().all(|g| near(g.start()) && near(g.mid()) && near(g.end()))
}

/// Tells which loops of a sheet's flat pattern are letters (see [`SheetBody::letters`]).
#[derive(Clone, Debug)]
pub struct Letters {
    /// Each text region's corners (lowest, highest) and the segments of its outlines.
    regions: Vec<(Vec2, Vec2, Vec<Seg2>)>,
}

impl Letters {
    /// Whether `lp` is made of the outlines of text the sheet carries: a letter, or one of its
    /// counters.
    pub fn is_letter(&self, lp: &Loop2) -> bool {
        let Some(p) = lp.segs.first().map(Seg2::start) else { return false };
        self.regions.iter().any(|(lo, hi, curves)| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y && loop_lies_on(lp, curves))
    }
}

/// Bounded overlap predicate, including crossing-only cuts whose vertices lie outside a panel.
pub(crate) fn flat_regions_overlap(a: &Region2, b: &Region2, work: &mut FlatWork) -> Result<bool> {
    let (a, b) = (planar_region(a)?, planar_region(b)?);
    Ok(!material_boolean(&[a], &[b], kernel::BoolOp::Intersect, work)?.is_empty())
}

/// A sheet from a closed profile (base flange): the region in `plane`, thickness along its normal.
pub fn base_flange(body: &str, rule: &str, rv: &RuleValues, plane: Plane, region: Region2) -> SheetBody {
    let holes = region.holes.into_iter().map(|h| Region2 { outer: h.ccw(), holes: Vec::new() }).collect();
    SheetBody {
        body: body.into(),
        rule: rule.into(),
        t: rv.thickness,
        k: rv.k_factor,
        frame: plane,
        base: Region2 { outer: region.outer, holes: Vec::new() },
        flanges: Vec::new(),
        holes,
        pockets: Vec::new(),
        bores: Vec::new(),
        perforations: Vec::new(),
        labels: Vec::new(),
        flat: false,
        gap: rv.gap,
        relief: rv.corner_relief_shape,
        relief_size: rv.corner_relief,
        bend_relief: (rv.relief_width, rv.relief_depth),
        base_curves: Vec::new(),
    }
}

/// A sheet from an open chain of lines (contour flange): `pts` in `plane`, the material on the
/// right of the travel (or the left when `flip`), `width` along the plane normal.
pub fn contour_flange(body: &str, rule: &str, rv: &RuleValues, plane: &Plane, pts: &[Vec2], width: f64, flip: bool) -> Result<SheetBody> {
    if pts.len() < 2 {
        return Err(DocError::Invalid("a contour flange needs an open chain of lines".into()));
    }
    let (t, r) = (rv.thickness, rv.bend_radius);
    let seg_dir = |i: usize| -> Result<(Vec2, f64)> {
        let (Some(a), Some(b)) = (pts.get(i), pts.get(i + 1)) else { return Err(DocError::Invalid("chain".into())) };
        let l = a.dist(*b);
        Ok(((*b - *a).normalized().ok_or_else(|| DocError::Invalid("zero-length line in the contour".into()))?, l))
    };
    let side = if flip { -1.0 } else { 1.0 };
    // Material side in the sketch: right of travel.
    let (d0, _) = seg_dir(0)?;
    let m0 = Vec2::new(d0.y, -d0.x) * side;
    let start = pts.first().copied().unwrap_or_default();
    let (x, z) = (plane.dir_to_world(d0), plane.dir_to_world(m0));
    let frame = Plane::new(plane.to_world(start), x, z.cross(x)).ok_or_else(|| DocError::Invalid("contour frame".into()))?;
    // Width along the sketch normal: +y or −y of the frame.
    let ysign = if frame.y.dot(plane.normal()) >= 0.0 { 1.0 } else { -1.0 };
    let n = pts.len() - 1;
    // Turn at each corner: angle and whether it bends toward the material (up).
    let mut corners = Vec::new();
    for i in 0..n.saturating_sub(1) {
        let (da, _) = seg_dir(i)?;
        let (db, _) = seg_dir(i + 1)?;
        let cross = da.cross(db) * side;
        let ang = da.dot(db).clamp(-1.0, 1.0).acos();
        if ang < 1e-9 {
            return Err(DocError::Invalid("straight corners in a contour flange".into()));
        }
        // Right-hand material: turning right (negative cross) bends toward it.
        corners.push((ang, cross < 0.0));
    }
    let setback = |ang: f64, up: bool| if up { (r + t) * (ang / 2.0).tan() } else { r * (ang / 2.0).tan() };
    let mut sheet = SheetBody {
        body: body.into(),
        rule: rule.into(),
        t,
        k: rv.k_factor,
        frame,
        base: Region2 { outer: Loop2 { segs: Vec::new() }, holes: Vec::new() },
        flanges: Vec::new(),
        holes: Vec::new(),
        pockets: Vec::new(),
        bores: Vec::new(),
        perforations: Vec::new(),
        labels: Vec::new(),
        flat: false,
        gap: rv.gap,
        relief: rv.corner_relief_shape,
        relief_size: rv.corner_relief,
        bend_relief: (rv.relief_width, rv.relief_depth),
        base_curves: Vec::new(),
    };
    let (y0, y1) = if ysign > 0.0 { (0.0, width) } else { (-width, 0.0) };
    // First segment: the base panel.
    let (_, l0) = seg_dir(0)?;
    let end0 = l0 - corners.first().map(|(a, u)| setback(*a, *u)).unwrap_or(0.0);
    if end0 <= 0.0 {
        return Err(DocError::Invalid("a contour line is shorter than its bends".into()));
    }
    sheet.base = rect(Vec2::new(0.0, y0), Vec2::new(end0, y0), Vec2::new(end0, y1), Vec2::new(0.0, y1));
    let mut parent: Option<usize> = None;
    let mut at = end0;
    for (i, (ang, up)) in corners.iter().enumerate() {
        let (_, l) = seg_dir(i + 1)?;
        let before = setback(*ang, *up);
        let after = corners.get(i + 1).map(|(a, u)| setback(*a, *u)).unwrap_or(0.0);
        let leg = l - before - after;
        if leg < -1e-9 {
            return Err(DocError::Invalid("a contour line is shorter than its bends".into()));
        }
        // The material flips sides relative to the panel when the bend goes down.
        let f = SheetFlange {
            parent,
            p: Vec2::new(at, y0),
            d: Vec2::new(0.0, 1.0),
            len: y1 - y0,
            n: Vec2::new(1.0, 0.0),
            angle: if *up { *ang } else { -*ang },
            radius: r,
            leg: leg.max(0.0),
            ext: (0.0, 0.0),
            slope: (0.0, 0.0),
            tear: (false, false),
            shape: None,
            exact: None,
            tag: String::new(),
        };
        let ba = sheet.allowance(&f);
        sheet.flanges.push(f);
        parent = Some(sheet.flanges.len() - 1);
        at += ba + leg.max(0.0);
    }
    Ok(sheet)
}

/// The name a sketch keeps for a flat face of a sheet: `sheet:<body>:<panel>:<top|bottom>`.
pub fn sheet_face_name(body: &str, face: &str) -> String {
    format!("sheet:{body}:{face}")
}

/// Point the sheet face a sketch plane is attached to (also beneath an offset or a rotation)
/// at body `new`, when it is on body `old`.
pub fn rename_plane_sheet_body(plane: &mut crate::PlaneRef, old: &str, new: &str) {
    match plane {
        crate::PlaneRef::Face { name: Some(name), anchor: None, .. } => {
            let renamed = split_sheet_face_name(name).filter(|(body, _)| *body == old).map(|(_, face)| sheet_face_name(new, face));
            if let Some(renamed) = renamed {
                *name = renamed;
            }
        }
        crate::PlaneRef::Offset { base, .. } | crate::PlaneRef::AtAngle { base, .. } | crate::PlaneRef::Flipped { base, .. } => {
            rename_plane_sheet_body(base, old, new)
        }
        _ => {}
    }
}

/// The sheet face a sketch plane is attached to (also beneath an offset or a rotation): its
/// [`sheet_face_name`].
pub fn plane_sheet_face(plane: &crate::PlaneRef) -> Option<&str> {
    match plane {
        crate::PlaneRef::Face { name: Some(name), anchor: None, .. } if split_sheet_face_name(name).is_some() => Some(name),
        crate::PlaneRef::Offset { base, .. } | crate::PlaneRef::AtAngle { base, .. } | crate::PlaneRef::Flipped { base, .. } => {
            plane_sheet_face(base)
        }
        _ => None,
    }
}

/// The body and the `<panel>:<side>` of a [`sheet_face_name`].
pub fn split_sheet_face_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("sheet:")?;
    let (head, side) = rest.rsplit_once(':')?;
    let (body, panel) = head.rsplit_once(':')?;
    (!body.is_empty() && !panel.is_empty() && !side.is_empty()).then_some(())?;
    Some((body, rest.get(body.len() + 1..)?))
}

/// The plane of the sheet face with a [`sheet_face_name`], in the body's own coordinates.
pub fn sheet_face_plane(st: &crate::ModelState, name: &str) -> Option<(String, Plane)> {
    let (body, face) = split_sheet_face_name(name)?;
    let plane = st.sheets.iter().find(|s| s.body == body)?.panel_face(face)?;
    Some((body.to_string(), plane))
}

/// Persistent names of the sheet edges nearest each world point (empty when none).
pub fn sheet_edge_names(st: &crate::ModelState, pts: &[Vec3]) -> Vec<String> {
    pts.iter()
        .map(|p| {
            st.sheets
                .iter()
                .filter_map(|sh| sh.edge_at(*p).map(|e| (e.4, sh)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .and_then(|(_, sh)| sh.edge_name(*p))
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steel() -> RuleValues {
        RuleValues {
            thickness: 2.5,
            k_factor: 0.44,
            bend_radius: 2.5,
            relief_width: 2.5,
            relief_depth: 1.25,
            corner_relief: 10.0,
            corner_relief_shape: ReliefShape::Notch,
            hem_gap: 0.01,
            gap: 2.5,
        }
    }

    fn plate(w: f64, h: f64) -> SheetBody {
        let r = rect(Vec2::new(0.0, 0.0), Vec2::new(w, 0.0), Vec2::new(w, h), Vec2::new(0.0, h));
        base_flange(
            "Body1",
            "Steel (mm)",
            &steel(),
            Plane::new(Vec3::ZERO, Vec3::X, Vec3::Y)
                .unwrap_or_else(|| Plane::named("XY").unwrap_or(Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Y })),
            r,
        )
    }

    fn vol(b: &Body) -> f64 {
        kernel::measure(b).map(|m| m.volume).unwrap_or(f64::NAN)
    }

    #[test]
    fn loops_lie_on_the_curves_they_are_made_of() {
        let circle = Loop2::circle(Vec2::new(5.0, 5.0), 3.0);
        let curves: Vec<Seg2> = circle.segs.clone();
        // The loop itself, started elsewhere and run the other way, and a loop of its halves.
        assert!(loop_lies_on(&circle, &curves));
        assert!(loop_lies_on(&Loop2::circle_from(Vec2::new(5.0, 5.0), 3.0, 1.0).reversed(), &curves));
        let halves: Vec<Seg2> = circle.segs.iter().flat_map(|g| [g.split_half().0, g.split_half().1]).collect();
        assert!(loop_lies_on(&Loop2 { segs: halves }, &curves));
        // Another circle, a square and an empty loop do not.
        assert!(!loop_lies_on(&Loop2::circle(Vec2::new(5.0, 5.0), 3.1), &curves));
        assert!(!loop_lies_on(&Loop2::circle(Vec2::new(5.0, 5.0), 2.0), &curves));
        assert!(!loop_lies_on(&rect(Vec2::ZERO, Vec2::X, Vec2::new(1.0, 1.0), Vec2::Y).outer, &curves));
        assert!(!loop_lies_on(&Loop2 { segs: Vec::new() }, &curves));
        // A loop only partly on them (a letter across the sheet's edge) is not on them.
        let notch = Loop2 { segs: vec![circle.segs[0], Seg2::Line { a: circle.segs[0].end(), b: Vec2::new(50.0, 50.0) }] };
        assert!(!loop_lies_on(&notch, &curves));
    }

    #[test]
    fn a_sheet_knows_the_letters_it_carries() {
        // A recorded text region with a counter: its outline and its counter are letters, the same
        // circle elsewhere, a loop near it that is another size and a loop far away are not.
        let mut s = plate(100.0, 60.0);
        let counter = Loop2::circle(Vec2::new(20.0, 20.0), 2.0).reversed();
        s.labels.push(Region2 { outer: Loop2::circle(Vec2::new(20.0, 20.0), 5.0), holes: vec![counter.clone()] });
        let letters = s.letters();
        assert!(letters.is_letter(&Loop2::circle(Vec2::new(20.0, 20.0), 5.0)));
        assert!(letters.is_letter(&counter.reversed()));
        assert!(!letters.is_letter(&Loop2::circle(Vec2::new(20.0, 20.0), 4.0)));
        assert!(!letters.is_letter(&Loop2::circle(Vec2::new(70.0, 20.0), 5.0)));
        // A sheet that carries none has no letters.
        assert!(!plate(100.0, 60.0).letters().is_letter(&Loop2::circle(Vec2::new(20.0, 20.0), 5.0)));
    }

    #[test]
    fn edge_flange_matches_fusion() {
        // Oracle 68: 100 x 60 plate, 20 mm flange on the y = 0 top edge.
        let mut s = plate(100.0, 60.0);
        s.add_flange(Vec3::new(50.0, 0.0, 2.5), 20.0, std::f64::consts::FRAC_PI_2, 2.5, "inside", false).unwrap();
        let b = s.solid().unwrap();
        assert!((vol(&b) - 18972.622).abs() < 2.0, "{}", vol(&b));
        let (w, h) = s.flat_size().unwrap();
        assert!((w - 100.0).abs() < 1e-9 && (h - 75.654867).abs() < 1e-5, "{w} {h}");
        let bends = s.bends();
        assert_eq!(bends.len(), 1);
        assert!((bends[0].allowance - std::f64::consts::FRAC_PI_2 * 3.6).abs() < 1e-9);
        // Unfolded: the same volume, flat.
        s.flat = true;
        let f = s.solid().unwrap();
        assert!((vol(&f) - 100.0 * 75.654867 * 2.5).abs() < 2.0);
        // Oracle 70: 45°, height 15.
        let mut s = plate(100.0, 60.0);
        s.add_flange(Vec3::new(50.0, 0.0, 2.5), 15.0, std::f64::consts::FRAC_PI_4, 2.5, "inside", false).unwrap();
        assert!((vol(&s.solid().unwrap()) - 18450.777).abs() < 2.0);
        assert!((s.flat_size().unwrap().1 - 73.685298).abs() < 1e-5);
    }

    #[test]
    fn bends_are_listed_where_their_centre_lines_have_material() {
        // A 200 × 100 plate with a 40 mm flange on its y = 0 edge: the bend zone lies below
        // y = 0, BA wide, its centre line along y = −BA / 2 from x = 0 to 200.
        let mut s = plate(200.0, 100.0);
        s.add_flange(Vec3::new(100.0, 0.0, 2.5), 40.0, std::f64::consts::FRAC_PI_2, 2.5, "outside", false).unwrap();
        let ba = s.allowance(&s.flanges[0]);
        assert!((ba - std::f64::consts::FRAC_PI_2 * 3.6).abs() < 1e-12);
        let y = -ba / 2.0;
        let whole = s.bends();
        assert_eq!(whole.len(), 1);
        assert!(whole[0].length == 200.0 && whole[0].start == Vec2::new(0.0, y) && whole[0].end == Vec2::new(200.0, y), "{whole:?}");
        let spans = |s: &SheetBody| -> Vec<(usize, f64, f64)> {
            let b = s.bends();
            assert!(b.iter().all(|b| (b.start.y - y).abs() < 1e-12 && (b.end.y - y).abs() < 1e-12 && (b.length - b.start.dist(b.end)).abs() < 1e-12));
            assert!(b.iter().all(|b| b.up && b.allowance == ba && b.radius == 2.5 && (b.angle_deg - 90.0).abs() < 1e-12));
            b.iter().map(|b| (b.index, b.start.x, b.end.x)).collect()
        };
        let close = |got: &[(usize, f64, f64)], want: &[(usize, f64, f64)]| {
            got.len() == want.len() && got.iter().zip(want).all(|(g, w)| g.0 == w.0 && (g.1 - w.1).abs() < 1e-9 && (g.2 - w.2).abs() < 1e-9)
        };
        let block = |x0: f64, x1: f64, y0: f64, y1: f64| rect(Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1));
        let disc = |x: f64, y: f64, r: f64| Region2 { outer: Loop2::circle(Vec2::new(x, y), r), holes: vec![] };
        let with = |cuts: Vec<Region2>| {
            let mut c = s.clone();
            c.holes.extend(cuts);
            c
        };
        // A cut that takes the flange's left half, bend and all (and a strip of the base).
        let half = with(vec![block(-10.0, 100.0, -60.0, 10.0)]);
        assert!(close(&spans(&half), &[(1, 100.0, 200.0)]), "{:?}", half.bends());
        // A slot across the bend: two stretches, both the same bend.
        let slot = with(vec![block(90.0, 110.0, -20.0, 10.0)]);
        assert!(close(&spans(&slot), &[(1, 0.0, 90.0), (1, 110.0, 200.0)]), "{:?}", slot.bends());
        // A round hole on the centre line takes its diameter; one in the zone beside the line
        // (0.5 across, 2 mm off it) takes nothing.
        let hole = with(vec![disc(50.0, y, 1.0), disc(150.0, y + 2.0, 0.25)]);
        assert!(close(&spans(&hole), &[(1, 0.0, 49.0), (1, 51.0, 200.0)]), "{:?}", hole.bends());
        // A hole crossed off its centre takes its chord there: 2·√(3² − 1²).
        let chord = with(vec![disc(50.0, y + 1.0, 3.0)]);
        let c = 8f64.sqrt();
        assert!(close(&spans(&chord), &[(1, 0.0, 50.0 - c), (1, 50.0 + c, 200.0)]), "{:?}", chord.bends());
        // A ring leaves the island inside it its stretch of the bend.
        let ring = with(vec![Region2 { outer: Loop2::circle(Vec2::new(150.0, y), 10.0), holes: vec![Loop2::circle(Vec2::new(150.0, y), 4.0)] }]);
        assert!(close(&spans(&ring), &[(1, 0.0, 140.0), (1, 146.0, 154.0), (1, 160.0, 200.0)]), "{:?}", ring.bends());
        // A cut over the whole bend zone leaves no bend, and cuts elsewhere leave it whole.
        assert!(with(vec![block(-1.0, 201.0, -ba - 0.5, 0.5)]).bends().is_empty());
        assert_eq!(with(vec![disc(50.0, 50.0, 10.0), block(20.0, 40.0, -30.0, -10.0)]).bends(), whole);
        // Pockets count where together they go through the sheet.
        let mut pocketed = s.clone();
        pocketed.record_pocket(block(90.0, 110.0, -20.0, 10.0), 0.0, 1.0, &mut FlatWork::new()).unwrap();
        assert_eq!(pocketed.bends(), whole);
        pocketed.record_pocket(block(80.0, 105.0, -20.0, 10.0), 1.0, 2.5, &mut FlatWork::new()).unwrap();
        assert!(close(&spans(&pocketed), &[(1, 0.0, 90.0), (1, 105.0, 200.0)]), "{:?}", pocketed.bends());
        // A second flange keeps its own number.
        let mut two = half.clone();
        two.add_flange(Vec3::new(100.0, 100.0, 2.5), 40.0, std::f64::consts::FRAC_PI_2, 2.5, "outside", false).unwrap();
        let b = two.bends();
        assert!(b.len() == 2 && b[0].index == 1 && (b[0].length - 100.0).abs() < 1e-9 && b[1].index == 2 && b[1].length == 200.0, "{b:?}");
    }

    #[test]
    fn flat_faces_of_a_sheet_are_found_and_named_by_panel() {
        use std::f64::consts::FRAC_PI_2;
        // Steel, 2.5 thick, R = T: a 90° flange on the y = 0 edge stands between the planes
        // y = 0 (its outer face) and y = 2.5 (its inner face) from z = 5 up.
        let mut s = plate(100.0, 60.0);
        s.add_flange(Vec3::new(50.0, 0.0, 2.5), 20.0, FRAC_PI_2, 2.5, "inside", false).unwrap();
        s.flanges[0].tag = "F7.0".into();
        let face = |s: &SheetBody, p: Vec3| s.panel_face_at(p, 1e-6);
        let (name, plane) = face(&s, Vec3::new(50.0, 30.0, 2.5)).unwrap();
        assert_eq!(name, "base:top");
        assert!(plane.normal().dist(Vec3::Z) < 1e-12 && (plane.origin.z - 2.5).abs() < 1e-12);
        // The top face's coordinates are the flat pattern's; the bottom face's mirror y.
        assert!(plane.to_local(Vec3::new(30.0, 20.0, 2.5)).dist(Vec2::new(30.0, 20.0)) < 1e-12);
        let (name, plane) = face(&s, Vec3::new(50.0, 30.0, 0.0)).unwrap();
        assert_eq!(name, "base:bottom");
        assert!(plane.normal().dist(Vec3::Z * -1.0) < 1e-12 && plane.origin.z.abs() < 1e-12);
        assert!(plane.to_local(Vec3::new(30.0, 20.0, 0.0)).dist(Vec2::new(30.0, -20.0)) < 1e-12);
        // The flange's inner face is the sheet's top side there; 7 mm up it is flat y = 5 − BA − 7.
        let ba = FRAC_PI_2 * (2.5 + 0.44 * 2.5);
        let (inner, plane) = face(&s, Vec3::new(30.0, 2.5, 12.0)).unwrap();
        assert_eq!(inner, "F7.0:top");
        assert!(plane.normal().dist(Vec3::Y) < 1e-12, "{:?}", plane.normal());
        assert!(plane.to_local(Vec3::new(30.0, 2.5, 12.0)).dist(Vec2::new(30.0, 5.0 - ba - 7.0)) < 1e-9);
        let (outer, plane) = face(&s, Vec3::new(30.0, 0.0, 12.0)).unwrap();
        assert_eq!(outer, "F7.0:bottom");
        assert!(plane.normal().dist(Vec3::Y * -1.0) < 1e-12, "{:?}", plane.normal());
        // A name gives the same plane again.
        for p in [Vec3::new(50.0, 30.0, 2.5), Vec3::new(50.0, 30.0, 0.0), Vec3::new(30.0, 2.5, 12.0), Vec3::new(30.0, 0.0, 12.0)] {
            let (name, plane) = face(&s, p).unwrap();
            assert_eq!(s.panel_face(&name), Some(plane), "{name}");
        }
        // Inside the sheet, on the bend, beside the sheet, above the flange: no flat face.
        for p in [Vec3::new(50.0, 30.0, 1.0), Vec3::new(50.0, 1.0, 1.0), Vec3::new(150.0, 30.0, 2.5), Vec3::new(30.0, 2.5, 40.0)] {
            assert!(face(&s, p).is_none(), "{p:?}");
        }
        // Names that are no face, or no flange, give no plane.
        for name in ["base", "base:side", "nobody:top", "#7:top", "F8.0:top", ""] {
            assert!(s.panel_face(name).is_none(), "{name}");
        }
        // A flange without a tag is named by its index.
        s.flanges[0].tag.clear();
        assert_eq!(face(&s, Vec3::new(30.0, 2.5, 12.0)).map(|f| f.0).as_deref(), Some("#0:top"));
        assert!(s.panel_face("#0:bottom").is_some());
        // Unfolded, the whole sheet is one panel.
        s.flat = true;
        assert_eq!(face(&s, Vec3::new(50.0, -10.0, 2.5)).map(|f| f.0).as_deref(), Some("flat:top"));
        assert!(s.panel_face("flat:bottom").is_some());
    }

    #[test]
    fn a_pick_beside_a_thin_sheet_takes_the_nearer_face() {
        // The tolerance of a pick on a large part is far above a thin sheet's thickness, and
        // never reaches a quarter of it here: the nearer side wins, the middle is no face.
        let rv = RuleValues { thickness: 0.5, ..steel() };
        let s = base_flange(
            "Thin",
            "Sheet",
            &rv,
            Plane::XY,
            rect(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(100.0, 60.0), Vec2::new(0.0, 60.0)),
        );
        let at = |z: f64| s.panel_face_at(Vec3::new(50.0, 30.0, z), 5.0).map(|f| f.0);
        assert_eq!(at(0.4).as_deref(), Some("base:top"));
        assert_eq!(at(0.1).as_deref(), Some("base:bottom"));
        assert_eq!(at(0.25), None);
        assert_eq!(at(3.0), None);
    }

    #[test]
    fn sheet_face_names_split_from_the_right() {
        assert_eq!(split_sheet_face_name(&sheet_face_name("Body1", "F7.0:top")), Some(("Body1", "F7.0:top")));
        // A body name may hold colons; the panel and the side never do.
        assert_eq!(split_sheet_face_name("sheet:Part:2:base:bottom"), Some(("Part:2", "base:bottom")));
        for name in ["", "sheet", "sheet:Body1", "sheet:Body1:top", "sheet::base:top", "sheet:Body1::top", "sheet:Body1:base:", "Body1:base:top"] {
            assert_eq!(split_sheet_face_name(name), None, "{name}");
        }
    }

    #[test]
    fn contour_flange_matches_fusion() {
        // Oracle 72: XZ chain (0,0)→(40,0)→(40,−30)→(70,−30), 50 wide.
        let xz = Plane::new(Vec3::ZERO, Vec3::X, Vec3::new(0.0, 0.0, -1.0)).unwrap();
        let pts = [Vec2::new(0.0, 0.0), Vec2::new(40.0, 0.0), Vec2::new(40.0, -30.0), Vec2::new(70.0, -30.0)];
        let s = contour_flange("Body1", "Steel (mm)", &steel(), &xz, &pts, 50.0, false).unwrap();
        let b = s.solid().unwrap();
        assert!((vol(&b) - 12097.622).abs() < 1.2, "{}", vol(&b));
        let m = kernel::measure(&b).unwrap();
        assert!((m.bbox.max.z - 32.5).abs() < 1e-6 && (m.bbox.max.y - 50.0).abs() < 1e-6, "{:?}", m.bbox);
        assert!((s.flat_size().unwrap().0 - 96.309734).abs() < 1e-5, "{:?}", s.flat_size());
    }

    #[test]
    fn hem_and_holes() {
        // Oracle 73: flange then a flat hem of 8 on its top edge.
        let mut s = plate(100.0, 60.0);
        s.add_flange(Vec3::new(50.0, 0.0, 2.5), 20.0, std::f64::consts::FRAC_PI_2, 2.5, "inside", false).unwrap();
        s.add_hem(Vec3::new(50.0, 0.0, 20.0), 8.0, 0.01, false).unwrap();
        let b = s.solid().unwrap();
        assert!((vol(&b) - 21334.723).abs() < 2.0, "{}", vol(&b));
        assert!((s.flat_size().unwrap().1 - 84.632035).abs() < 1e-4, "{:?}", s.flat_size());
        // A hole in the base and a slot across the bend.
        let mut s = plate(80.0, 50.0);
        s.add_flange(Vec3::new(40.0, 0.0, 2.5), 20.0, std::f64::consts::FRAC_PI_2, 2.5, "inside", false).unwrap();
        s.holes.push(Region2 { outer: Loop2::circle(Vec2::new(40.0, 30.0), 3.0), holes: vec![] });
        let v0 = vol(&s.solid().unwrap());
        s.holes.push(rect(Vec2::new(35.0, -12.0), Vec2::new(45.0, -12.0), Vec2::new(45.0, 10.0), Vec2::new(35.0, 10.0)));
        let v1 = vol(&s.solid().unwrap());
        let ba = std::f64::consts::FRAC_PI_2 * 3.6;
        let want = 10.0 * 2.5 * (22.0 - ba) + 10.0 * std::f64::consts::FRAC_PI_2 * (2.5 + 1.25) * 2.5;
        assert!((v0 - v1 - want).abs() < 1.0, "{v0} {v1} {want}");
        s.flat = true;
        assert!(s.solid().is_ok());
    }

    #[test]
    fn disconnected_material_keeps_each_hole_with_its_own_region() {
        let mut s = plate(100.0, 60.0);
        s.holes.push(rect(Vec2::new(45.0, -5.0), Vec2::new(55.0, -5.0), Vec2::new(55.0, 65.0), Vec2::new(45.0, 65.0)));
        s.holes.push(Region2 { outer: Loop2::circle(Vec2::new(20.0, 30.0), 5.0), holes: vec![] });
        let regions = s.flat_regions().unwrap();
        assert_eq!(regions.len(), 2);
        let left = regions.iter().find(|r| r.outer.contains(Vec2::new(10.0, 10.0))).unwrap();
        let right = regions.iter().find(|r| r.outer.contains(Vec2::new(90.0, 10.0))).unwrap();
        assert_eq!(left.holes.len(), 1);
        assert!(left.holes[0].segs.iter().all(|s| matches!(s, Seg2::Arc { radius, .. } if (*radius - 5.0).abs() < 1e-9)));
        assert!(right.holes.is_empty());
        let area: f64 = regions.iter().map(Region2::area).sum();
        assert!((area - (5400.0 - std::f64::consts::PI * 25.0)).abs() < 1e-6);
        assert!((vol(&s.flat_solid().unwrap()) - area * 2.5).abs() < 0.1);
    }

    #[test]
    fn ring_cut_retains_exact_central_material_island() {
        let mut s = plate(100.0, 60.0);
        s.holes.push(Region2 { outer: Loop2::circle(Vec2::new(50.0, 30.0), 10.0), holes: vec![Loop2::circle(Vec2::new(50.0, 30.0), 5.0)] });
        let regions = s.flat_regions().unwrap();
        assert_eq!(regions.len(), 2);
        let island = regions.iter().find(|r| r.outer.contains(Vec2::new(50.0, 30.0))).unwrap();
        assert!(island.holes.is_empty());
        assert!(island.outer.segs.iter().all(|s| matches!(s, Seg2::Arc { radius, .. } if (*radius - 5.0).abs() < 1e-9)));
        let area: f64 = regions.iter().map(Region2::area).sum();
        assert!((area - (6000.0 - std::f64::consts::PI * 75.0)).abs() < 1e-6);
        assert!((vol(&s.flat_solid().unwrap()) - area * 2.5).abs() < 0.1);
    }

    #[test]
    fn freeform_profile_stays_exact_with_exact_round_holes() {
        let mut s = plate(100.0, 60.0);
        let cubic =
            Seg2::Cubic { p0: Vec2::new(100.0, 60.0), p1: Vec2::new(200.0 / 3.0, 80.0), p2: Vec2::new(100.0 / 3.0, 80.0), p3: Vec2::new(0.0, 60.0) };
        s.base.outer.segs = vec![
            Seg2::Line { a: Vec2::ZERO, b: Vec2::new(100.0, 0.0) },
            Seg2::Line { a: Vec2::new(100.0, 0.0), b: Vec2::new(100.0, 60.0) },
            cubic,
            Seg2::Line { a: Vec2::new(0.0, 60.0), b: Vec2::ZERO },
        ];
        s.holes.push(Region2 { outer: Loop2::circle(Vec2::new(50.0, 30.0), 3.0), holes: vec![] });
        let regions = s.flat_regions().unwrap();
        // The outline the cut leaves whole keeps its exact cubic, the hole its exact arcs.
        assert_eq!(regions[0].outer.segs.iter().filter(|s| matches!(s, Seg2::Line { .. })).count(), 3);
        assert!(regions[0].outer.segs.contains(&cubic), "{:?}", regions[0].outer);
        assert!(regions[0].holes[0].segs.iter().all(|s| matches!(s, Seg2::Arc { .. })));
        // The cubic has x=100(1-t), y=60+60t(1-t), hence an exact blank area of 7000.
        assert!((regions[0].area() - (7000.0 - std::f64::consts::PI * 9.0)).abs() < 1e-9, "{}", regions[0].area());
        let expected = (7000.0 - std::f64::consts::PI * 9.0) * 2.5;
        assert!((vol(&s.solid().unwrap()) - expected).abs() < 0.5);
        assert!((vol(&s.flat_solid().unwrap()) - expected).abs() < 0.5);
    }

    /// An ellipse about `c` with semi-axes `a` (along x) and `b`, as four exact conics.
    fn ellipse(c: Vec2, a: f64, b: f64) -> Region2 {
        let corners = [c + Vec2::new(a, 0.0), c + Vec2::new(0.0, b), c - Vec2::new(a, 0.0), c - Vec2::new(0.0, b)];
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let segs = (0..4).map(|i| Seg2::Conic { a: corners[i], apex: corners[i] + corners[(i + 1) % 4] - c, b: corners[(i + 1) % 4], w }).collect();
        Region2 { outer: Loop2 { segs }, holes: vec![] }
    }

    #[test]
    fn free_form_cut_outs_stay_exact_in_the_flat_pattern_and_follow_arcs_in_the_folded_solid() {
        let mut s = plate(100.0, 60.0);
        s.add_flange(Vec3::new(50.0, 0.0, 2.5), 20.0, std::f64::consts::FRAC_PI_2, 2.5, "inside", false).unwrap();
        let (blank, folded) = (s.flat_regions().unwrap()[0].area(), vol(&s.solid().unwrap()));
        let hole = ellipse(Vec2::new(40.0, 30.0), 15.0, 8.0);
        let cut = std::f64::consts::PI * 15.0 * 8.0;
        s.holes.push(hole.clone());
        // The flat pattern's cut-out is the ellipse itself (four conics, running as a hole) and
        // its area is the blank less π·15·8.
        let regions = s.flat_regions().unwrap();
        assert_eq!(regions.len(), 1);
        let h = &regions[0].holes[0];
        assert!(h.segs.len() == 4 && h.segs.iter().all(|x| hole.outer.segs.contains(&x.reversed())), "{h:?}");
        assert!((regions[0].area() - (blank - cut)).abs() < 1e-9, "{} vs {}", regions[0].area(), blank - cut);
        // The folded solid is sewn from lines and arcs within the fit tolerance (times the
        // perimeter, 74 mm): fewer than half the 264 chords of 0.001 mm.
        let cells = s.cells_with_work(&mut FlatWork::new()).unwrap();
        let base = cells.iter().find(|c| c.piece == Piece::Panel(None)).unwrap();
        assert!(base.holes.len() == 1 && base.holes[0].len() < 132 && base.holes[0].iter().all(Seg2::is_line_or_arc), "{}", base.holes[0].len());
        assert!((Loop2 { segs: base.holes[0].clone() }.signed_area().abs() - cut).abs() < kernel::PLANAR_FIT_TOL * 74.0);
        assert!((folded - vol(&s.solid().unwrap()) - cut * 2.5).abs() < 0.5);
        // The flat solid has the ellipse's wall as four faces.
        let flat = s.flat_solid().unwrap();
        assert!((vol(&flat) - (blank - cut) * 2.5).abs() < 0.5);
        assert_eq!(flat.face_count(), regions[0].outer.segs.len() + 4 + 2);
    }

    #[test]
    fn regions_map_into_the_flat_frame_with_their_exact_curves() {
        let s = plate(100.0, 60.0);
        let hole = ellipse(Vec2::new(40.0, 30.0), 15.0, 8.0);
        // Drawn on the sheet's own plane: the region as it is.
        assert_eq!(s.region_to_flat(None, &Plane::XY, &hole).unwrap(), hole);
        // Drawn from below (y mirrored) at (40, −30), with a cubic and a quartic lobe on its
        // right: the same ellipse on the sheet, every curve through its mirrored controls.
        let below = Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Y * -1.0 };
        let mirror = |p: Vec2| Vec2::new(p.x, -p.y);
        let seen = Region2 { outer: Loop2 { segs: hole.outer.segs.iter().map(|x| map_controls(x, mirror)).collect() }, holes: vec![] };
        let mapped = s.region_to_flat(None, &below, &seen).unwrap();
        assert!(mapped.outer.is_ccw() && mapped.outer.segs.len() == 4 && mapped.outer.segs.iter().all(|x| hole.outer.segs.contains(x)), "{mapped:?}");
        // A cubic and a quartic closed by a line, clockwise in that plane: counter-clockwise on
        // the sheet, every control point mirrored.
        let (a, b, c) = (Vec2::new(60.0, -20.0), Vec2::new(60.0, -40.0), Vec2::new(80.0, -30.0));
        let (cubic, quartic) = (
            [a, a + Vec2::new(10.0, 4.0), c + Vec2::new(0.0, 8.0), c],
            [c, c + Vec2::new(0.0, -6.0), c + Vec2::new(-6.0, -12.0), b + Vec2::new(8.0, -2.0), b],
        );
        let lobe = |f: fn(Vec2) -> Vec2| Loop2 {
            segs: vec![
                solidcraft_geom::bezier(&cubic.map(f)).unwrap(),
                solidcraft_geom::bezier(&quartic.map(f)).unwrap(),
                Seg2::Line { a: f(b), b: f(a) },
            ],
        };
        let drawn = lobe(|p| p);
        let mapped = s.region_to_flat(None, &below, &Region2 { outer: drawn.clone(), holes: vec![] }).unwrap();
        assert_eq!(mapped.outer, lobe(mirror));
        assert!(mapped.holes.is_empty() && (mapped.area() + drawn.signed_area()).abs() < 1e-9 && mapped.area() > 100.0, "{}", mapped.area());
    }

    #[test]
    fn pockets_with_free_form_outlines_are_machined_beside_free_form_cut_outs() {
        let mut s = plate(100.0, 60.0);
        s.holes.push(ellipse(Vec2::new(30.0, 30.0), 15.0, 8.0));
        let pocket = ellipse(Vec2::new(70.0, 30.0), 10.0, 6.0);
        s.record_pocket(pocket.clone(), 0.0, 1.0, &mut FlatWork::new()).unwrap();
        // The plate less the through ellipse (π·15·8 of 2.5 mm) and the pocket (π·10·6 of 1 mm).
        let expected = (6000.0 - std::f64::consts::PI * 120.0) * 2.5 - std::f64::consts::PI * 60.0;
        assert!((vol(&s.solid().unwrap()) - expected).abs() < 0.5, "{}", vol(&s.solid().unwrap()));
        assert!((vol(&s.flat_solid().unwrap()) - expected).abs() < 0.5, "{}", vol(&s.flat_solid().unwrap()));
        // The machining record keeps the pocket's exact outline.
        let machined = s.machining().unwrap();
        assert!(machined.len() == 1 && machined[0].region.outer == pocket.outer, "{} records", machined.len());
    }

    #[test]
    fn flat_geometry_budgets_reject_aggregate_and_intermediate_work() {
        let mut s = plate(100.0, 60.0);
        // Each arc is individually legal, but their combined sampled points exceed the cap.
        s.base.outer.segs = vec![Seg2::Arc { center: Vec2::ZERO, radius: 1000.0, start: 0.0, sweep: std::f64::consts::TAU }; 32];
        assert!(s.flat_regions().unwrap_err().to_string().contains("budget"));
        let mut s = plate(100.0, 60.0);
        s.holes = vec![Region2 { outer: Loop2::circle(Vec2::new(20.0, 20.0), 1.0), holes: vec![] }; MAX_FLAT_PIECES + 1];
        assert!(s.flat_regions().unwrap_err().to_string().contains("budget"));
        let p = vec![vec![Vec2::ZERO; MAX_FLAT_POINTS]];
        assert!(merge_loops(&p, &mut FlatWork::new()).unwrap_err().to_string().contains("budget"));
        // Exhausting a clipping/merge allowance must report an error before work begins.
        assert!(merge_loops(&[vec![Vec2::ZERO, Vec2::X, Vec2::Y]], &mut FlatWork(0)).is_err());
    }

    #[test]
    fn round_cutouts_count_as_exact_arcs_in_the_flat_budget() {
        // 240 round 4 mm holes sample to over 16 384 chords of 0.001 mm, but are 480 exact arcs.
        let mut s = plate(250.0, 130.0);
        for i in 0..24 {
            for j in 0..10 {
                s.holes.push(Region2 { outer: Loop2::circle(Vec2::new(5.0 + 10.0 * i as f64, 15.0 + 10.0 * j as f64), 2.0), holes: vec![] });
            }
        }
        let regions = s.flat_regions().unwrap();
        assert_eq!(regions[0].holes.len(), 240);
        let area = 250.0 * 130.0 - 240.0 * std::f64::consts::PI * 2.0 * 2.0;
        assert!((regions[0].area() - area).abs() < 1e-6);
    }

    #[test]
    fn overlapping_round_cutouts_union_with_exact_arc_boundaries() {
        let mut s = plate(100.0, 60.0);
        s.holes.push(Region2 { outer: Loop2::circle(Vec2::new(20.0, 30.0), 5.0), holes: vec![] });
        s.holes.push(Region2 { outer: Loop2::circle(Vec2::new(22.0, 30.0), 5.0), holes: vec![] });
        let regions = s.flat_regions().unwrap();
        assert_eq!(regions[0].holes.len(), 1);
        assert!(regions[0].holes[0].segs.iter().all(|s| matches!(s, Seg2::Arc { radius, .. } if (*radius - 5.0).abs() < 1e-9)));
        let intersection = 50.0 * (0.2f64).acos() - (96.0f64).sqrt();
        let area = 6000.0 - (50.0 * std::f64::consts::PI - intersection);
        assert!((regions[0].area() - area).abs() < 1e-6);
        assert!((vol(&s.solid().unwrap()) - area * 2.5).abs() < 0.1);
    }

    #[test]
    fn nested_and_repeated_round_cutouts_keep_only_the_largest_exact_boundary() {
        for reverse in [false, true] {
            let mut s = plate(100.0, 60.0);
            let mut radii = vec![3.0, 5.0, 5.0];
            if reverse {
                radii.reverse();
            }
            for radius in radii {
                s.holes.push(Region2 { outer: Loop2::circle(Vec2::new(20.0, 30.0), radius), holes: vec![] });
            }
            let regions = s.flat_regions().unwrap();
            assert_eq!(regions[0].holes.len(), 1);
            assert!((regions[0].area() - (6000.0 - std::f64::consts::PI * 25.0)).abs() < 1e-6);
            let volume = vol(&s.solid().unwrap());
            // Body measure tessellates curved faces; the exact region area above is the
            // stronger analytic check, while this follows the existing sheet measure budget.
            assert!((volume - (6000.0 - std::f64::consts::PI * 25.0) * 2.5).abs() < 0.1, "{volume}");
            assert_eq!(s.round_hole_correction(), (-1, -4, -2));
        }
    }

    #[test]
    fn sewing_budget_rejects_edge_splitting_growth_before_allocating_faces() {
        let s = plate(100.0, 60.0);
        let cells: Vec<Cell> = (0..250)
            .map(|i| Cell {
                piece: Piece::Panel(None),
                outer: Loop2::polygon(&[Vec2::new(i as f64, 0.0), Vec2::new(250.0, 0.0), Vec2::new(i as f64, 1.0)]).segs,
                holes: vec![],
            })
            .collect();
        assert!(s.faces(&cells, 0.0, 2.5, &mut FlatWork::new()).unwrap_err().to_string().contains("budget"));
        assert!(s.faces(&cells[..1], 0.0, 2.5, &mut FlatWork(0)).unwrap_err().to_string().contains("budget"));
    }

    #[test]
    fn pocket_geometry_and_depth_budgets_are_checked_before_replay() {
        let pocket = SheetPocket {
            region: rect(Vec2::new(20.0, 20.0), Vec2::new(30.0, 20.0), Vec2::new(30.0, 30.0), Vec2::new(20.0, 30.0)),
            z0: 0.0,
            z1: 1.0,
        };
        let mut s = plate(100.0, 60.0);
        s.pockets = vec![pocket.clone(); 1001];
        assert!(s.flat_solid().unwrap_err().to_string().contains("budget"));
        s.pockets = vec![pocket.clone()];
        s.pockets[0].region.outer.segs = vec![Seg2::Line { a: Vec2::ZERO, b: Vec2::X }; MAX_FLAT_POINTS + 1];
        assert!(s.flat_solid().unwrap_err().to_string().contains("budget"));
        for (z0, z1) in [(f64::NAN, 1.0), (-1.0, 1.0), (0.0, 3.0), (1.0, 1.0)] {
            s.pockets = vec![SheetPocket { z0, z1, ..pocket.clone() }];
            assert!(s.flat_solid().unwrap_err().to_string().contains("depth"));
        }
    }
    #[test]
    fn pocket_recording_deduplicates_at_limit_and_rejects_growth_before_mutation() {
        let mut s = plate(100.0, 60.0);
        let pocket =
            SheetPocket { region: rect(Vec2::new(1.0, 1.0), Vec2::new(2.0, 1.0), Vec2::new(2.0, 2.0), Vec2::new(1.0, 2.0)), z0: 0.0, z1: 0.5 };
        s.pockets = vec![pocket.clone(); 1000];
        let mut work = FlatWork::new();
        s.record_pocket(pocket.region.clone(), pocket.z0, pocket.z1, &mut work).unwrap();
        assert_eq!(s.pockets.len(), 1000);
        assert!(s.record_pocket(pocket.region.clone(), 0.0, 0.6, &mut work).is_err());
        assert_eq!(s.pockets.len(), 1000);
        s.pockets.push(pocket.clone());
        assert!(s.record_pocket(pocket.region, 0.0, 0.5, &mut work).is_err());
    }

    /// A sketch solved again after a parameter change leaves its corners off by ~1e-13 mm, so
    /// its edges are not quite square to the axes. Such a tray (four 72° flanges, shrunk to
    /// 400 x 300 from a larger size) failed to unfold in the planar arrangement while the same
    /// tray drawn at that size unfolded.
    #[test]
    fn nearly_square_base_unfolds_like_the_exact_one() {
        let rv = RuleValues { thickness: 2.0, k_factor: 0.5, bend_radius: 1.0, ..steel() };
        let tray = |[a, b, c, d]: [Vec2; 4]| {
            let mut s = base_flange("Body1", "AL2", &rv, Plane::XY, rect(a, b, c, d));
            let picks = [Vec3::new(200.0, 0.0, 2.0), Vec3::new(400.0, 150.0, 2.0), Vec3::new(200.0, 300.0, 2.0), Vec3::new(0.0, 150.0, 2.0)];
            s.add_flanges(&picks, 38.6, 72f64.to_radians(), 1.0, "inside", false).unwrap();
            s
        };
        let flat_area = |s: &SheetBody| s.flat_regions().unwrap().iter().map(|r| r.area().abs()).sum::<f64>();
        let square = [Vec2::new(0.0, 0.0), Vec2::new(400.0, 0.0), Vec2::new(400.0, 300.0), Vec2::new(0.0, 300.0)];
        let exact = flat_area(&tray(square));
        // The corners the solver left after the parameter change, and the rectangle turned by a hair.
        let solved = [
            Vec2::new(0.0, 0.0),
            Vec2::new(400.0000000000001, 4.369543929613394e-13),
            Vec2::new(400.00000000000017, 300.0000000000008),
            Vec2::new(0.0, 300.0000000000009),
        ];
        let (c, s) = (1e-14f64.cos(), 1e-14f64.sin());
        let turned = square.map(|p| Vec2::new(p.x * c - p.y * s, p.x * s + p.y * c));
        for corners in [solved, turned] {
            let mut sheet = tray(corners);
            assert!((flat_area(&sheet) - exact).abs() < 1e-6, "{} {exact}", flat_area(&sheet));
            sheet.flat = true;
            assert!((vol(&sheet.solid().unwrap()) - exact * 2.0).abs() < 1e-3);
        }
    }

    /// A 100 × 80 plate with a 20 mm flange on its y = 0 edge: 2 mm sheet, inner radius 1,
    /// K 0.5. The neutral radius is the mid radius, so the folded sheet holds exactly its flat
    /// area times the thickness.
    fn flanged(angle: f64) -> SheetBody {
        let rule = RuleValues {
            thickness: 2.0,
            k_factor: 0.5,
            bend_radius: 1.0,
            relief_width: 2.0,
            relief_depth: 1.0,
            corner_relief: 8.0,
            corner_relief_shape: ReliefShape::Notch,
            hem_gap: 0.01,
            gap: 2.0,
        };
        let r = rect(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(100.0, 80.0), Vec2::new(0.0, 80.0));
        let mut s = base_flange("Body1", "AL2", &rule, Plane::XY, r);
        s.add_flanges(&[Vec3::new(50.0, 0.0, 2.0)], 20.0, angle, 1.0, "inside", false).unwrap();
        s
    }

    #[test]
    fn slanted_and_round_cut_outs_across_a_bend_fold() {
        let area = |s: &SheetBody| s.flat_regions().unwrap().iter().map(Region2::area).sum::<f64>();
        for angle in [90f64, 72.0, 108.0] {
            let plain = flanged(angle.to_radians());
            let (a0, v0) = (area(&plain), vol(&plain.solid().unwrap()));
            // A 10 × 20 slot turned 30° about (50, 0) and a Ø8 circle at (30, 0.5): both reach
            // from the base over the whole bend zone into the flange.
            let (c, a) = (Vec2::new(50.0, 0.0), 30f64.to_radians());
            let (ux, uy) = (Vec2::from_angle(a) * 5.0, Vec2::from_angle(a + std::f64::consts::FRAC_PI_2) * 10.0);
            let slot = Region2 { outer: Loop2::polygon(&[c - ux - uy, c + ux - uy, c + ux + uy, c - ux + uy]).ccw(), holes: vec![] };
            let circle = Region2 { outer: Loop2::circle(Vec2::new(30.0, 0.5), 4.0), holes: vec![] };
            for (cut, removed) in [(slot, 200.0), (circle, std::f64::consts::PI * 16.0)] {
                let mut s = plain.clone();
                s.holes.push(cut);
                assert!((a0 - area(&s) - removed).abs() < 1e-6, "{angle}°: flat area {}", a0 - area(&s));
                let body = s.solid().unwrap();
                assert!(body.validity().is_empty(), "{angle}°: {:?}", body.validity());
                // The folded sheet loses the cut-out's flat area times the thickness.
                assert!((v0 - vol(&body) - removed * 2.0).abs() < 0.05, "{angle}°: {} vs {}", v0 - vol(&body), removed * 2.0);
            }
        }
    }

    #[test]
    fn a_slanted_flat_edge_folds_to_its_exact_helix() {
        // A 72° flange, inside: the bend's axis runs along x through y = setback, z = T + R,
        // and a point at height z in the sheet stays T + R − z away from it.
        let angle = 72f64.to_radians();
        let s = flanged(angle);
        let f = s.flanges[0].clone();
        let setback = 3.0 * (angle / 2.0).tan();
        // The unclamped zone map agrees with the fold inside the zone.
        for (sv, u, z) in [(0.3, 10.0, 0.0), (1.2, 40.0, 1.0), (2.4, 90.0, 2.0)] {
            let q = f.p + f.n * sv + f.d * u;
            assert!(s.zone_point(&f, q, z).0.dist(s.map3(Piece::Zone(0), q, z)) < 1e-12);
        }
        // A flat line across the whole zone, 15 mm along the bend: the bend angle and the
        // distance along the axis both grow evenly along it. Two pieces of degree 7 follow it
        // (cubic ones took 32).
        let e = Seg2::Line { a: f.p + f.d * 20.0, b: f.p + f.n * s.allowance(&f) + f.d * 35.0 };
        let n = s.zone_pieces(&f, &e);
        assert_eq!(n, 2);
        for z in [0.0, 2.0] {
            let helix = |t: f64| {
                let (phi, rho) = (angle * t, 3.0 - z);
                Vec3::new(e.point_at(t).x, setback - rho * phi.sin(), 3.0 - rho * phi.cos())
            };
            let EdgeSpec::Chain { degree: 7, ctrl } = s.zone_chain(&f, &e, z) else { panic!("a slanted edge folds to a chain of degree 7") };
            assert_eq!(ctrl.len(), 7 * n + 1);
            for k in 0..n {
                for i in 0..=40 {
                    let x = i as f64 / 40.0;
                    let off = bezier_at(&ctrl[7 * k..=7 * k + 7], x).dist(helix((k as f64 + x) / n as f64));
                    assert!(off < BEND_CHAIN_TOL, "height {z}, piece {k}, {x}: {off}");
                }
            }
            // The pieces are equal, so the chain is one smooth curve: on either side of a joint
            // the first three differences of the control points are the same.
            let diff = |p: &[Vec3]| p.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>();
            for k in 1..n {
                let (mut before, mut after) = (ctrl[7 * k - 3..=7 * k].to_vec(), ctrl[7 * k..=7 * k + 3].to_vec());
                for order in 1..=3 {
                    (before, after) = (diff(&before), diff(&after));
                    let (b, a) = (before[before.len() - 1], after[0]);
                    assert!(b.dist(a) < 1e-12, "height {z}, joint {k}, order {order}: {b:?} vs {a:?}");
                }
            }
        }
        // Lines along the bend and square to it keep their exact line and arc.
        let along = Seg2::Line { a: f.p + f.n * 1.0 + f.d * 20.0, b: f.p + f.n * 1.0 + f.d * 35.0 };
        let across = Seg2::Line { a: f.p + f.d * 20.0, b: f.p + f.n * s.allowance(&f) + f.d * 20.0 };
        assert!(matches!(s.edge3(Piece::Zone(0), &along, 2.0), Ok(EdgeSpec::Line { .. })));
        assert!(matches!(s.edge3(Piece::Zone(0), &across, 2.0), Ok(EdgeSpec::Arc { .. })));
    }

    /// 2 mm aluminium, R 1, K 0.5 (folding keeps the volume), flanges 0.5 mm apart at corners.
    fn alu() -> RuleValues {
        RuleValues {
            thickness: 2.0,
            k_factor: 0.5,
            bend_radius: 1.0,
            relief_width: 2.0,
            relief_depth: 1.0,
            corner_relief: 8.0,
            corner_relief_shape: ReliefShape::Notch,
            hem_gap: 0.01,
            gap: 0.5,
        }
    }

    /// A 120 x 90 tray: flanges `height` high at `deg` on all four edges (bottom, right, top, left).
    fn tray(rv: &RuleValues, deg: f64, height: f64) -> (SheetBody, Vec<String>) {
        let r = rect(Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0));
        let mut s = base_flange("Body1", "AL2", rv, Plane::XY, r);
        let t = rv.thickness;
        let picks = [Vec3::new(60.0, 0.0, t), Vec3::new(120.0, 45.0, t), Vec3::new(60.0, 90.0, t), Vec3::new(0.0, 45.0, t)];
        let (_, notes) = s.add_flanges_noted(&picks, height, deg.to_radians(), rv.bend_radius, "inside", false).unwrap();
        (s, notes)
    }

    fn flat_area(s: &SheetBody) -> f64 {
        s.flat_regions().unwrap().iter().map(Region2::area).sum()
    }

    /// A plate (0, 0) (100, 0) (85, 20) (0, 20) with a 30 mm inside flange on its slanted edge
    /// (2 mm sheet, R 2, K 0.5: the folded sheet holds its flat area times the thickness), and a
    /// 4 mm² triangle cut out of the flat across the bend line, with a corner on the line or a
    /// hair (0.01 to 0.1 µm) to either side of it, in the middle of the bend and near its ends.
    /// The planar booleans leave such a corner as corners a hair apart on the base and on the
    /// bend, and the refold joined them by an edge as short ("sew: a degenerate edge").
    #[test]
    fn a_cut_out_corner_a_hair_beside_a_bend_line_refolds() {
        let rule = RuleValues { bend_radius: 2.0, ..alu() };
        let outline = [Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(85.0, 20.0), Vec2::new(0.0, 20.0)];
        for deg in [90.0f64, 120.0] {
            let mut s = base_flange("Body1", "AL2", &rule, Plane::XY, Region2 { outer: Loop2::polygon(&outline).ccw(), holes: vec![] });
            s.add_flange(Vec3::new(92.5, 10.0, 2.0), 30.0, deg.to_radians(), 2.0, "inside", false).unwrap();
            let (a0, v0) = (flat_area(&s), vol(&s.solid().unwrap()));
            if deg == 90.0 {
                // The base less 4 mm along the edge's normal (5 mm along x): 1750; the bend
                // 25 x 3π/2; the flange 25 x 26.
                assert!((a0 - (2400.0 + 37.5 * std::f64::consts::PI)).abs() < 1e-9, "{a0}");
            }
            // The measure follows the bend by facets.
            assert!((v0 / (a0 * 2.0) - 1.0).abs() < 1e-4, "{deg}°: {v0} vs {}", a0 * 2.0);
            let f = s.flanges[0].clone();
            for (along, off) in [(0.5, 0.0), (0.5, 1e-8), (0.5, 5e-8), (0.5, -5e-8), (0.5, 1e-7), (0.5, -1e-7), (0.16, 5e-8), (0.84, -5e-8)] {
                let what = format!("{deg}°, corner {along} along the bend, {off} mm beside it");
                let c = f.p + f.d * (f.len * along) + f.n * off;
                let triangle = Loop2::polygon(&[c, c - f.n * 2.8 - f.d * 0.4, c + f.n * 0.4 - f.d * 2.8]).ccw();
                let mut cut = s.clone();
                cut.holes.push(Region2 { outer: triangle, holes: vec![] });
                assert!((a0 - flat_area(&cut) - 4.0).abs() < 1e-6, "{what}: flat area {}", a0 - flat_area(&cut));
                let body = cut.solid().unwrap_or_else(|e| panic!("{what}: {e}"));
                assert!(body.validity().is_empty(), "{what}: {:?}", body.validity());
                assert!((v0 - vol(&body) - 8.0).abs() < 0.01, "{what}: {} vs 8", v0 - vol(&body));
            }
        }
    }

    /// Distance between two parallel 3D lines through a0→a1 and b0→b1 (None if not parallel).
    fn parallel_gap(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> Option<f64> {
        let (u, v) = ((a1 - a0).normalized()?, (b1 - b0).normalized()?);
        (u.cross(v).len() < 1e-9).then(|| (b0 - a0).cross(u).len())
    }

    #[test]
    fn closed_corners_end_on_the_bisector_with_the_gap() {
        let (t, r, g, h) = (2.0, 1.0, 0.5, 38.6);
        for deg in [90.0f64, 72.0] {
            let (s, notes) = tray(&alu(), deg, h);
            assert!(notes.is_empty(), "{notes:?}");
            let th = deg.to_radians();
            let s0 = (r + t) * (th / 2.0).tan();
            let ba = th * (r + 0.5 * t);
            let leg = h - s0;
            // By hand, for the flange on y = 0 and its mirror image on x = 0: a point of the flange w up
            // its outer face from the virtual sharp (the original edge), h along the outward normal
            // (−T ≤ h ≤ 0) is (x, −w·cosθ − h·sinθ, w·sinθ − h·cosθ). Keeping x − y ≥ g/√2 on the
            // inner face (h = −T) gives the end x(w) = g/√2 + T·sinθ − w·cosθ; the panel spans w = s0 … h.
            let x_end = |w: f64| g / std::f64::consts::SQRT_2 + t * th.sin() - w * th.cos();
            let (y0, y1) = (s0 - ba, s0 - ba - leg);
            let want = [(x_end(s0), y0), (120.0 - x_end(s0), y0), (120.0 - x_end(h), y1), (x_end(h), y1)];
            let got = corners_of(&s.panel(&s.flanges[0]));
            assert_eq!(got.len(), 4);
            for (x, y) in want {
                assert!(got.iter().any(|p| p.dist(Vec2::new(x, y)) < 1e-9), "{deg}°: ({x}, {y}) not in {got:?}");
            }
            if deg == 90.0 {
                // The rule 90° corners always had: the panel runs on s0 − (T + g/√2) past its bend.
                assert!((s.flanges[0].ext.0 - (s0 - t - g / std::f64::consts::SQRT_2)).abs() < 1e-12);
                assert_eq!(s.flanges[0].slope, (0.0, 0.0));
            } else {
                assert!((s.flanges[0].slope.0 - th.cos()).abs() < 1e-12, "{:?}", s.flanges[0].slope);
            }
            // Folded, the inner edges of the two ends (bottom flange 0, left flange 3) run parallel,
            // `gap` apart.
            let (fa, fb) = (s.fold(0), s.fold(3));
            let pa = |x: f64, y: f64| apply_point(&fa, Vec3::new(x, y, t));
            let pb = |x: f64, y: f64| apply_point(&fb, Vec3::new(x, y, t));
            let gap = parallel_gap(pa(x_end(s0), y0), pa(x_end(h), y1), pb(y0, x_end(s0)), pb(y1, x_end(h)));
            assert!(gap.is_some_and(|d| (d - g).abs() < 1e-9), "{deg}°: {gap:?}");
            // The flat pattern is one piece and the folded part holds the same material (K = 0.5).
            let regions = s.flat_regions().unwrap();
            assert_eq!(regions.len(), 1);
            let b = s.solid().unwrap();
            assert!(b.validity().is_empty() && b.shell_count() == 1, "{deg}°: {:?}", b.validity());
            let v = vol(&b);
            assert!((v - flat_area(&s) * t).abs() < 1e-3 * v, "{deg}°: {v} vs {}", flat_area(&s) * t);
        }
    }

    #[test]
    fn a_flange_of_a_later_feature_leaves_the_corner_as_it_was() {
        // The convention of reference case 75-sm-bracket-holes: the earlier flange keeps its length
        // and the later one ends at its bend. Only flanges made together close toward each other.
        let rv = alu();
        let base = rect(Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0));
        let mut s = base_flange("Body1", "AL2", &rv, Plane::XY, base);
        s.add_flanges(&[Vec3::new(60.0, 0.0, 2.0)], 38.6, 72f64.to_radians(), 1.0, "inside", false).unwrap();
        let (made, notes) = s.add_flanges_noted(&[Vec3::new(120.0, 45.0, 2.0)], 25.0, 72f64.to_radians(), 1.0, "inside", false).unwrap();
        assert!(notes.is_empty() && s.corners(&made).is_empty(), "{notes:?}");
        assert!(s.flanges.iter().all(|f| f.ext == (0.0, 0.0) && f.slope == (0.0, 0.0)));
        assert!((s.flanges[0].len - 120.0).abs() < 1e-9);
        assert!(s.solid().is_ok() && s.flat_regions().unwrap().len() == 1);
    }

    #[test]
    fn corners_that_are_not_square_close_with_the_gap() {
        // A trapezoid: two acute and two obtuse corners.
        let pts = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(100.0, 90.0), Vec2::new(20.0, 90.0)];
        let base = Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] };
        let mut s = base_flange("Body1", "AL2", &alu(), Plane::XY, base);
        let picks = [Vec3::new(60.0, 0.0, 2.0), Vec3::new(110.0, 45.0, 2.0), Vec3::new(60.0, 90.0, 2.0), Vec3::new(10.0, 45.0, 2.0)];
        let (made, notes) = s.add_flanges_noted(&picks, 38.6, 72f64.to_radians(), 1.0, "inside", false).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let corners = s.corners(&made);
        assert_eq!(corners.len(), 4);
        // Each end's inner edge, folded.
        let edge = |i: usize, at_end: bool| {
            let f = &s.flanges[i];
            let ba = s.allowance(f);
            let (q0, q1) = if at_end {
                let b = f.p + f.n * ba + f.d * (f.len + f.ext.1);
                (b, b + f.n * f.leg + f.d * (f.slope.1 * f.leg))
            } else {
                let a = f.p + f.n * ba - f.d * f.ext.0;
                (a, a + f.n * f.leg - f.d * (f.slope.0 * f.leg))
            };
            let w = s.fold(i);
            (apply_point(&w, Vec3::new(q0.x, q0.y, 2.0)), apply_point(&w, Vec3::new(q1.x, q1.y, 2.0)))
        };
        for k in &corners {
            let ((a0, a1), (b0, b1)) = (edge(k.a, k.a_end), edge(k.b, k.b_end));
            let gap = parallel_gap(a0, a1, b0, b1);
            assert!(gap.is_some_and(|d| (d - 0.5).abs() < 1e-9), "{k:?}: {gap:?}");
        }
        let b = s.solid().unwrap();
        assert!(b.validity().is_empty() && b.shell_count() == 1, "{:?}", b.validity());
        assert!((vol(&b) - flat_area(&s) * 2.0).abs() < 1e-3 * vol(&b));
    }

    #[test]
    fn corners_that_cannot_close_stay_open_with_a_note() {
        // A gap of zero would make the ends touch.
        let mut rv = alu();
        rv.gap = 0.0;
        let (s, notes) = tray(&rv, 72.0, 38.6);
        assert_eq!(notes.len(), 4, "{notes:?}");
        assert!(notes.iter().all(|n| n.contains("left open") && n.contains("gap")), "{notes:?}");
        assert!(s.flanges.iter().all(|f| f.ext == (0.0, 0.0) && f.slope == (0.0, 0.0)));
        assert!(s.solid().is_ok());
        // Flanges bending opposite ways (the second edge picked on the bottom face).
        let base = rect(Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0));
        let mut s = base_flange("Body1", "AL2", &alu(), Plane::XY, base);
        let picks = [Vec3::new(60.0, 0.0, 2.0), Vec3::new(120.0, 45.0, 0.0)];
        let (_, notes) = s.add_flanges_noted(&picks, 20.0, 72f64.to_radians(), 1.0, "inside", false).unwrap();
        assert!(notes.len() == 1 && notes[0].contains("opposite"), "{notes:?}");
        assert!(s.solid().is_ok());
    }

    /// Folded, the inner edge of the end of flange `i`'s panel (`at_end`: the end past the far end
    /// of its bend).
    fn inner_end_edge(s: &SheetBody, i: usize, at_end: bool) -> (Vec3, Vec3) {
        let f = &s.flanges[i];
        let ba = s.allowance(f);
        let (q0, q1) = if at_end {
            let b = f.p + f.n * ba + f.d * (f.len + f.ext.1);
            (b, b + f.n * f.leg + f.d * (f.slope.1 * f.leg))
        } else {
            let a = f.p + f.n * ba - f.d * f.ext.0;
            (a, a + f.n * f.leg - f.d * (f.slope.0 * f.leg))
        };
        let (w, z) = (s.fold(i), if f.angle >= 0.0 { s.t } else { 0.0 });
        (apply_point(&w, Vec3::new(q0.x, q0.y, z)), apply_point(&w, Vec3::new(q1.x, q1.y, z)))
    }

    /// At each of `corners` the two panel ends' inner edges run parallel, `gap` apart.
    fn ends_are_gap_apart(s: &SheetBody, corners: &[Corner], gap: f64, what: &str) {
        for k in corners {
            let ((a0, a1), (b0, b1)) = (inner_end_edge(s, k.a, k.a_end), inner_end_edge(s, k.b, k.b_end));
            let got = parallel_gap(a0, a1, b0, b1);
            assert!(got.is_some_and(|d| (d - gap).abs() < 1e-9), "{what}: {k:?}: {got:?}");
        }
    }

    /// The flat pattern is one piece without hairline edges, and the folded part is one valid shell
    /// holding the same material (K = 0.5 keeps the volume).
    fn holds_the_flat_material(s: &SheetBody, what: &str) {
        let regions = s.flat_regions().unwrap_or_else(|e| panic!("{what}: no flat pattern: {e}"));
        assert_eq!(regions.len(), 1, "{what}");
        let shortest = regions[0].outer.segs.iter().map(Seg2::length).fold(f64::MAX, f64::min);
        assert!(regions[0].holes.is_empty() && shortest > 1e-3, "{what}: shortest outline edge {shortest:e}");
        assert_eq!(s.outline().unwrap_or_else(|e| panic!("{what}: no outline: {e}")).len(), 1, "{what}");
        let b = s.solid().unwrap_or_else(|e| panic!("{what}: no solid: {e}"));
        assert!(b.validity().is_empty() && b.shell_count() == 1, "{what}: {:?}", b.validity());
        let v = vol(&b);
        assert!((v - flat_area(s) * s.t).abs() < 1e-3 * v, "{what}: {v} vs {}", flat_area(s) * s.t);
    }

    /// The flat area a corner relief `size` across takes where the base fills the angle `alpha`
    /// and both bends are `ba` wide: of the base a sector of radius size/2 (round) or the kite
    /// with that side, square to both edges (rectangular), and size/2 of each bend's length.
    fn relief_area(shape: ReliefShape, size: f64, alpha: f64, ba: f64) -> f64 {
        let r = size / 2.0;
        match shape {
            ReliefShape::Notch | ReliefShape::Tear => 0.0,
            ReliefShape::Round => r * r * alpha / 2.0 + 2.0 * r * ba,
            ReliefShape::Rectangular => r * r * (alpha / 2.0).tan() + 2.0 * r * ba,
        }
    }

    #[test]
    fn the_rule_names_its_corner_relief_shape() {
        // A rule saved before the shape existed reads as the notch it always had.
        let old: SheetRule = serde_json::from_str(r#"{"name":"Old","thickness":"2 mm","k_factor":"0.5","bend_radius":"1 mm"}"#).unwrap();
        assert_eq!(old.corner_relief_shape, "notch");
        assert_eq!(SheetRule::steel().corner_relief_shape, "notch");
        let (doc, vals) = (Document::default(), std::collections::BTreeMap::new());
        assert_eq!(doc.rule_values(&vals, &old).unwrap().corner_relief_shape, ReliefShape::Notch);
        let shapes =
            [("round", ReliefShape::Round), (" Rectangular ", ReliefShape::Rectangular), ("NOTCH", ReliefShape::Notch), ("Tear", ReliefShape::Tear)];
        for (text, shape) in shapes {
            let rule = SheetRule { corner_relief_shape: text.into(), ..old.clone() };
            assert_eq!(doc.rule_values(&vals, &rule).unwrap().corner_relief_shape, shape);
            assert_eq!(ReliefShape::parse(shape.name()), Some(shape));
        }
        // No other name is a shape.
        for text in ["square", "circle", "rip", ""] {
            let err = doc.rule_values(&vals, &SheetRule { corner_relief_shape: text.into(), ..old.clone() }).unwrap_err().to_string();
            assert!(err.contains(&format!("unknown corner relief shape `{text}` (notch, round, rectangular or tear)")), "{err}");
        }
    }

    #[test]
    fn corner_reliefs_cut_the_base_corner_and_stop_the_bends() {
        use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};
        // The tray: the bends on x = 0 and y = 0 meet at c = (s0, s0), s0 = (R + T)·tan(θ/2) in from
        // the sheet's corner. A relief 8 mm across reaches 4 mm from c along both edges of the base.
        for deg in [72.0f64, 90.0] {
            let th = deg.to_radians();
            let (s0, ba) = (3.0 * (th / 2.0).tan(), th * 2.0);
            let len = 120.0 - 2.0 * s0;
            let c = Vec2::new(s0, s0);
            let (notch, _) = tray(&alu(), deg, 38.6);
            assert_eq!(notch.base.outer.segs.len(), 4, "{deg}°: the notch cuts nothing");
            for shape in [ReliefShape::Round, ReliefShape::Rectangular] {
                let what = format!("{shape:?} at {deg}°");
                let (s, notes) = tray(&RuleValues { corner_relief_shape: shape, ..alu() }, deg, 38.6);
                assert!(notes.is_empty(), "{what}: {notes:?}");
                let segs = &s.base.outer.segs;
                if shape == ReliefShape::Round {
                    // A quarter round of radius 4 about c, from the edge x = s0 round to the edge y = s0.
                    assert_eq!((segs.len(), segs.iter().filter(|g| matches!(g, Seg2::Arc { .. })).count()), (8, 4), "{what}");
                    let found = segs.iter().any(|g| {
                        matches!(g, Seg2::Arc { center, radius, .. } if center.dist(c) < 1e-8 && (radius - 4.0).abs() < 1e-12)
                            && g.start().dist(c + Vec2::new(0.0, 4.0)) < 1e-9
                            && g.point_at(0.5).dist(c + Vec2::from_angle(FRAC_PI_4) * 4.0) < 1e-9
                            && g.end().dist(c + Vec2::new(4.0, 0.0)) < 1e-9
                    });
                    assert!(found, "{what}: {segs:?}");
                } else {
                    // A 4 mm square: in from the edge x = s0 to (s0 + 4, s0 + 4), out to the edge y = s0.
                    let line =
                        |a: Vec2, b: Vec2| segs.iter().any(|g| matches!(g, Seg2::Line { .. }) && g.start().dist(a) < 1e-9 && g.end().dist(b) < 1e-9);
                    assert_eq!(segs.len(), 12, "{what}");
                    assert!(
                        line(c + Vec2::new(0.0, 4.0), c + Vec2::new(4.0, 4.0)) && line(c + Vec2::new(4.0, 4.0), c + Vec2::new(4.0, 0.0)),
                        "{what}: {segs:?}"
                    );
                }
                // The bend on y = 0 stops 4 mm from the corners at both its ends, and the bend table follows.
                let (u0, u1) = s.zone_span(&s.flanges[0]);
                assert!((u0 - 4.0).abs() < 1e-9 && (u1 - (len - 4.0)).abs() < 1e-9, "{what}: {u0} {u1}");
                let bend = &s.bends()[0];
                let (start, end) = (Vec2::new(s0 + 4.0, s0 - ba / 2.0), Vec2::new(120.0 - s0 - 4.0, s0 - ba / 2.0));
                assert!((bend.length - (len - 8.0)).abs() < 1e-9 && bend.start.dist(start) < 1e-9 && bend.end.dist(end) < 1e-9, "{what}: {bend:?}");
                // Only the base changes: the flanges are the ones the closing left.
                assert_eq!(s.flanges, notch.flanges, "{what}");
                // The flat pattern loses, at each of its four corners, the relief in the base and 4 mm of two bends.
                let (removed, want) = (flat_area(&notch) - flat_area(&s), 4.0 * relief_area(shape, 8.0, FRAC_PI_2, ba));
                assert!((removed - want).abs() < 1e-6, "{what}: {removed} vs {want}");
                holds_the_flat_material(&s, &what);
                let unfolded = vol(&s.flat_solid().unwrap());
                assert!((unfolded - flat_area(&s) * 2.0).abs() < 1e-3 * unfolded, "{what}: {unfolded}");
            }
        }
    }

    /// The trapezoid of the closing tests with four flanges at `deg`, under rule `rv`.
    fn trapezoid(rv: &RuleValues, deg: f64) -> (SheetBody, Vec<String>) {
        let pts = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(100.0, 90.0), Vec2::new(20.0, 90.0)];
        let mut s = base_flange("Body1", "AL2", rv, Plane::XY, Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] });
        let picks = [Vec3::new(60.0, 0.0, 2.0), Vec3::new(110.0, 45.0, 2.0), Vec3::new(60.0, 90.0, 2.0), Vec3::new(10.0, 45.0, 2.0)];
        let (_, notes) = s.add_flanges_noted(&picks, 38.6, deg.to_radians(), rv.bend_radius, "inside", false).unwrap();
        (s, notes)
    }

    #[test]
    fn corner_reliefs_follow_corners_that_are_not_square() {
        use std::f64::consts::PI;
        // The base fills α = atan(90/20) = 77.47° at the two lower corners of the trapezoid and 180° − α
        // at the upper ones. With every edge trimmed by s0, the bends on y = 0 and on the left edge meet
        // at c = (s0/tan(α/2), s0), and the left edge leaves c in the direction (cos α, sin α).
        let th = 72f64.to_radians();
        let (s0, ba) = (3.0 * (th / 2.0).tan(), th * 2.0);
        let alpha = 90f64.atan2(20.0);
        let c = Vec2::new(s0 / (alpha / 2.0).tan(), s0);
        let (along, up) = (Vec2::new(1.0, 0.0), Vec2::from_angle(alpha));
        let (notch, _) = trapezoid(&alu(), 72.0);
        for shape in [ReliefShape::Round, ReliefShape::Rectangular] {
            let what = format!("{shape:?}");
            let (s, notes) = trapezoid(&RuleValues { corner_relief_shape: shape, ..alu() }, 72.0);
            assert!(notes.is_empty(), "{what}: {notes:?}");
            let segs = &s.base.outer.segs;
            if shape == ReliefShape::Round {
                // An arc of radius 4 about c through the angle α, tangent to both bends' straight ends.
                let found = segs.iter().any(|g| {
                    matches!(g, Seg2::Arc { center, radius, sweep, .. } if center.dist(c) < 1e-8 && (radius - 4.0).abs() < 1e-12 && (sweep + alpha).abs() < 1e-9)
                        && g.start().dist(c + up * 4.0) < 1e-8
                        && g.end().dist(c + along * 4.0) < 1e-8
                        && g.start_tangent().dot(up).abs() < 1e-9
                        && g.end_tangent().dot(along).abs() < 1e-9
                });
                assert!(found, "{what}: {segs:?}");
            } else {
                // A kite: both cuts square to their edge, meeting 4/cos(α/2) from c on the bisector.
                let x = c + Vec2::from_angle(alpha / 2.0) * (4.0 / (alpha / 2.0).cos());
                let line =
                    |a: Vec2, b: Vec2| segs.iter().any(|g| matches!(g, Seg2::Line { .. }) && g.start().dist(a) < 1e-8 && g.end().dist(b) < 1e-8);
                assert!(line(c + up * 4.0, x) && line(x, c + along * 4.0), "{what}: {segs:?}");
                assert!((x - (c + up * 4.0)).dot(up).abs() < 1e-9 && (x - (c + along * 4.0)).dot(along).abs() < 1e-9);
            }
            // Two corners of α and two of 180° − α, eight bend ends.
            let want = 2.0 * relief_area(shape, 8.0, alpha, ba) + 2.0 * relief_area(shape, 8.0, PI - alpha, ba);
            let removed = flat_area(&notch) - flat_area(&s);
            assert!((removed - want).abs() < 1e-6, "{what}: {removed} vs {want}");
            assert_eq!(s.flanges, notch.flanges, "{what}");
            holds_the_flat_material(&s, &what);
        }
    }

    #[test]
    fn corner_reliefs_hold_at_every_angle_and_size() {
        use std::f64::consts::{FRAC_PI_2, PI};
        // The round relief runs into the bends' straight ends tangentially, a contact the planar
        // booleans have to keep as one point. So every 5° from 45° to 135° is built on the tray and on
        // the trapezoid with both shapes, and on the tray reliefs from 0.5 to 20 mm across.
        let alpha = 90f64.atan2(20.0);
        for shape in [ReliefShape::Round, ReliefShape::Rectangular] {
            for k in 9..=27 {
                let deg = k as f64 * 5.0;
                let ba = deg.to_radians() * 2.0;
                let rv = RuleValues { corner_relief_shape: shape, ..alu() };
                for (name, (s, notes), (notch, _), corners) in [
                    ("tray", tray(&rv, deg, 38.6), tray(&alu(), deg, 38.6), [FRAC_PI_2; 4]),
                    ("trapezoid", trapezoid(&rv, deg), trapezoid(&alu(), deg), [alpha, alpha, PI - alpha, PI - alpha]),
                ] {
                    let what = format!("{shape:?} on the {name} at {deg}°");
                    assert!(notes.is_empty(), "{what}: {notes:?}");
                    let want: f64 = corners.iter().map(|a| relief_area(shape, 8.0, *a, ba)).sum();
                    let removed = flat_area(&notch) - flat_area(&s);
                    assert!((removed - want).abs() < 1e-6, "{what}: {removed} vs {want}");
                    holds_the_flat_material(&s, &what);
                }
            }
            for size in [0.5, 1.0, 3.0, 5.0, 12.0, 20.0] {
                for deg in [72.0f64, 90.0] {
                    let what = format!("{shape:?} {size} mm across at {deg}°");
                    let (s, notes) = tray(&RuleValues { corner_relief_shape: shape, corner_relief: size, ..alu() }, deg, 38.6);
                    assert!(notes.is_empty(), "{what}: {notes:?}");
                    let want = 4.0 * relief_area(shape, size, FRAC_PI_2, deg.to_radians() * 2.0);
                    let removed = flat_area(&tray(&alu(), deg, 38.6).0) - flat_area(&s);
                    assert!((removed - want).abs() < 1e-6, "{what}: {removed} vs {want}");
                    holds_the_flat_material(&s, &what);
                }
            }
        }
    }

    #[test]
    fn a_relief_that_does_not_fit_is_left_out_with_a_note() {
        let closed = tray(&alu(), 72.0, 38.6).0;
        // 200 mm across reaches 100 mm along edges of which the shorter is 90 − 2·s0 = 85.6 mm long.
        // 100 mm across reaches 50 mm: that fits the edge once but not from both its ends, and all four
        // corners are left out together. Nothing, or less than nothing, is too small to cut.
        let sizes = [(200.0, "whole bend"), (100.0, "whole bend"), (0.0, "below 0.01 mm"), (-8.0, "below 0.01 mm"), (f64::NAN, "below 0.01 mm")];
        for (size, why) in sizes {
            let (s, notes) = tray(&RuleValues { corner_relief_shape: ReliefShape::Round, corner_relief: size, ..alu() }, 72.0, 38.6);
            assert_eq!(notes.len(), 4, "{size}: {notes:?}");
            assert!(notes.iter().all(|n| n.starts_with("no corner relief at (") && n.contains(why)), "{size}: {notes:?}");
            // The part is the one the notch gives, corners closed.
            assert_eq!((&s.base, &s.flanges), (&closed.base, &closed.flanges), "{size}");
        }
        // A 120 × 12 strip with flanges on its lower and left edges, a relief 16 mm across. Its 8 mm fit
        // on both edges at the corner (the left one is 12 − s0 = 9.82 long), but the far corner of the
        // square is 8·√2 = 11.3 mm from the corner of the bends, and the strip's upper edge is only 9.82
        // from it: no relief may reach as far from its corner as another edge is. The round one reaches
        // 8 mm and fits.
        let strip = |shape: ReliefShape| {
            let base = rect(Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 12.0), Vec2::new(0.0, 12.0));
            let mut s = base_flange("Body1", "AL2", &RuleValues { corner_relief_shape: shape, corner_relief: 16.0, ..alu() }, Plane::XY, base);
            let picks = [Vec3::new(60.0, 0.0, 2.0), Vec3::new(0.0, 6.0, 2.0)];
            let (_, notes) = s.add_flanges_noted(&picks, 20.0, 72f64.to_radians(), 1.0, "inside", false).unwrap();
            (s, notes)
        };
        let (s, notes) = strip(ReliefShape::Rectangular);
        assert!(notes.len() == 1 && notes[0].contains("another edge of the base"), "{notes:?}");
        assert!(s.base.outer.segs.len() == 4 && s.solid().is_ok());
        let (s, notes) = strip(ReliefShape::Round);
        assert!(notes.is_empty() && s.base.outer.segs.iter().filter(|g| matches!(g, Seg2::Arc { .. })).count() == 1, "{notes:?}");
        holds_the_flat_material(&s, "the strip");
        // A right triangle with flanges on its two legs: the base's legs are 100 − 2·s0 = 95.6 long and
        // its long edge passes (100 − 2·s0)/√2 = 67.6 mm from the corner of the bends. A round relief of
        // radius 75 would cut through that edge; one of radius 65 stays inside.
        let triangle = |size: f64| {
            let pts = [Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(0.0, 100.0)];
            let rv = RuleValues { corner_relief_shape: ReliefShape::Round, corner_relief: size, ..alu() };
            let mut s = base_flange("Body1", "AL2", &rv, Plane::XY, Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] });
            let picks = [Vec3::new(50.0, 0.0, 2.0), Vec3::new(0.0, 50.0, 2.0)];
            let (_, notes) = s.add_flanges_noted(&picks, 20.0, 72f64.to_radians(), 1.0, "inside", false).unwrap();
            (s, notes)
        };
        let (s, notes) = triangle(150.0);
        assert!(notes.len() == 1 && notes[0].contains("another edge of the base"), "{notes:?}");
        assert!(s.base.outer.segs.len() == 3 && s.solid().is_ok());
        let (s, notes) = triangle(130.0);
        assert!(notes.is_empty() && s.base.outer.segs.len() == 4, "{notes:?}");
        holds_the_flat_material(&s, "the triangle");
    }

    #[test]
    fn a_corner_with_a_flange_of_an_earlier_feature_gets_no_relief() {
        // Like the closing, the relief belongs to the corners between the flanges of one feature.
        let rv = RuleValues { corner_relief_shape: ReliefShape::Round, ..alu() };
        let base = rect(Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0));
        let mut s = base_flange("Body1", "AL2", &rv, Plane::XY, base);
        s.add_flanges(&[Vec3::new(60.0, 0.0, 2.0)], 38.6, 72f64.to_radians(), 1.0, "inside", false).unwrap();
        let (_, notes) = s.add_flanges_noted(&[Vec3::new(120.0, 45.0, 2.0)], 25.0, 72f64.to_radians(), 1.0, "inside", false).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert!(s.base.outer.segs.iter().all(|g| matches!(g, Seg2::Line { .. })));
        assert!(s.solid().is_ok() && s.flat_regions().unwrap().len() == 1);
    }

    #[test]
    fn closed_corners_keep_their_flat_pattern_at_every_angle() {
        // 45° to 135° in steps of 5° on the tray and on a trapezoid. Which angles lose the flat
        // pattern to a rounding slip in the planar arrangement is a matter of luck (50°, 60°, 65°,
        // 75°, 105° and 110° did on the tray), so every one is built.
        let tray = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0)];
        let tray_picks = [Vec3::new(60.0, 0.0, 2.0), Vec3::new(120.0, 45.0, 2.0), Vec3::new(60.0, 90.0, 2.0), Vec3::new(0.0, 45.0, 2.0)];
        let trapezoid = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(100.0, 90.0), Vec2::new(20.0, 90.0)];
        let trapezoid_picks = [Vec3::new(60.0, 0.0, 2.0), Vec3::new(110.0, 45.0, 2.0), Vec3::new(60.0, 90.0, 2.0), Vec3::new(10.0, 45.0, 2.0)];
        for (name, pts, picks) in [("tray", tray, tray_picks), ("trapezoid", trapezoid, trapezoid_picks)] {
            for k in 9..=27 {
                let deg = k as f64 * 5.0;
                let what = format!("{name} at {deg}°");
                let base = Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] };
                let mut s = base_flange("Body1", "AL2", &alu(), Plane::XY, base);
                let (made, notes) = s.add_flanges_noted(&picks, 38.6, deg.to_radians(), 1.0, "inside", false).unwrap();
                assert!(notes.is_empty(), "{what}: {notes:?}");
                let corners = s.corners(&made);
                assert_eq!(corners.len(), 4, "{what}");
                ends_are_gap_apart(&s, &corners, 0.5, &what);
                holds_the_flat_material(&s, &what);
            }
        }
    }

    /// A 160 x 30 bar with a leg 20 wide standing `leg` past its top edge at the left end: the
    /// top edge (x 20…160) ends at an inside corner of the base.
    fn bar_with_leg(rv: &RuleValues, leg: f64) -> SheetBody {
        let pts = [(0.0, 0.0), (160.0, 0.0), (160.0, 30.0), (20.0, 30.0), (20.0, 30.0 + leg), (0.0, 30.0 + leg)].map(|(x, y)| Vec2::new(x, y));
        base_flange("Body1", "AL2", rv, Plane::XY, Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] })
    }

    /// The base's outline has these corners, and their polygon's area.
    fn base_is(s: &SheetBody, want: &[(f64, f64)], what: &str) {
        let want: Vec<Vec2> = want.iter().map(|&(x, y)| Vec2::new(x, y)).collect();
        let got = corners_of(&s.base);
        for p in &want {
            assert!(got.iter().any(|q| q.dist(*p) < 1e-9), "{what}: {p:?} not in {got:?}");
        }
        let area = signed_area(&want).abs();
        assert!((s.base.area() - area).abs() < 1e-9, "{what}: {} vs {area}", s.base.area());
    }

    #[test]
    fn a_flange_beside_a_leg_of_the_base_trims_its_own_edge_and_gets_a_bend_relief() {
        // A 90° flange 15 high on the bar's top edge: setback s0 = (R + T)·tan 45° = 3 inside,
        // half of it in the middle, none outside; bend allowance BA = (π/2)·(R + K·T) = π; panel
        // 15 − s0 = 12. Only the strip under that edge goes, so the bend runs from x = 160 to the
        // leg at x = 20 and the leg stays. Beside the bend's end the rule's relief (2 wide, 1 deep)
        // is cut into the leg: x 18…20, from 1 before the bend line to 2 past the flange's far
        // edge in the flat (BA + 12 past the line), or to the leg's end where that comes first.
        let rv = alu();
        let (ba, panel) = (std::f64::consts::PI, 12.0);
        for leg in [30.0, 10.0] {
            for (position, line) in [("inside", 27.0), ("middle", 28.5), ("outside", 30.0)] {
                let what = format!("{position}, leg {leg}");
                let mut s = bar_with_leg(&rv, leg);
                let (made, notes) =
                    s.add_flanges_noted(&[Vec3::new(90.0, 30.0, 2.0)], 15.0, std::f64::consts::FRAC_PI_2, 1.0, position, false).unwrap();
                assert!(notes.is_empty(), "{what}: {notes:?}");
                let (top, end) = (30.0 + leg, line + ba + panel + 2.0);
                let mut want = vec![(0.0, 0.0), (160.0, 0.0), (160.0, line), (20.0, line), (20.0, line - 1.0), (18.0, line - 1.0)];
                if end < top {
                    want.extend([(18.0, end), (20.0, end), (20.0, top), (0.0, top)]);
                } else {
                    want.extend([(18.0, top), (0.0, top)]);
                }
                base_is(&s, &want, &what);
                let f = &s.flanges[made[0]];
                assert!(f.p.dist(Vec2::new(160.0, line)) < 1e-9 && (f.len - 140.0).abs() < 1e-9, "{what}: {f:?}");
                assert!((s.bends()[0].length - 140.0).abs() < 1e-9, "{what}: {:?}", s.bends());
                // The flat pattern is the base and 140 × (BA + 12) of flange in one piece; folded,
                // the part is one valid shell holding the same material.
                let base = 160.0 * line + 20.0 * (top - line) - 2.0 * (end.min(top) - (line - 1.0));
                assert!((flat_area(&s) - (base + 140.0 * (ba + panel))).abs() < 1e-6, "{what}: {}", flat_area(&s));
                holds_the_flat_material(&s, &what);
            }
        }
    }

    #[test]
    fn flanges_on_both_sides_of_a_leg_each_trim_their_own_edge() {
        // A 160 x 30 bar with a 20 x 10 leg in the middle of its top edge, and flanges on the two
        // parts of that edge (x 0…70 and 90…160) in one call: each takes the strip under its own
        // part, and the leg gets a relief on either side (x 70…72 and 88…90) up to its end.
        let pts = [(0.0, 0.0), (160.0, 0.0), (160.0, 30.0), (90.0, 30.0), (90.0, 40.0), (70.0, 40.0), (70.0, 30.0), (0.0, 30.0)];
        let outer = Loop2::polygon(&pts.map(|(x, y)| Vec2::new(x, y))).ccw();
        let mut s = base_flange("Body1", "AL2", &alu(), Plane::XY, Region2 { outer, holes: vec![] });
        let picks = [Vec3::new(125.0, 30.0, 2.0), Vec3::new(35.0, 30.0, 2.0)];
        let (made, notes) = s.add_flanges_noted(&picks, 15.0, std::f64::consts::FRAC_PI_2, 1.0, "inside", false).unwrap();
        assert!(notes.is_empty() && s.corners(&made).is_empty(), "{notes:?}");
        let right = [(0.0, 0.0), (160.0, 0.0), (160.0, 27.0), (90.0, 27.0), (90.0, 26.0), (88.0, 26.0), (88.0, 40.0)];
        let left = [(72.0, 40.0), (72.0, 26.0), (70.0, 26.0), (70.0, 27.0), (0.0, 27.0)];
        base_is(&s, &[&right[..], &left[..]].concat(), "tee");
        assert!(s.flanges.iter().all(|f| (f.len - 70.0).abs() < 1e-9), "{:?}", s.flanges);
        holds_the_flat_material(&s, "tee");
    }

    #[test]
    fn a_flange_on_the_side_of_a_leg_leaves_the_base_the_leg_stands_on() {
        // The leg's right side (x = 20, y 30…60): past that edge's line the whole bar runs on. The
        // setback takes x 17…20 of the leg alone, and the relief goes into the bar beside the
        // bend's end at y = 30: y 28…30, from x = 16 (1 before the bend line) to 2 past the
        // flange's far edge in the flat at x = 17 + BA + 12.
        let mut s = bar_with_leg(&alu(), 30.0);
        let (made, notes) = s.add_flanges_noted(&[Vec3::new(20.0, 45.0, 2.0)], 15.0, std::f64::consts::FRAC_PI_2, 1.0, "inside", false).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let end = 17.0 + std::f64::consts::PI + 12.0 + 2.0;
        let want =
            [(0.0, 0.0), (160.0, 0.0), (160.0, 30.0), (end, 30.0), (end, 28.0), (16.0, 28.0), (16.0, 30.0), (17.0, 30.0), (17.0, 60.0), (0.0, 60.0)];
        base_is(&s, &want, "side");
        let f = &s.flanges[made[0]];
        assert!(f.p.dist(Vec2::new(17.0, 30.0)) < 1e-9 && (f.len - 30.0).abs() < 1e-9, "{f:?}");
        holds_the_flat_material(&s, "side");
    }

    /// A 100 x 60 plate with a ledge 3 wide and `h` tall beside its y = 0 edge, at its right end
    /// or at both ends, turned `turn` about the origin.
    fn ledged(h: f64, both: bool, turn: f64) -> SheetBody {
        let mut pts = vec![(0.0, 0.0), (100.0, 0.0), (100.0, h), (97.0, h), (97.0, 60.0)];
        pts.extend(if both { vec![(3.0, 60.0), (3.0, h), (0.0, h)] } else { vec![(0.0, 60.0)] });
        let pts: Vec<Vec2> = pts.into_iter().map(|(x, y)| turned(x, y, turn)).collect();
        base_flange("Body1", "AL2", &alu(), Plane::XY, Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] })
    }

    fn turned(x: f64, y: f64, turn: f64) -> Vec2 {
        Vec2::new(x * turn.cos() - y * turn.sin(), x * turn.sin() + y * turn.cos())
    }

    #[test]
    fn a_ledge_beside_an_edge_as_tall_as_the_setback_goes_with_the_strip() {
        // A 100 x 60 plate with a ledge 3 wide beside its y = 0 edge, as tall as the setback of a
        // 90° flange 15 high on that edge: (R + T)·tan 45° = 3 inside, half of it in the middle;
        // BA = (π/2)·(R + K·T) = π, panel 15 − 3 = 12. The setback takes the strip under the edge
        // and the ledge with it, as it takes a lower one: the base ends on the bend line, 97 wide
        // (94 with a ledge at either end), and the bend and the flange run along it. The trim
        // used to leave the bend line running out to the ledge's corner and back, and the part
        // failed to sew ("a degenerate edge"); so did a ledge within the model's tolerance of the
        // setback. Square to the axes and turned (a 5-12-13 slope) alike.
        let (ba, panel, quarter) = (std::f64::consts::PI, 12.0, std::f64::consts::FRAC_PI_2);
        for (position, line) in [("inside", 3.0), ("middle", 3.0 / 2.0)] {
            for both in [false, true] {
                for turn in [0.0, (5.0f64 / 13.0).asin()] {
                    // Exact square to the axes; to within the crossings' 1e-9 grid when turned.
                    let tol = if turn == 0.0 { 1e-9 } else { 1e-6 };
                    for hair in [0.0, 1e-7, -1e-7] {
                        let what = format!("{position}, ledge {line}{hair:+e}, both ends {both}, turned {turn:.4}");
                        let mut s = ledged(line + hair, both, turn);
                        let pick = turned(50.0, 0.0, turn);
                        let (made, notes) = s
                            .add_flanges_noted(&[Vec3::new(pick.x, pick.y, 2.0)], 15.0, quarter, 1.0, position, false)
                            .unwrap_or_else(|e| panic!("{what}: {e}"));
                        assert!(notes.is_empty(), "{what}: {notes:?}");
                        let x0 = if both { 3.0 } else { 0.0 };
                        let got = corners_of(&s.base);
                        let want = [(x0, line), (97.0, line), (97.0, 60.0), (x0, 60.0)].map(|(x, y)| turned(x, y, turn));
                        assert!(got.len() == 4 && want.iter().all(|p| got.iter().any(|q| q.dist(*p) < tol)), "{what}: {got:?}");
                        let base = (97.0 - x0) * (60.0 - line);
                        assert!((s.base.area() - base).abs() < tol, "{what}: {} vs {base}", s.base.area());
                        let f = &s.flanges[made[0]];
                        assert!(f.p.dist(turned(x0, line, turn)) < tol && (f.len - (97.0 - x0)).abs() < tol, "{what}: {f:?}");
                        assert!((s.bends()[0].length - (97.0 - x0)).abs() < tol, "{what}: {:?}", s.bends());
                        assert!((flat_area(&s) - (base + (97.0 - x0) * (ba + panel))).abs() < 1e-6, "{what}: {}", flat_area(&s));
                        holds_the_flat_material(&s, &what);
                    }
                    // A ledge clearly shorter goes the same way. One clearly taller keeps a sliver
                    // above the bend line, and the bend runs on under it, the edge's whole 100;
                    // turned, its bend used to end a fraction of a nanometre along the sliver's
                    // side, which split that side there and failed to sew in the middle.
                    for hair in [1e-4, 0.05, -1e-4, -0.05] {
                        let what = format!("{position}, ledge {line}{hair:+e}, both ends {both}, turned {turn:.4}");
                        let mut s = ledged(line + hair, both, turn);
                        let pick = turned(50.0, 0.0, turn);
                        let (made, notes) = s
                            .add_flanges_noted(&[Vec3::new(pick.x, pick.y, 2.0)], 15.0, quarter, 1.0, position, false)
                            .unwrap_or_else(|e| panic!("{what}: {e}"));
                        assert!(notes.is_empty(), "{what}: {notes:?}");
                        let (x0, ends) = (if both { 3.0 } else { 0.0 }, if both { 2.0 } else { 1.0 });
                        let (from, len, sliver) = if hair > 0.0 { (0.0, 100.0, ends * 3.0 * hair) } else { (x0, 97.0 - x0, 0.0) };
                        let f = &s.flanges[made[0]];
                        assert!(f.p.dist(turned(from, line, turn)) < tol && (f.len - len).abs() < tol, "{what}: {f:?}");
                        let base = (97.0 - x0) * (60.0 - line) + sliver;
                        assert!((s.base.area() - base).abs() < tol, "{what}: {} vs {base}", s.base.area());
                        assert!((flat_area(&s) - (base + len * (ba + panel))).abs() < 1e-6, "{what}: {}", flat_area(&s));
                        // (A sliver 0.0001 wide is below the flat pattern's shortest edge.)
                        if !(0.0..=1e-3).contains(&hair) {
                            holds_the_flat_material(&s, &what);
                        } else {
                            let b = s.solid().unwrap_or_else(|e| panic!("{what}: no solid: {e}"));
                            assert!(b.validity().is_empty() && b.shell_count() == 1, "{what}: {:?}", b.validity());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_bend_relief_at_one_end_leaves_the_corner_at_the_other_as_it_was() {
        // Flanges on the bar's top and right edges in one call: their bends meet at (157, 27),
        // where the corner is closed and relieved or torn by the rule's shape, and the top one
        // ends beside the leg at (20, 27), where it gets the bend relief. Against the bar without
        // the leg, the same corner and all, the flat pattern gains the leg from the bend line
        // up, 20 × (60 − 27), less the relief, 2 × (1 + BA + 12 + 2), and loses 20 of the top
        // flange's length, 20 × (BA + 12).
        let (ba, panel) = (std::f64::consts::PI, 12.0);
        let picks = [Vec3::new(90.0, 30.0, 2.0), Vec3::new(160.0, 15.0, 2.0)];
        for shape in [ReliefShape::Notch, ReliefShape::Round, ReliefShape::Rectangular, ReliefShape::Tear] {
            let what = format!("{shape:?}");
            let rv = RuleValues { corner_relief_shape: shape, ..alu() };
            let bar = rect(Vec2::new(0.0, 0.0), Vec2::new(160.0, 0.0), Vec2::new(160.0, 30.0), Vec2::new(0.0, 30.0));
            let mut plain = base_flange("Body1", "AL2", &rv, Plane::XY, bar);
            let (_, notes) = plain.add_flanges_noted(&picks, 15.0, std::f64::consts::FRAC_PI_2, 1.0, "inside", false).unwrap();
            assert!(notes.is_empty(), "{what}, no leg: {notes:?}");
            let mut s = bar_with_leg(&rv, 30.0);
            let (made, notes) = s.add_flanges_noted(&picks, 15.0, std::f64::consts::FRAC_PI_2, 1.0, "inside", false).unwrap();
            assert!(notes.is_empty() && made.len() == 2, "{what}: {notes:?}");
            // The corner's relief reaches 4 along both bends (8 across); the notch none. A tear
            // moves the bend's end at the corner out by grow = sin 90° − 0.5/√2 on the flange's
            // side, so its centre line is half of that longer.
            let cut = match shape {
                ReliefShape::Notch => 0.0,
                ReliefShape::Tear => -(1.0 - 0.5 / std::f64::consts::SQRT_2) / 2.0,
                ReliefShape::Round | ReliefShape::Rectangular => 4.0,
            };
            let lengths: Vec<f64> = s.bends().iter().map(|b| b.length).collect();
            assert!((lengths[0] - (137.0 - cut)).abs() < 1e-9 && (lengths[1] - (27.0 - cut)).abs() < 1e-9, "{what}: {lengths:?}");
            let got = corners_of(&s.base);
            let end = 27.0 + ba + panel + 2.0;
            for p in [(20.0, 27.0), (20.0, 26.0), (18.0, 26.0), (18.0, end), (20.0, end), (20.0, 60.0), (0.0, 60.0)] {
                assert!(got.iter().any(|q| q.dist(Vec2::new(p.0, p.1)) < 1e-9), "{what}: {p:?} not in {got:?}");
            }
            let want = flat_area(&plain) + 20.0 * 33.0 - 2.0 * (ba + panel + 3.0) - 20.0 * (ba + panel);
            assert!((flat_area(&s) - want).abs() < 1e-6, "{what}: {} vs {want}", flat_area(&s));
            holds_the_flat_material(&s, &what);
        }
    }

    #[test]
    fn a_bend_relief_that_cannot_be_cut_is_left_out_with_a_note() {
        let leg = [(20.0, 60.0), (0.0, 60.0)];
        // The same leg held by a neck 2 wide: a notch from the left (x 0…18, y 43…45) under its end.
        let neck = [(20.0, 60.0), (0.0, 60.0), (0.0, 45.0), (18.0, 45.0), (18.0, 43.0), (0.0, 43.0)];
        // No width to cut; a relief wider than the leg, which would take the leg away; and a
        // relief through the neck, which would cut the leg's end off.
        for (width, top, why) in [(0.0, &leg[..], "relief width"), (25.0, &leg[..], "beside the bend"), (2.0, &neck[..], "one piece")] {
            let rv = RuleValues { relief_width: width, ..alu() };
            let outline = |line: f64| [&[(0.0, 0.0), (160.0, 0.0), (160.0, line), (20.0, line)][..], top].concat();
            let pts: Vec<Vec2> = outline(30.0).iter().map(|&(x, y)| Vec2::new(x, y)).collect();
            let mut s = base_flange("Body1", "AL2", &rv, Plane::XY, Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] });
            let (_, notes) = s.add_flanges_noted(&[Vec3::new(90.0, 30.0, 2.0)], 15.0, std::f64::consts::FRAC_PI_2, 1.0, "inside", false).unwrap();
            assert!(
                notes.len() == 1 && notes[0].starts_with("no bend relief at (20.000, 27.000, 0.000)") && notes[0].contains(why),
                "{why}: {notes:?}"
            );
            // The flange still took its own edge only.
            base_is(&s, &outline(27.0), why);
        }
    }

    /// A 200 x 100 plate (2 mm, R 1, K 0.5) with these cut-outs through it.
    fn plate_with_cuts(cuts: &[Region2]) -> SheetBody {
        let outline = rect(Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0), Vec2::new(200.0, 100.0), Vec2::new(0.0, 100.0));
        let mut s = base_flange("Body1", "AL2", &alu(), Plane::XY, outline);
        s.holes.extend(cuts.iter().cloned());
        s
    }

    fn block(x0: f64, x1: f64, y0: f64, y1: f64) -> Region2 {
        rect(Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1))
    }

    #[test]
    fn an_edge_flange_runs_along_what_a_cut_out_leaves_of_its_edge() {
        // A cut has taken the plate's bottom left 100 x 60, so its y = 0 edge is left from x = 100
        // to 200. A 90° flange 40 high picked there (setback 3 inside, half of it in the middle,
        // none outside; BA = π; panel 37) spans that stretch and takes its setback off it alone.
        // It used to span x 0…200, half of it a strip standing beside nothing.
        let (ba, panel, quarter) = (std::f64::consts::PI, 37.0, std::f64::consts::FRAC_PI_2);
        let corner = [block(-10.0, 100.0, -10.0, 60.0)];
        for (position, line) in [("inside", 3.0), ("middle", 3.0 / 2.0), ("outside", 0.0)] {
            let mut s = plate_with_cuts(&corner);
            let (made, notes) = s.add_flanges_noted(&[Vec3::new(150.0, 0.0, 2.0)], 40.0, quarter, 1.0, position, false).unwrap();
            assert!(notes.is_empty(), "{position}: {notes:?}");
            let f = s.flanges[made[0]].clone();
            assert!(f.p.dist(Vec2::new(100.0, line)) < 1e-9 && (f.len - 100.0).abs() < 1e-9 && f.d.dist(Vec2::X) < 1e-12, "{position}: {f:?}");
            let bends = s.bends();
            assert!(bends.len() == 1 && (bends[0].length - 100.0).abs() < 1e-9 && (bends[0].start.x - 100.0).abs() < 1e-9, "{position}: {bends:?}");
            // The base keeps its edge where the cut was: the step at x = 100 lies inside the cut-out.
            let step = [(0.0, 0.0), (100.0, 0.0), (100.0, line), (200.0, line), (200.0, 100.0), (0.0, 100.0)];
            let whole = [(0.0, 0.0), (200.0, 0.0), (200.0, 100.0), (0.0, 100.0)];
            base_is(&s, if line > 0.0 { &step[..] } else { &whole[..] }, position);
            // Flat: the plate less the cut and the setback under the flange, and 100 × (BA + 37).
            assert!((flat_area(&s) - (14_000.0 - 100.0 * line + 100.0 * (ba + panel))).abs() < 1e-6, "{position}: {}", flat_area(&s));
            let regions = s.flat_regions().unwrap();
            let y = line - ba - 20.0;
            assert!(regions[0].contains(Vec2::new(150.0, y)) && !regions[0].contains(Vec2::new(50.0, y)), "{position}");
            holds_the_flat_material(&s, position);
            s.flat = true;
            assert!(s.solid().is_ok_and(|b| b.validity().is_empty()), "{position}: unfolded");
        }
        // A pick on the part the cut took gets the nearest stretch that is left.
        let mut s = plate_with_cuts(&corner);
        let i = s.add_flange(Vec3::new(30.0, 0.0, 2.0), 40.0, quarter, 1.0, "outside", false).unwrap();
        assert!(s.flanges[i].p.dist(Vec2::new(100.0, 0.0)) < 1e-9 && (s.flanges[i].len - 100.0).abs() < 1e-9, "{:?}", s.flanges[i]);
        // Cut-outs that leave the edge alone leave the flange alone: a hole in the plate, and a
        // cut that only touches the edge's line from outside.
        let mut plain = plate_with_cuts(&[]);
        plain.add_flange(Vec3::new(100.0, 0.0, 2.0), 40.0, quarter, 1.0, "inside", false).unwrap();
        let mut s = plate_with_cuts(&[Region2 { outer: Loop2::circle(Vec2::new(100.0, 50.0), 10.0), holes: vec![] }, block(20.0, 60.0, -30.0, 0.0)]);
        s.add_flange(Vec3::new(100.0, 0.0, 2.0), 40.0, quarter, 1.0, "inside", false).unwrap();
        assert_eq!((&s.flanges, &s.base), (&plain.flanges, &plain.base));
        // An edge the cut-outs took whole has nothing to bend, and the sheet stays as it was.
        let mut s = plate_with_cuts(&[block(-10.0, 210.0, -10.0, 30.0)]);
        let before = s.clone();
        let picks = [Vec3::new(200.0, 60.0, 2.0), Vec3::new(100.0, 0.0, 2.0)];
        let e = s.add_flanges(&picks, 40.0, quarter, 1.0, "inside", false).unwrap_err().to_string();
        assert!(e.contains("edge 2: a cut-out has taken that edge away") && s == before, "{e}");
    }

    #[test]
    fn edge_flanges_on_either_side_of_a_notch_each_take_their_own_stretch_of_the_edge() {
        // A notch 40 wide and 30 deep in the middle of the plate's y = 0 edge leaves x 0…80 and
        // 120…200 of it. A flange picked on one side spans that side and takes its setback (3)
        // off it alone, the other side keeping its edge; picked on both sides in one call, each
        // flange has its own stretch. The notch is deeper than the bend line, so it frees the
        // bends' ends and no bend relief is cut.
        let (ba, panel, quarter) = (std::f64::consts::PI, 37.0, std::f64::consts::FRAC_PI_2);
        let left = [(0.0, 3.0), (80.0, 3.0), (80.0, 0.0)];
        let right = [(120.0, 0.0), (120.0, 3.0), (200.0, 3.0)];
        let (plain_left, plain_right, top) = ([(0.0, 0.0)], [(200.0, 0.0)], [(200.0, 100.0), (0.0, 100.0)]);
        // (picks' x, the base's corners along that edge, where each flange starts)
        let cases: [(&[f64], Vec<(f64, f64)>, &[f64]); 4] = [
            (&[40.0], [&left[..], &plain_right[..]].concat(), &[0.0]),
            (&[150.0], [&plain_left[..], &right[..]].concat(), &[120.0]),
            (&[40.0, 150.0], [&left[..], &right[..]].concat(), &[0.0, 120.0]),
            // A pick in the notch gets the side nearer to it.
            (&[110.0], [&plain_left[..], &right[..]].concat(), &[120.0]),
        ];
        for (xs, bottom, starts) in cases {
            let what = format!("picks at x {xs:?}");
            let mut s = plate_with_cuts(&[block(80.0, 120.0, -10.0, 30.0)]);
            let picks: Vec<Vec3> = xs.iter().map(|&x| Vec3::new(x, 0.0, 2.0)).collect();
            let (made, notes) = s.add_flanges_noted(&picks, 40.0, quarter, 1.0, "inside", false).unwrap();
            assert!(notes.is_empty() && s.corners(&made).is_empty(), "{what}: {notes:?}");
            let got: Vec<(f64, f64)> = made.iter().map(|&i| (s.flanges[i].p.x, s.flanges[i].len)).collect();
            assert!(
                got.len() == starts.len() && got.iter().zip(starts).all(|(g, x)| (g.0 - x).abs() < 1e-9 && (g.1 - 80.0).abs() < 1e-9),
                "{what}: {got:?}"
            );
            assert!(made.iter().all(|&i| (s.flanges[i].p.y - 3.0).abs() < 1e-9), "{what}: {:?}", s.flanges);
            base_is(&s, &[&bottom[..], &top[..]].concat(), &what);
            // Flat: the plate less the notch and 80 × 3 of setback per flange, and 80 × (BA + 37)
            // of each flange.
            let n = made.len() as f64;
            assert!((flat_area(&s) - (20_000.0 - 40.0 * 30.0 + n * 80.0 * (ba + panel - 3.0))).abs() < 1e-6, "{what}: {}", flat_area(&s));
            holds_the_flat_material(&s, &what);
        }
    }

    #[test]
    fn an_edge_flange_beside_base_that_a_cut_out_left_gets_its_bend_relief() {
        // A cut 2 deep along x 0…100 of the y = 0 edge is shallower than the flange's setback (3):
        // beside the bend's end at x = 100 it leaves 1 mm of the base before the bend line. The
        // flange picked on x 100…200 takes its setback off that stretch alone, and the rule's
        // relief (2 wide, 1 deep) frees its bend there as it does beside a leg of the base's own
        // outline: x 98…100, up to 1 past the bend line.
        let (ba, panel) = (std::f64::consts::PI, 37.0);
        let mut s = plate_with_cuts(&[block(-10.0, 100.0, -10.0, 2.0)]);
        let (made, notes) = s.add_flanges_noted(&[Vec3::new(150.0, 0.0, 2.0)], 40.0, std::f64::consts::FRAC_PI_2, 1.0, "inside", false).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        base_is(&s, &[(0.0, 0.0), (98.0, 0.0), (98.0, 4.0), (100.0, 4.0), (100.0, 3.0), (200.0, 3.0), (200.0, 100.0), (0.0, 100.0)], "step");
        let f = &s.flanges[made[0]];
        assert!(f.p.dist(Vec2::new(100.0, 3.0)) < 1e-9 && (f.len - 100.0).abs() < 1e-9, "{f:?}");
        // Flat: the plate less the cut (100 × 2), the setback (100 × 3) and the 2 × 2 the relief
        // takes of what the cut left, and 100 × (BA + 37) of flange.
        assert!((flat_area(&s) - (20_000.0 - 200.0 - 300.0 - 4.0 + 100.0 * (ba + panel))).abs() < 1e-6, "{}", flat_area(&s));
        holds_the_flat_material(&s, "step");
    }

    #[test]
    fn a_flange_on_a_flange_runs_along_what_a_cut_out_leaves_of_its_far_edge() {
        // A 40 high flange on the plate's y = 0 edge (outside): its panel's far edge lies at
        // y = −(BA + 37) in the flat. A cut takes x 0…60 of that edge, 12 into the panel, and a
        // 15 high flange picked on the rest of it spans x 60…200. Set inside, it moves the far
        // edge back by its setback (3) end to end: a flange's panel stays a rectangle.
        let (ba, quarter) = (std::f64::consts::PI, std::f64::consts::FRAC_PI_2);
        let far = -(ba + 37.0);
        let mut wall = plate_with_cuts(&[]);
        wall.add_flange(Vec3::new(100.0, 0.0, 2.0), 40.0, quarter, 1.0, "outside", false).unwrap();
        wall.holes.push(block(-10.0, 60.0, far - 5.0, far + 12.0));
        let pick = apply_point(&wall.world(Some(0)), Vec3::new(150.0, far, 2.0));
        for (position, setback) in [("outside", 0.0), ("inside", 3.0)] {
            let mut s = wall.clone();
            let i = s.add_flange(pick, 15.0, quarter, 1.0, position, false).unwrap();
            let f = &s.flanges[i];
            assert!(f.parent == Some(0) && f.p.dist(Vec2::new(60.0, far + setback)) < 1e-9 && (f.len - 140.0).abs() < 1e-9, "{position}: {f:?}");
            // Flat: the plate, 200 × (BA + 37 − setback) of wall less the cut's 60 × (12 − setback),
            // and 140 × (BA + 12) of the flange on it.
            let want = 20_000.0 + 200.0 * (ba + 37.0 - setback) - 60.0 * (12.0 - setback) + 140.0 * (ba + 12.0);
            assert!((flat_area(&s) - want).abs() < 1e-6, "{position}: {} vs {want}", flat_area(&s));
            holds_the_flat_material(&s, position);
        }
    }

    /// A sheet (2 mm, R 1, K 0.5) on an outline of lines, arcs and curves (counter-clockwise).
    fn outlined(segs: Vec<Seg2>) -> SheetBody {
        base_flange("Body1", "AL2", &alu(), Plane::XY, Region2 { outer: Loop2 { segs }, holes: vec![] })
    }

    /// The same sheet on its outline's 0.001 mm polyline: the base a trim used to leave.
    fn chorded(s: &SheetBody) -> SheetBody {
        let mut c = s.clone();
        c.base = Region2 { outer: Loop2::polygon(&s.base.outer.polyline(1e-3)), holes: vec![] };
        c
    }

    fn straight(x0: f64, y0: f64, x1: f64, y1: f64) -> Seg2 {
        Seg2::Line { a: Vec2::new(x0, y0), b: Vec2::new(x1, y1) }
    }

    /// A quarter turn of radius 8 about (x, y), starting `from` quarter turns round.
    fn round8(x: f64, y: f64, from: f64) -> Seg2 {
        let quarter = std::f64::consts::FRAC_PI_2;
        Seg2::Arc { center: Vec2::new(x, y), radius: 8.0, start: from * quarter, sweep: quarter }
    }

    /// A 200 x 100 plate with its corners rounded 8.
    fn rounded_plate() -> SheetBody {
        outlined(vec![
            straight(8.0, 0.0, 192.0, 0.0),
            round8(192.0, 8.0, -1.0),
            straight(200.0, 8.0, 200.0, 92.0),
            round8(192.0, 92.0, 0.0),
            straight(192.0, 100.0, 8.0, 100.0),
            round8(8.0, 92.0, 1.0),
            straight(0.0, 92.0, 0.0, 8.0),
            round8(8.0, 8.0, 2.0),
        ])
    }

    /// An outline's arcs of radius 8, its edges, its length and its shortest edge.
    fn arcs_and_edges(l: &Loop2) -> (usize, usize, f64, f64) {
        let arcs = l.segs.iter().filter(|s| matches!(s, Seg2::Arc { radius, .. } if (*radius - 8.0).abs() < 1e-9)).count();
        (arcs, l.segs.len(), l.segs.iter().map(Seg2::length).sum(), l.segs.iter().map(Seg2::length).fold(f64::MAX, f64::min))
    }

    /// ∫ √(64 − z²): the area under an arc of radius 8, from its centre's height on.
    fn under8(z: f64) -> f64 {
        z / 2.0 * (64.0 - z * z).sqrt() + 32.0 * (z / 8.0).asin()
    }

    #[test]
    fn a_base_trimmed_for_a_flange_keeps_its_arcs() {
        // A 200 x 100 plate with its corners rounded 8 and a 90° flange 40 high on its y = 0 edge
        // (setback 3 inside, half of it in the middle, none outside; BA = π; panel 37). Bent
        // inside or in the middle, the base is cut back through the two lower arcs. It used to
        // become the outline's 0.001 mm polyline then: about 150 chords of a quarter millimetre,
        // and no arc.
        let (ba, panel, quarter) = (std::f64::consts::PI, 37.0, std::f64::consts::FRAC_PI_2);
        let whole = 20_000.0 - (4.0 - std::f64::consts::PI) * 64.0;
        // The plate below the height y: ∫ 184 + 2·√(64 − (8 − y)²).
        let below = |y: f64| 184.0 * y + 2.0 * (under8(8.0) - under8(8.0 - y));
        for (position, line) in [("outside", 0.0f64), ("middle", 3.0 / 2.0), ("inside", 3.0)] {
            let mut s = rounded_plate();
            let i = s.add_flange(Vec3::new(100.0, 0.0, 2.0), 40.0, quarter, 1.0, position, false).unwrap();
            // The trimmed line runs from arc to arc, `reach` past the ends of the edge as drawn.
            let reach = (64.0 - (8.0 - line) * (8.0 - line)).sqrt();
            let f = s.flanges[i].clone();
            assert!(f.p.dist(Vec2::new(8.0 - reach, line)) < 1e-9 && (f.len - (184.0 + 2.0 * reach)).abs() < 1e-9, "{position}: {f:?}");
            // The eight edges as drawn: the upper arcs whole, the lower ones from the plate's
            // sides to the trimmed line, an angle asin((8 − line)/8) each.
            let (arcs, edges, length, shortest) = arcs_and_edges(&s.base.outer);
            assert!((arcs, edges) == (4, 8) && shortest > 2.0, "{position}: {arcs} arcs of {edges} edges, shortest {shortest:e}");
            let outline = f.len + 2.0 * 84.0 + 184.0 + 8.0 * std::f64::consts::PI + 16.0 * ((8.0 - line) / 8.0).asin();
            assert!((length - outline).abs() < 1e-9, "{position}: {length} vs {outline}");
            assert!((s.base.area() - (whole - below(line))).abs() < 1e-9, "{position}: {} vs {}", s.base.area(), whole - below(line));
            // The flat pattern: the base and f.len × (BA + 37) of flange, its outline the base's
            // with the flange's two sides (bend and panel, a line each) in place of nothing.
            let regions = s.flat_regions().unwrap();
            let (arcs, edges, length, shortest) = arcs_and_edges(&regions[0].outer);
            assert!(regions[0].outer.is_line_arc() && (arcs, edges) == (4, 12) && shortest > 2.0, "{position}: {:?}", regions[0].outer);
            assert!((length - (outline + 2.0 * (ba + panel))).abs() < 1e-9, "{position}: {length}");
            let flat = whole - below(line) + f.len * (ba + panel);
            assert!((regions[0].area() - flat).abs() < 1e-9, "{position}: {} vs {flat}", regions[0].area());
            holds_the_flat_material(&s, position);
            // It is the trim the outline's polyline got, to within its chords: 50 to a quarter
            // turn, each 0.001 mm inside the arc, so that the trimmed line ends up to
            // 0.001/sin 36° = 0.0017 short of the arc where it crosses it at that angle.
            let mut c = chorded(&rounded_plate());
            c.add_flange(Vec3::new(100.0, 0.0, 2.0), 40.0, quarter, 1.0, position, false).unwrap();
            assert!(c.base.outer.segs.len() > 100 && c.base.area() < s.base.area() && s.base.area() - c.base.area() < 0.05, "{position}");
            assert!(c.flanges[0].p.dist(f.p) < 2e-3 && (c.flanges[0].len - f.len).abs() < 4e-3, "{position}: {:?} vs {f:?}", c.flanges[0]);
        }
        // Flanges inside on two adjacent edges: each trim passes through the arc of the corner
        // between them, which goes. The bends meet at the sharp corner that leaves, (197, 3),
        // and close there as on a square plate; the other three arcs stay.
        let root = 39f64.sqrt();
        let picks = [Vec3::new(100.0, 0.0, 2.0), Vec3::new(200.0, 50.0, 2.0)];
        let mut s = rounded_plate();
        let (made, notes) = s.add_flanges_noted(&picks, 40.0, quarter, 1.0, "inside", false).unwrap();
        assert!(notes.is_empty() && s.corners(&made).len() == 1, "{notes:?}");
        let (arcs, edges, _, shortest) = arcs_and_edges(&s.base.outer);
        assert!((arcs, edges) == (3, 7) && shortest > 2.0, "{:?}", s.base.outer);
        assert!(corners_of(&s.base).iter().any(|q| q.dist(Vec2::new(197.0, 3.0)) < 1e-9), "{:?}", s.base.outer);
        // The plate less the strip under either edge, and the part of the corner both strips
        // hold counted once: x 197…200 below y = 3, inside the corner's arc about (192, 8).
        let right = 84.0 * 3.0 + 2.0 * (under8(8.0) - under8(5.0));
        let shared = under8(root) - under8(5.0) - 5.0 * (root - 5.0);
        let base = whole - below(3.0) - right + shared;
        assert!((s.base.area() - base).abs() < 1e-9, "{} vs {base}", s.base.area());
        let (bottom, side) = (&s.flanges[made[0]], &s.flanges[made[1]]);
        assert!(bottom.p.dist(Vec2::new(8.0 - root, 3.0)) < 1e-9 && (bottom.len - (189.0 + root)).abs() < 1e-9, "{bottom:?}");
        assert!(side.p.dist(Vec2::new(197.0, 3.0)) < 1e-9 && (side.len - (89.0 + root)).abs() < 1e-9, "{side:?}");
        // The square plate: the same corner, the base 197 × 97, either flange 8 − √39 longer.
        let mut square = plate_with_cuts(&[]);
        let (corners, _) = square.add_flanges_noted(&picks, 40.0, quarter, 1.0, "inside", false).unwrap();
        let near = |x: (f64, f64), y: (f64, f64)| (x.0 - y.0).abs() < 1e-9 && (x.1 - y.1).abs() < 1e-9;
        for (x, y) in made.iter().zip(&corners).map(|(&x, &y)| (&s.flanges[x], &square.flanges[y])) {
            assert!(near(x.ext, y.ext) && near(x.slope, y.slope), "{x:?} vs {y:?}");
        }
        let want = flat_area(&square) - (197.0 * 97.0 - base) - 2.0 * (8.0 - root) * (ba + panel);
        assert!((flat_area(&s) - want).abs() < 1e-6, "{} vs {want}", flat_area(&s));
        holds_the_flat_material(&s, "two edges");
    }

    #[test]
    fn a_trim_that_stops_at_a_leg_or_a_cut_out_keeps_the_arc_at_its_other_end() {
        let (ba, quarter, root) = (std::f64::consts::PI, std::f64::consts::FRAC_PI_2, 39f64.sqrt());
        // A 200 x 40 bar with a 30 x 20 leg on its upper edge at the left and its upper right
        // corner rounded 8, and a flange 15 high inside on the upper edge beside the leg. The
        // trim stops at the leg, where the rule's bend relief (2 wide, 1 deep) is cut, and runs
        // through the arc at the other end, to x = 192 + √39 on the bend line y = 37.
        let hook = || {
            outlined(vec![
                straight(0.0, 0.0, 200.0, 0.0),
                straight(200.0, 0.0, 200.0, 32.0),
                round8(192.0, 32.0, 0.0),
                straight(192.0, 40.0, 30.0, 40.0),
                straight(30.0, 40.0, 30.0, 60.0),
                straight(30.0, 60.0, 0.0, 60.0),
                straight(0.0, 60.0, 0.0, 0.0),
            ])
        };
        let mut s = hook();
        let (made, notes) = s.add_flanges_noted(&[Vec3::new(110.0, 40.0, 2.0)], 15.0, quarter, 1.0, "inside", false).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let f = s.flanges[made[0]].clone();
        assert!(f.p.dist(Vec2::new(192.0 + root, 37.0)) < 1e-9 && (f.len - (162.0 + root)).abs() < 1e-9, "{f:?}");
        // The arc from the bar's side to the bend line, the bend line, the relief's four edges
        // in the leg, and the six edges the trim left alone.
        let (arcs, edges, _, shortest) = arcs_and_edges(&s.base.outer);
        assert!((arcs, edges) == (1, 11) && (shortest - 1.0).abs() < 1e-9, "{:?}", s.base.outer);
        for p in [(30.0, 37.0), (30.0, 36.0), (28.0, 36.0), (28.0, 51.0 + ba), (30.0, 51.0 + ba), (30.0, 60.0)] {
            assert!(corners_of(&s.base).iter().any(|q| q.dist(Vec2::new(p.0, p.1)) < 1e-9), "{p:?} not in {:?}", s.base.outer);
        }
        // By hand: the bar and the leg less the rounded corner (64 − 16π); the strip, 162 × 3
        // and what the corner holds above y = 37; and the relief, 2 wide from 1 below the bend
        // line to 2 past the flange's flat length: 2 × (1 + BA + 12 + 2), up to y = 51 + BA.
        let base = 8600.0 - (64.0 - 16.0 * ba) - (486.0 + under8(8.0) - under8(5.0)) - 2.0 * (ba + 15.0);
        assert!((s.base.area() - base).abs() < 1e-9, "{} vs {base}", s.base.area());
        assert!((flat_area(&s) - (base + f.len * (ba + 12.0))).abs() < 1e-6, "{}", flat_area(&s));
        holds_the_flat_material(&s, "hook");
        let mut c = chorded(&hook());
        c.add_flange(Vec3::new(110.0, 40.0, 2.0), 15.0, quarter, 1.0, "inside", false).unwrap();
        assert!(c.base.area() < s.base.area() && s.base.area() - c.base.area() < 0.05, "{} vs {}", s.base.area(), c.base.area());
        assert!(c.flanges[0].p.dist(f.p) < 2e-3 && (c.flanges[0].len - f.len).abs() < 2e-3, "{:?} vs {f:?}", c.flanges[0]);
        // The rounded plate with a notch 40 wide and 30 deep in the middle of its y = 0 edge,
        // and a flange 40 high inside on what the notch leaves at the right: the trim starts
        // square at the notch (which frees the bend's end: no relief) and runs through the lower
        // right arc. The base under the notch and on its other side keeps its edge and its arc.
        let mut s = rounded_plate();
        s.holes.push(block(80.0, 120.0, -10.0, 30.0));
        let (made, notes) = s.add_flanges_noted(&[Vec3::new(160.0, 0.0, 2.0)], 40.0, quarter, 1.0, "inside", false).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let f = s.flanges[made[0]].clone();
        assert!(f.p.dist(Vec2::new(120.0, 3.0)) < 1e-9 && (f.len - (72.0 + root)).abs() < 1e-9, "{f:?}");
        // The edge as far as the notch's end, the step, the bend line, and the seven edges on.
        let (arcs, edges, _, shortest) = arcs_and_edges(&s.base.outer);
        assert!((arcs, edges) == (4, 10) && (shortest - 3.0).abs() < 1e-9, "{:?}", s.base.outer);
        assert!(s.base.contains(Vec2::new(40.0, 1.0)) && s.base.contains(Vec2::new(100.0, 1.0)) && !s.base.contains(Vec2::new(160.0, 1.0)));
        let whole = 20_000.0 - (4.0 - std::f64::consts::PI) * 64.0;
        let base = whole - (72.0 * 3.0 + under8(8.0) - under8(5.0));
        assert!((s.base.area() - base).abs() < 1e-9, "{} vs {base}", s.base.area());
        assert!((flat_area(&s) - (base - 40.0 * 30.0 + f.len * (ba + 37.0))).abs() < 1e-6, "{}", flat_area(&s));
        holds_the_flat_material(&s, "notch");
    }

    #[test]
    fn a_tab_trimmed_for_a_flange_leaves_the_arcs_of_the_tab_beside_it_whole() {
        // A 200 x 60 plate with two tabs 40 wide and 25 long on its y = 0 edge (x 30…70 and
        // 130…170), their outer corners rounded 8, and a flange 15 high inside on the end of the
        // first. The second tab's end lies on the same line: the strip that goes ends in the gap
        // between the two, so the second keeps its end and both its arcs whole.
        let (ba, quarter, root) = (std::f64::consts::PI, std::f64::consts::FRAC_PI_2, 39f64.sqrt());
        let tab = |x: f64| {
            [
                straight(x, 0.0, x, -17.0),
                round8(x + 8.0, -17.0, 2.0),
                straight(x + 8.0, -25.0, x + 32.0, -25.0),
                round8(x + 32.0, -17.0, 3.0),
                straight(x + 40.0, -17.0, x + 40.0, 0.0),
            ]
        };
        let mut segs = vec![straight(0.0, 0.0, 30.0, 0.0)];
        segs.extend(tab(30.0));
        segs.push(straight(70.0, 0.0, 130.0, 0.0));
        segs.extend(tab(130.0));
        segs.extend([
            straight(170.0, 0.0, 200.0, 0.0),
            straight(200.0, 0.0, 200.0, 60.0),
            straight(200.0, 60.0, 0.0, 60.0),
            straight(0.0, 60.0, 0.0, 0.0),
        ]);
        let mut s = outlined(segs);
        // The plate and two tabs, each less its two rounded corners (64 − 16π).
        let whole = 12_000.0 + 2.0 * (1000.0 - 2.0 * (64.0 - 16.0 * ba));
        assert!((s.base.area() - whole).abs() < 1e-9, "{}", s.base.area());
        let (made, notes) = s.add_flanges_noted(&[Vec3::new(50.0, -25.0, 2.0)], 15.0, quarter, 1.0, "inside", false).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let f = s.flanges[made[0]].clone();
        assert!(f.p.dist(Vec2::new(38.0 - root, -22.0)) < 1e-9 && (f.len - (24.0 + 2.0 * root)).abs() < 1e-9, "{f:?}");
        // The sixteen edges as drawn: the first tab's arcs end on the bend line, the second's
        // are the quarter turns they were.
        let (arcs, edges, _, shortest) = arcs_and_edges(&s.base.outer);
        assert!((arcs, edges) == (4, 16) && shortest > 2.0, "{:?}", s.base.outer);
        let quarters = |x0: f64| {
            let turn =
                |e: &&Seg2| matches!(e, Seg2::Arc { center, sweep, .. } if center.x > x0 && center.x < x0 + 40.0 && (sweep - quarter).abs() < 1e-12);
            s.base.outer.segs.iter().filter(turn).count()
        };
        assert_eq!((quarters(30.0), quarters(130.0)), (0, 2), "{:?}", s.base.outer);
        // The first tab loses 24 × 3 and what its two corners hold below the bend line.
        let base = whole - (72.0 + 2.0 * (under8(8.0) - under8(5.0)));
        assert!((s.base.area() - base).abs() < 1e-9, "{} vs {base}", s.base.area());
        assert!((flat_area(&s) - (base + f.len * (ba + 12.0))).abs() < 1e-6, "{}", flat_area(&s));
        holds_the_flat_material(&s, "tabs");
    }

    #[test]
    fn a_base_trimmed_through_a_free_form_curve_follows_it_with_arcs() {
        // A 200 x 60 plate under a parabola 40 high: y = 60 + 160·(x/200)·(1 − x/200), a conic
        // of weight 1. A flange 40 high inside on the right edge trims the base to x = 197,
        // through the curve's end: 183.564 goes (∫ y from 197 to 200), and the bend runs up the
        // line x = 197 to the curve, at y = 62.364. The planar booleans follow a curve they
        // cut with tangent arcs within 0.001 mm: over the parabola's 220 mm that is at most
        // 0.22 mm² of the base, and 0.001/cos 38° = 0.0013 of the bend where it meets the curve.
        // It used to become about 200 chords.
        let (ba, quarter) = (std::f64::consts::PI, std::f64::consts::FRAC_PI_2);
        let crowned = || {
            outlined(vec![
                straight(0.0, 0.0, 200.0, 0.0),
                straight(200.0, 0.0, 200.0, 60.0),
                Seg2::Conic { a: Vec2::new(200.0, 60.0), apex: Vec2::new(100.0, 140.0), b: Vec2::new(0.0, 60.0), w: 1.0 },
                straight(0.0, 60.0, 0.0, 0.0),
            ])
        };
        let whole = 200.0 * 60.0 + 200.0 * 40.0 * 2.0 / 3.0;
        assert!((crowned().base.area() - whole).abs() < 1e-9);
        // (the pick, the strip that goes, where the bend starts, how long it is)
        let cases = [
            ("right", Vec3::new(200.0, 30.0, 2.0), 183.564, Vec2::new(197.0, 0.0), 62.364),
            // On the bottom edge the trim leaves the curve alone: 200 × 3 goes.
            ("bottom", Vec3::new(100.0, 0.0, 2.0), 600.0, Vec2::new(0.0, 3.0), 200.0),
        ];
        for (what, pick, strip, start, len) in cases {
            let mut s = crowned();
            let i = s.add_flange(pick, 40.0, quarter, 1.0, "inside", false).unwrap();
            let outer = &s.base.outer;
            let arcs = outer.segs.iter().filter(|e| matches!(e, Seg2::Arc { .. })).count();
            assert!(outer.is_line_arc() && arcs > 1 && outer.segs.len() == arcs + 3 && arcs < 64, "{what}: {arcs} arcs of {}", outer.segs.len());
            assert!((s.base.area() - (whole - strip)).abs() < 0.22, "{what}: {} vs {}", s.base.area(), whole - strip);
            let f = s.flanges[i].clone();
            assert!(f.p.dist(start) < 1e-9 && (f.len - len).abs() < 2e-3, "{what}: {f:?}");
            assert!((flat_area(&s) - (s.base.area() + f.len * (ba + 37.0))).abs() < 1e-6, "{what}: {}", flat_area(&s));
            holds_the_flat_material(&s, what);
            let mut c = chorded(&crowned());
            c.add_flange(pick, 40.0, quarter, 1.0, "inside", false).unwrap();
            assert!(c.base.outer.segs.len() > 150 && (c.base.area() - s.base.area()).abs() < 0.22, "{what}: {}", c.base.outer.segs.len());
        }
    }

    /// Edge flanges on `base`, each with its own height and angle, added in one call. Returns the
    /// sheet and the notes on corners left open.
    fn flanges_each(base: Region2, rv: &RuleValues, each: &[EdgeFlange]) -> (SheetBody, Vec<String>) {
        let mut s = base_flange("Body1", "AL2", rv, Plane::XY, base);
        let notes = s.add_flanges_each(each, rv.bend_radius, "inside", false, ReliefOverride::default()).unwrap().1;
        (s, notes)
    }

    /// The 120 x 90 tray's four flanges (bottom, right, top, left), each with its own angle
    /// (degrees) and height.
    fn tray_each(rv: &RuleValues, angles: [f64; 4], heights: [f64; 4]) -> (SheetBody, Vec<String>) {
        let base = rect(Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0));
        let t = rv.thickness;
        let picks = [Vec3::new(60.0, 0.0, t), Vec3::new(120.0, 45.0, t), Vec3::new(60.0, 90.0, t), Vec3::new(0.0, 45.0, t)];
        let each: Vec<EdgeFlange> = (0..4).map(|i| EdgeFlange { pick: picks[i], height: heights[i], angle: angles[i].to_radians() }).collect();
        flanges_each(base, rv, &each)
    }

    /// The material of a base flange bent up, for asking whether a folded point lies in it. Worked
    /// out from the fold on its own (not through `map3`): the panel through the inverse of its fold,
    /// the bend from the radius and turn about the bend's axis.
    struct Material {
        back: Mat,
        panel: Vec<Vec2>,
        axis: (Vec3, Vec3),
        p: Vec2,
        d: Vec2,
        span: (f64, f64),
        ends: (f64, f64),
        radius: f64,
        angle: f64,
        t: f64,
    }

    impl Material {
        fn of(s: &SheetBody, i: usize) -> Material {
            let f = &s.flanges[i];
            assert!(f.parent.is_none() && f.angle > 0.0);
            let (o, k, _) = s.bend_axis(f);
            Material {
                back: mat_inverse(&s.fold(i)).unwrap(),
                panel: corners_of(&s.panel(f)),
                axis: (o, k),
                p: f.p,
                d: f.d,
                span: s.zone_span(f),
                ends: s.zone_ends(f),
                radius: f.radius,
                angle: f.angle,
                t: s.t,
            }
        }

        /// How deep (mm) `q` lies inside the flange's panel or bend; None when it is outside both.
        fn depth(&self, q: Vec3) -> Option<f64> {
            let flat = apply_point(&self.back, q);
            let m = self.panel.len();
            let sides = (0..m).map(|j| {
                let (a, b) = (self.panel[j], self.panel[(j + 1) % m]);
                (b - a).cross(Vec2::new(flat.x, flat.y) - a) / (b - a).len()
            });
            let in_panel = sides.fold(flat.z.min(self.t - flat.z), f64::min);
            // The bend turns the sheet under its axis toward the flange: a point's distance from
            // the axis is its radius, its turn from straight below the axis its place across the bend.
            let (o, k) = self.axis;
            let rel = q - o;
            let perp = rel - k * rel.dot(k);
            let down = Vec3::Z * -1.0;
            let (rho, turn) = (perp.len(), perp.dot(k.cross(down)).atan2(perp.dot(down)));
            let u = (Vec2::new(q.x, q.y) - self.p).dot(self.d);
            // A torn end runs along the bend as the bend turns, from the start line's end to the panel's.
            let (span, ends, x) = (self.span, self.ends, turn / self.angle);
            let (u0, u1) = (span.0 + (ends.0 - span.0) * x, span.1 + (ends.1 - span.1) * x);
            let in_bend = [rho - self.radius, self.radius + self.t - rho, turn * rho, (self.angle - turn) * rho, u - u0, u1 - u]
                .into_iter()
                .fold(f64::INFINITY, f64::min);
            let depth = in_panel.max(in_bend);
            (depth > 0.0).then_some(depth)
        }
    }

    /// Folded, the two flanges at corner `k` keep out of each other: no point of one's bend or
    /// panel near the corner (sampled every 0.5 mm or finer, at three heights through the sheet)
    /// lies in the other's. Each sample is also found inside its own flange, which checks
    /// [`Material`] against the fold that places the solid's faces.
    fn flanges_keep_clear(s: &SheetBody, k: &Corner, what: &str) {
        let (ma, mb) = (Material::of(s, k.a), Material::of(s, k.b));
        for (i, at_end, own, other) in [(k.a, k.a_end, &ma, &mb), (k.b, k.b_end, &mb, &ma)] {
            let f = &s.flanges[i];
            let ba = s.allowance(f);
            let mut samples = Vec::new();
            // The bend's 12 mm nearest the corner, from its end (which a tear leaves askew).
            let ((u0, u1), (e0, e1)) = (s.zone_span(f), s.zone_ends(f));
            for iu in 0..48 {
                let du = 0.02 + 0.25 * iu as f64;
                for is in 0..24 {
                    let x = (is as f64 + 0.5) / 24.0;
                    let u = if at_end { u1 + (e1 - u1) * x - du } else { u0 + (e0 - u0) * x + du };
                    samples.push((Piece::Zone(i), f.p + f.d * u + f.n * (ba * x)));
                }
            }
            // The panel's 25 mm nearest its end, over its whole height.
            let (q0, q1, inward) = if at_end {
                let b = f.p + f.n * ba + f.d * (f.len + f.ext.1);
                (b, b + f.n * f.leg + f.d * (f.slope.1 * f.leg), f.d * -1.0)
            } else {
                let a = f.p + f.n * ba - f.d * f.ext.0;
                (a, a + f.n * f.leg - f.d * (f.slope.0 * f.leg), f.d)
            };
            let rows = (f.leg / 0.5).ceil().max(1.0) as usize;
            for ir in 0..rows {
                let on_end = q0 + (q1 - q0) * ((ir as f64 + 0.5) / rows as f64);
                for iu in 0..50 {
                    samples.push((Piece::Panel(Some(i)), on_end + inward * (0.02 + 0.5 * iu as f64)));
                }
            }
            for (piece, flat) in samples {
                for z in [0.1 * s.t, 0.5 * s.t, 0.9 * s.t] {
                    let q = s.map3(piece, flat, z);
                    assert!(own.depth(q).is_some(), "{what}: flange {i}: its own point {flat:?} at {z} ({piece:?}) is not found in it");
                    let d = other.depth(q);
                    assert!(
                        d.is_none_or(|d| d < 1e-6),
                        "{what}: flange {i}'s {piece:?} at {flat:?}, height {z}, lies {d:?} mm inside the other flange"
                    );
                }
            }
        }
    }

    /// The panel of flange `i` of the tray by hand, in the flat pattern. Take a flange at θ on the
    /// edge y = 0 and its neighbour at φ on x = 0, T the thickness. A point of the first one's inner
    /// face, w up its outer face from the virtual sharp, is (x, −w·cosθ + T·sinθ, w·sinθ + T·cosθ);
    /// the inner faces are the planes y·sinθ + z·cosθ = T and x·sinφ + z·cosφ = T, and the plane
    /// halving them passes through the corner with normal (sinφ, −sinθ, cosφ − cosθ), of length
    /// √(2 − 2·cosθ·cosφ). Keeping the inner face gap/2 from it gives the panel's end
    /// x(w) = ((g/2)·√(2 − 2·cosθ·cosφ) + T·(1 − cosθ·cosφ) − w·sinθ·cosφ) / sinφ for w = s0 … h, at
    /// flat y = s0 − BA − (w − s0). Each flange is taken in its own frame: the origin at the start
    /// corner of its edge, u along the edge, v into the base; flange i − 1 is at its start and
    /// flange i + 1 at its end.
    fn hand_panel(rv: &RuleValues, angles: [f64; 4], heights: [f64; 4], i: usize) -> Vec<Vec2> {
        let (t, r, g) = (rv.thickness, rv.bend_radius, rv.gap);
        let (o, d, v, len) = [
            (Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), 120.0),
            (Vec2::new(120.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0), 90.0),
            (Vec2::new(120.0, 90.0), Vec2::new(-1.0, 0.0), Vec2::new(0.0, -1.0), 120.0),
            (Vec2::new(0.0, 90.0), Vec2::new(0.0, -1.0), Vec2::new(1.0, 0.0), 90.0),
        ][i];
        let th = angles[i].to_radians();
        let (s0, ba) = ((r + t) * (th / 2.0).tan(), th * (r + rv.k_factor * t));
        let x_end = |phi: f64, w: f64| {
            let c = th.cos() * phi.cos();
            ((g / 2.0) * (2.0 - 2.0 * c).sqrt() + t * (1.0 - c) - w * th.sin() * phi.cos()) / phi.sin()
        };
        let (prev, next) = (angles[(i + 3) % 4].to_radians(), angles[(i + 1) % 4].to_radians());
        let flat = |u: f64, w: f64| o + d * u + v * (s0 - ba - (w - s0));
        let h = heights[i];
        vec![flat(x_end(prev, s0), s0), flat(x_end(prev, h), h), flat(len - x_end(next, h), h), flat(len - x_end(next, s0), s0)]
    }

    /// The tray's flat area by hand: the base inside the four bend start lines, every bend zone
    /// (the base's trimmed edge by the bend allowance) and every panel of [`hand_panel`]. With
    /// s0 = (R + T)·tan(θ/2) a flange's setback and BA = θ·(R + K·T) its bend allowance, the base
    /// is (120 − s0_left − s0_right) by (90 − s0_bottom − s0_top).
    fn hand_flat_area(rv: &RuleValues, angles: [f64; 4], heights: [f64; 4]) -> f64 {
        let s0 = angles.map(|a| (rv.bend_radius + rv.thickness) * (a.to_radians() / 2.0).tan());
        let ba = angles.map(|a| a.to_radians() * (rv.bend_radius + rv.k_factor * rv.thickness));
        let (w, h) = (120.0 - s0[3] - s0[1], 90.0 - s0[0] - s0[2]);
        let panels: f64 = (0..4).map(|i| signed_area(&hand_panel(rv, angles, heights, i)).abs()).sum();
        w * h + (ba[0] + ba[2]) * w + (ba[1] + ba[3]) * h + panels
    }

    /// 60°, 90°, 105° and 120° on the tray's bottom, right, top and left edges.
    const MIXED: [f64; 4] = [60.0, 90.0, 105.0, 120.0];

    #[test]
    fn flanges_with_their_own_angle_and_height_close_at_their_corners() {
        // Four angles on a 120 x 90 tray, then four heights as well, in one call: every panel ends
        // where the hand derivation says, whatever its neighbour's angle and height.
        let rv = alu();
        for (heights, what) in [([38.6; 4], "one height"), ([38.6, 30.0, 45.0, 25.0], "four heights")] {
            let (s, notes) = tray_each(&rv, MIXED, heights);
            assert!(notes.is_empty(), "{what}: {notes:?}");
            for i in 0..4 {
                let (f, th) = (&s.flanges[i], MIXED[i].to_radians());
                assert!((f.angle - th).abs() < 1e-12, "{what}: flange {i}");
                assert!((f.leg - (heights[i] - (rv.bend_radius + rv.thickness) * (th / 2.0).tan())).abs() < 1e-9, "{what}: flange {i}");
                let got = corners_of(&s.panel(f));
                assert_eq!(got.len(), 4, "{what}: flange {i}");
                for q in hand_panel(&rv, MIXED, heights, i) {
                    assert!(got.iter().any(|p| p.dist(q) < 1e-9), "{what}: flange {i}: {q:?} not in {got:?}");
                }
            }
            // The flat pattern is the base, the bend zones and those panels, nothing more or less;
            // folded it holds the same material (`holds_the_flat_material`), so the volume is this
            // area times the thickness.
            let (area, want) = (flat_area(&s), hand_flat_area(&rv, MIXED, heights));
            assert!((area - want).abs() < 1e-6, "{what}: flat area {area} vs {want}");
            let corners = s.corners(&[0, 1, 2, 3]);
            assert_eq!(corners.len(), 4, "{what}");
            ends_are_gap_apart(&s, &corners, rv.gap, what);
            for k in &corners {
                flanges_keep_clear(&s, k, what);
            }
            holds_the_flat_material(&s, what);
        }
    }

    #[test]
    fn a_trapezoid_with_unequal_angles_and_heights_closes_at_its_acute_and_obtuse_corners() {
        // 77.5° and 102.5° base corners; flanges at 60°, 105°, 75° and 120°, each its own height.
        let pts = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(100.0, 90.0), Vec2::new(20.0, 90.0)];
        let picks = [Vec3::new(60.0, 0.0, 2.0), Vec3::new(110.0, 45.0, 2.0), Vec3::new(60.0, 90.0, 2.0), Vec3::new(10.0, 45.0, 2.0)];
        let (angles, heights) = ([60.0f64, 105.0, 75.0, 120.0], [38.6, 30.0, 45.0, 25.0]);
        let each: Vec<EdgeFlange> = (0..4).map(|i| EdgeFlange { pick: picks[i], height: heights[i], angle: angles[i].to_radians() }).collect();
        let (s, notes) = flanges_each(Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] }, &alu(), &each);
        assert!(notes.is_empty(), "{notes:?}");
        for i in 0..4 {
            let (f, th) = (&s.flanges[i], angles[i].to_radians());
            assert!((f.angle - th).abs() < 1e-12 && (f.leg - (heights[i] - 3.0 * (th / 2.0).tan())).abs() < 1e-9, "flange {i}: {f:?}");
        }
        let corners = s.corners(&[0, 1, 2, 3]);
        assert_eq!(corners.len(), 4);
        ends_are_gap_apart(&s, &corners, 0.5, "trapezoid");
        for k in &corners {
            flanges_keep_clear(&s, k, "trapezoid");
        }
        holds_the_flat_material(&s, "trapezoid");
    }

    /// Flanges `height` high at `deg` on the edges `on` of the base `pts` (counter-clockwise; edge k
    /// leaves corner k), in one call, bend inside, their corners closed.
    fn walls(rv: &RuleValues, pts: &[Vec2], on: &[usize], deg: f64, height: f64) -> SheetBody {
        let n = pts.len();
        let each: Vec<EdgeFlange> = on
            .iter()
            .map(|&k| {
                let m = (pts[k] + pts[(k + 1) % n]) * 0.5;
                EdgeFlange { pick: Vec3::new(m.x, m.y, rv.thickness), height, angle: deg.to_radians() }
            })
            .collect();
        let (s, notes) = flanges_each(Region2 { outer: Loop2::polygon(pts).ccw(), holes: vec![] }, rv, &each);
        assert!(notes.is_empty(), "{notes:?}");
        s
    }

    /// The middle of flange `i`'s far edge, folded, on its inner face (the one toward its bend's
    /// axis) or its outer one: a flange picked there turns the way flange `i` turned, or the other.
    fn far_edge_pick(s: &SheetBody, i: usize, inner: bool) -> Vec3 {
        let f = &s.flanges[i];
        let (lo, hi) = s.far_edge(f);
        let q = f.p + f.n * (s.allowance(f) + f.leg) + f.d * ((lo + hi) / 2.0);
        let z = if (f.angle >= 0.0) == inner { s.t } else { 0.0 };
        apply_point(&s.world(Some(i)), Vec3::new(q.x, q.y, z))
    }

    /// Returns `height` high at `deg` on the far edges of the flanges `on`, in one call, turning in
    /// over the base (bend inside). The new flanges and the notes on corners left open.
    fn returns(s: &mut SheetBody, on: &[usize], deg: f64, height: f64) -> (Vec<usize>, Vec<String>) {
        let picks: Vec<Vec3> = on.iter().map(|&i| far_edge_pick(s, i, true)).collect();
        s.add_flanges_noted(&picks, height, deg.to_radians(), 1.0, "inside", false).unwrap()
    }

    /// Folded points of flange `i`'s material: its panel's corners and, with `bend`, its bend's
    /// cross-section at both ends (65 points across it), each on both faces.
    fn material(s: &SheetBody, i: usize, bend: bool) -> Vec<Vec3> {
        let f = &s.flanges[i];
        let mut v = Vec::new();
        if f.leg > 1e-9 {
            let w = s.fold(i);
            for q in corners_of(&s.panel(f)) {
                v.extend([0.0, s.t].map(|z| apply_point(&w, Vec3::new(q.x, q.y, z))));
            }
        }
        if bend {
            let ba = s.allowance(f);
            let (u0, u1) = s.zone_span(f);
            for u in [u0, u1] {
                for k in 0..=64 {
                    let q = f.p + f.d * u + f.n * (ba * k as f64 / 64.0);
                    v.extend([0.0, s.t].map(|z| s.map3(Piece::Zone(i), q, z)));
                }
            }
        }
        v
    }

    /// The least and greatest signed distance of points from the plane through `o` with normal `m`.
    fn spread(pts: &[Vec3], o: Vec3, m: Vec3) -> (f64, f64) {
        pts.iter().map(|p| (*p - o).dot(m)).fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)))
    }

    /// Folded, no flange on a wall's far edge reaches into another wall: no point of its bend or
    /// its panel within 25 mm of either end (sampled every 0.25 mm, at three heights through the
    /// sheet) lies in a flange on the base other than the one it stands on.
    fn returns_keep_out_of_the_walls(s: &SheetBody, what: &str) {
        let on_base: Vec<(usize, Material)> =
            (0..s.flanges.len()).filter(|&i| s.flanges[i].parent.is_none()).map(|i| (i, Material::of(s, i))).collect();
        for (i, f) in s.flanges.iter().enumerate() {
            let Some(parent) = f.parent else { continue };
            let ba = s.allowance(f);
            let (u0, u1) = s.zone_span(f);
            let mut samples = Vec::new();
            for at_end in [false, true] {
                for iu in 0..100 {
                    let du = 0.01 + 0.25 * iu as f64;
                    let u = if at_end { u1 - du } else { u0 + du };
                    for is in 0..24 {
                        samples.push((Piece::Zone(i), f.p + f.d * u + f.n * (ba * (is as f64 + 0.5) / 24.0)));
                    }
                }
                let (q0, q1, inward) = if at_end {
                    let b = f.p + f.n * ba + f.d * (f.len + f.ext.1);
                    (b, b + f.n * f.leg + f.d * (f.slope.1 * f.leg), f.d * -1.0)
                } else {
                    let a = f.p + f.n * ba - f.d * f.ext.0;
                    (a, a + f.n * f.leg - f.d * (f.slope.0 * f.leg), f.d)
                };
                let rows = (f.leg / 0.25).ceil().max(1.0) as usize;
                for ir in 0..rows {
                    let on_end = q0 + (q1 - q0) * ((ir as f64 + 0.5) / rows as f64);
                    for iu in 0..100 {
                        samples.push((Piece::Panel(Some(i)), on_end + inward * (0.01 + 0.25 * iu as f64)));
                    }
                }
            }
            for (piece, flat) in samples {
                for z in [0.02 * s.t, 0.5 * s.t, 0.98 * s.t] {
                    let q = s.map3(piece, flat, z);
                    for (w, m) in on_base.iter().filter(|(w, _)| *w != parent) {
                        let d = m.depth(q);
                        assert!(
                            d.is_none_or(|d| d < 1e-6),
                            "{what}: flange {i}'s {piece:?} at {flat:?}, height {z}, lies {d:?} mm inside flange {w}"
                        );
                    }
                }
            }
        }
    }

    /// The two returns at each corner of a four-sided tray, returns 4…7 on walls 0…3: (a, the end
    /// of a, b, the end of b), true for the end past the far end of the bend.
    const TRAY_RETURNS: [(usize, bool, usize, bool); 4] = [(4, false, 7, true), (4, true, 5, false), (5, true, 6, false), (6, true, 7, false)];

    #[test]
    fn returns_on_a_tray_are_mitred_the_gap_apart() {
        use std::f64::consts::{PI, SQRT_2};
        // The 120 x 90 tray with walls 20 high at 90° and, on their far edges, returns 12 high at
        // 90° turning in over the base. Each return used to take its wall's bend length (114 or
        // 84) whatever the wall's closed end did, and neighbours ran through each other: the solid
        // failed to sew.
        let rv = alu();
        let g = rv.gap;
        let (mut s, notes) = tray(&rv, 90.0, 20.0);
        assert!(notes.is_empty(), "{notes:?}");
        let (made, notes) = returns(&mut s, &[0, 1, 2, 3], 90.0, 12.0);
        assert!(notes.is_empty() && made == [4, 5, 6, 7], "{made:?} {notes:?}");
        // By hand (T 2, R 1, K 0.5, g 0.5; e = g/√2). Every bend takes a setback of
        // (R + T)·tan 45° = 3 and is π/2·(R + K·T) = π wide in the flat. The wall on y = 0 starts at
        // the base's trimmed edge y = 3 and is 20 − 3 − 3 = 14 past its bend once the return's
        // setback is off, so its far edge lies at y = 3 − π − 14 in the flat and runs, between its
        // closed ends, from x = 2 + e to 118 − e (as before). Folded, its inner face is y = 2 and
        // the return's bend turns about y = 3, z = 17: the return lies in 18 ≤ z ≤ 20 from y = 3 to
        // y = 12, and its neighbour on x = 0 likewise from x = 3. Their inner start lines y = 3
        // and x = 3 cross at (3, 3), so the mitre is the plane x = y and this return keeps
        // x − y ≥ e. Its bend (y ≤ 3) stops square at x = 3 + e, 1 mm in from the wall's end, and
        // its panel ends on x = 3 + e + (y − 3).
        let e = g / SQRT_2;
        let wall = &s.flanges[0];
        let (lo, hi) = s.far_edge(wall);
        assert!((wall.leg - 14.0).abs() < 1e-12 && (lo - (e - 1.0)).abs() < 1e-9 && (hi - (115.0 - e)).abs() < 1e-9, "{wall:?}");
        let r = &s.flanges[4];
        assert!(r.p.dist(Vec2::new(3.0 + e, -11.0 - PI)) < 1e-9 && (r.len - (114.0 - 2.0 * e)).abs() < 1e-9, "{r:?}");
        assert!(r.ext.0.abs() < 1e-9 && r.ext.1.abs() < 1e-9 && (r.slope.0 + 1.0).abs() < 1e-9 && (r.slope.1 + 1.0).abs() < 1e-9, "{r:?}");
        let lens: Vec<f64> = s.flanges[4..].iter().map(|f| f.len).collect();
        for (got, want) in lens.iter().zip([114.0, 84.0, 114.0, 84.0]) {
            assert!((got - (want - 2.0 * e)).abs() < 1e-9, "{lens:?}");
        }
        // In the flat the return's bend is π wide and its panel 12 − 3 = 9 past it.
        let (y0, y1) = (-11.0 - 2.0 * PI, -20.0 - 2.0 * PI);
        let got = corners_of(&s.panel(r));
        assert_eq!(got.len(), 4);
        for (x, y) in [(3.0 + e, y0), (117.0 - e, y0), (108.0 - e, y1), (12.0 + e, y1)] {
            assert!(got.iter().any(|p| p.dist(Vec2::new(x, y)) < 1e-9), "({x}, {y}) not in {got:?}");
        }
        // Folded, the inner end edges (on z = 18) of the two returns at each corner run parallel,
        // the gap apart.
        for (a, a_end, b, b_end) in TRAY_RETURNS {
            let ((a0, a1), (b0, b1)) = (inner_end_edge(&s, a, a_end), inner_end_edge(&s, b, b_end));
            assert!((a0.z - 18.0).abs() < 1e-9 && (b1.z - 18.0).abs() < 1e-9, "{a0:?} {b1:?}");
            let gap = parallel_gap(a0, a1, b0, b1);
            assert!(gap.is_some_and(|d| (d - g).abs() < 1e-9), "{a}/{b}: {gap:?}");
        }
        // The part stays inside the tray's 120 x 90 x 20.
        let bb = kernel::measure(&s.solid().unwrap()).unwrap().bbox;
        assert!(bb.min.len() < 1e-6 && (bb.max - Vec3::new(120.0, 90.0, 20.0)).len() < 1e-6, "{bb:?}");
        // The flat pattern by hand: the base 114 x 84; the walls' bends π·(114 + 84)·2 and panels
        // 14·((116 − 2e) + (86 − 2e))·2; the returns' bends π·((114 − 2e) + (84 − 2e))·2 and their
        // panels, 9 high and 9 shorter at each end of the far edge, 9·((105 − 2e) + (75 − 2e))·2.
        let want = 114.0 * 84.0 + 396.0 * PI + 28.0 * (202.0 - 4.0 * e) + 2.0 * PI * (198.0 - 4.0 * e) + 18.0 * (180.0 - 4.0 * e);
        assert!((flat_area(&s) - want).abs() < 1e-6, "{} vs {want}", flat_area(&s));
        // One piece in the flat and one valid shell folded, holding that area times the thickness.
        holds_the_flat_material(&s, "returns at 90°");
    }

    /// A tray under rule `rv` on the convex base `pts` (counter-clockwise): walls 20 high at `deg`
    /// on every edge and on each a return 12 high at 180° − `deg`, which turns in over the base and
    /// lies parallel to it. By hand, for a wall at θ and its return at ρ = 180° − θ (T, R, g as the rule has them):
    /// a point of the wall's inner face w up its outer face from the virtual sharp lies
    /// −w·cosθ + T·sinθ in from the base's edge. The return's setback leaves the wall's far edge at
    /// w = 20 − (R + T)·tan(ρ/2), and its bend turns about an axis R inside that inner face, so the
    /// return's panel starts v = −w·cosθ + (T + R)·sinθ in from the edge, and all of its bend lies
    /// further out than that. At a base corner of angle α, a point x along an edge from the corner
    /// and v in from that edge is x·sin(α/2) − v·cos(α/2) from the vertical plane on the corner's
    /// bisector. That plane is the mitre (the two returns lie in one plane and are alike), so each
    /// return's bend stops square at x_b = (g/2 + v·cos(α/2)) / sin(α/2) and its panel ends from
    /// there on a line that loses cot(α/2) per unit of height. Checks:
    /// - every return's bend is its edge less x_b at both corners, its panel starts with the bend
    ///   (no run-on) and its ends slant by −cot(α/2);
    /// - at every corner the two walls' panels and the two returns (bend and panel) keep g/2 from
    ///   the vertical plane on the corner's bisector, each on its own side, the panels' nearest
    ///   points exactly g/2;
    /// - nothing reaches further out past the base's edges than the walls' tips do;
    /// - one piece in the flat, one valid shell folded, holding the flat pattern's material.
    fn tray_with_level_returns_closes_every_corner(rv: &RuleValues, pts: &[Vec2], deg: f64, what: &str) -> SheetBody {
        let (t, r, g) = (rv.thickness, rv.bend_radius, rv.gap);
        let n = pts.len();
        let all: Vec<usize> = (0..n).collect();
        let mut s = walls(rv, pts, &all, deg, 20.0);
        let (made, notes) = returns(&mut s, &all, 180.0 - deg, 12.0);
        assert!(notes.is_empty() && made.len() == n, "{what}: {notes:?}");
        let (th, rho) = (deg.to_radians(), (180.0 - deg).to_radians());
        let v = (t + r) * th.sin() - (20.0 - (r + t) * (rho / 2.0).tan()) * th.cos();
        // Half the base's angle at corner c, and the two directions along its edges.
        let edges = |c: usize| ((pts[(c + n - 1) % n] - pts[c]).normalized().unwrap(), (pts[(c + 1) % n] - pts[c]).normalized().unwrap());
        let half = |c: usize| edges(c).0.dot(edges(c).1).acos() / 2.0;
        let x_b = |c: usize| (g / 2.0 + v * half(c).cos()) / half(c).sin();
        for k in 0..n {
            let (ret, next) = (&s.flanges[n + k], (k + 1) % n);
            let want = pts[k].dist(pts[next]) - x_b(k) - x_b(next);
            assert!((ret.len - want).abs() < 1e-9 && ret.ext.0.abs() < 1e-9 && ret.ext.1.abs() < 1e-9, "{what}: return {k}: {want} vs {ret:?}");
            assert!(
                (ret.slope.0 + 1.0 / half(k).tan()).abs() < 1e-9 && (ret.slope.1 + 1.0 / half(next).tan()).abs() < 1e-9,
                "{what}: return {k}: {ret:?}"
            );
        }
        for (c, corner) in pts.iter().enumerate() {
            let (to_prev, to_next) = edges(c);
            let bis = (to_prev + to_next).normalized().unwrap();
            // The plane's normal points to the side of the edge that leaves the corner (wall c).
            let (o, m) = (Vec3::new(corner.x, corner.y, 0.0), Vec3::new(bis.y, -bis.x, 0.0));
            for (wall, sign) in [((c + n - 1) % n, -1.0), (c, 1.0)] {
                for (i, bend) in [(wall, false), (wall + n, true)] {
                    let (all, panel) = (spread(&material(&s, i, bend), o, m * sign).0, spread(&material(&s, i, false), o, m * sign).0);
                    assert!(all > g / 2.0 - 1e-9 && (panel - g / 2.0).abs() < 1e-9, "{what}: corner {c}, flange {i}: {all} {panel}");
                }
            }
        }
        // A wall leaning out reaches 20·cosθ past its edge at its tip; one leaning in stays inside.
        let reach = (20.0 * th.cos()).max(0.0);
        for k in 0..n {
            let d = (pts[(k + 1) % n] - pts[k]).normalized().unwrap();
            for i in 0..2 * n {
                let far = spread(&material(&s, i, true), Vec3::new(pts[k].x, pts[k].y, 0.0), Vec3::new(d.y, -d.x, 0.0)).1;
                assert!(far < reach + 1e-6, "{what}: flange {i} reaches {far} past edge {k}");
            }
        }
        holds_the_flat_material(&s, what);
        s
    }

    #[test]
    fn returns_on_leaning_walls_close_on_each_corners_bisector() {
        // Walls at 110° (leaning 20° in) with returns at 70°. Every return used to run its wall's
        // bend length from where the wall's far edge started: through the neighbouring wall and
        // out past the corner (to x = 120.504 on the tray, whose walls end at x = 118.716).
        let tray = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0)];
        let s = tray_with_level_returns_closes_every_corner(&alu(), &tray, 110.0, "tray");
        // In numbers: v = 3·sin 110° + (20 − 3·tan 35°)·cos 70° = 8.941025 and x_b = v + g/√2
        // = 9.294579 at a square corner, so the bends are 120 − 2·x_b and 90 − 2·x_b long.
        assert!((s.flanges[4].len - 101.410842).abs() < 1e-6 && (s.flanges[5].len - 71.410842).abs() < 1e-6, "{:?}", s.flanges[4]);
        // The part is no larger than its walls: their bends' outer faces, 3·tan 55° − 3 = 1.284444
        // inside the base's edges (measured on the bends' facets, so to 1e-3), and the returns' top
        // at 20·sin 110° = 18.793852.
        let bb = kernel::measure(&s.solid().unwrap()).unwrap().bbox;
        assert!(
            (bb.min - Vec3::new(1.284444, 1.284444, 0.0)).len() < 1e-3 && (bb.max - Vec3::new(118.715556, 88.715556, 18.793852)).len() < 1e-3,
            "{bb:?}"
        );
        // A trapezoid: corners of 77.5° and 102.5°.
        let trapezoid = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(100.0, 90.0), Vec2::new(20.0, 90.0)];
        let s = tray_with_level_returns_closes_every_corner(&alu(), &trapezoid, 110.0, "trapezoid");
        let bb = kernel::measure(&s.solid().unwrap()).unwrap().bbox;
        assert!(bb.min.y > 1.28 && bb.max.y < 88.72 && bb.min.x > 2.41 && bb.max.x < 117.59, "{bb:?}");
    }

    #[test]
    fn returns_close_at_any_corner_on_walls_leaning_either_way() {
        // Walls leaning out (70°), upright and leaning in (110°), each with level returns, on the
        // tray, the trapezoid and parallelograms with corners of 60° and 120°: 9 parts, 36 corners.
        let (c, sn) = (60f64.to_radians().cos(), 60f64.to_radians().sin());
        let bases = [
            ("tray", [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0)]),
            ("trapezoid", [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(100.0, 90.0), Vec2::new(20.0, 90.0)]),
            ("parallelogram", [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0 + 90.0 * c, 90.0 * sn), Vec2::new(90.0 * c, 90.0 * sn)]),
        ];
        for (name, pts) in bases {
            for deg in [70.0, 90.0, 110.0] {
                tray_with_level_returns_closes_every_corner(&alu(), &pts, deg, &format!("{name}, walls at {deg}°"));
            }
        }
    }

    #[test]
    fn returns_close_at_relieved_corners_as_at_whole_ones() {
        use std::f64::consts::{FRAC_PI_2, PI};
        // A round or rectangular corner relief stops the walls' bends half its size short of the
        // corner and leaves the walls' closed ends as they are, so the returns on them meet as at
        // a whole corner: every check of `tray_with_level_returns_closes_every_corner` holds, and
        // the flanges are the very ones the notch gives. So it is with a relief 36 across, which
        // stops the bends 18 from the corner: further than the walls reach past their bends in the
        // flat (π + 14), where a cut in the base's outline would be an edge of the base. And the
        // relief is cut as without returns: at each corner the flat loses what the relief takes.
        let tray = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0)];
        let trapezoid = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(100.0, 90.0), Vec2::new(20.0, 90.0)];
        let alpha = 90f64.atan2(20.0);
        let cases = [
            ("tray", tray, [FRAC_PI_2; 4], 90.0, 8.0),
            ("tray", tray, [FRAC_PI_2; 4], 90.0, 36.0),
            ("tray", tray, [FRAC_PI_2; 4], 110.0, 8.0),
            ("trapezoid", trapezoid, [alpha, alpha, PI - alpha, PI - alpha], 110.0, 8.0),
        ];
        for shape in [ReliefShape::Round, ReliefShape::Rectangular] {
            for (name, pts, corners, deg, size) in cases {
                let what = format!("{shape:?} relief {size} mm across on the {name}, walls at {deg}°");
                let rv = RuleValues { corner_relief_shape: shape, corner_relief: size, ..alu() };
                let notch = tray_with_level_returns_closes_every_corner(&alu(), &pts, deg, &what);
                let s = tray_with_level_returns_closes_every_corner(&rv, &pts, deg, &what);
                assert_eq!(s.flanges, notch.flanges, "{what}");
                let want: f64 = corners.iter().map(|a| relief_area(shape, size, *a, deg.to_radians() * 2.0)).sum();
                let removed = flat_area(&notch) - flat_area(&s);
                assert!((removed - want).abs() < 1e-6, "{what}: {removed} vs {want}");
            }
        }
    }

    #[test]
    fn returns_at_unequal_angles_meet_on_a_leaning_mitre() {
        // The tray's 90° walls with, in one call, returns at 90° on the walls on y = 0 and y = 90
        // and at 70° (rising 20° toward the middle) on the other two: at every corner two returns
        // meet that don't lie in one plane. Their mitre runs along the mean of the two panels'
        // inner normals, bisects their bend lines seen along it, and passes where their inner
        // start lines cross seen along it.
        let rv = alu();
        let (t, g) = (rv.thickness, rv.gap);
        let (mut s, _) = tray(&rv, 90.0, 20.0);
        let each: Vec<EdgeFlange> =
            (0..4).map(|i| EdgeFlange { pick: far_edge_pick(&s, i, true), height: 12.0, angle: [90f64, 70.0][i % 2].to_radians() }).collect();
        let (made, notes) = s.add_flanges_each(&each, 1.0, "inside", false, ReliefOverride::default()).unwrap();
        assert!(notes.is_empty() && made == [4, 5, 6, 7], "{made:?} {notes:?}");
        for (a, a_end, b, b_end) in TRAY_RETURNS {
            let (fa, fb) = (&s.flanges[a], &s.flanges[b]);
            let (wa, wb) = (s.fold(a), s.fold(b));
            let up = (apply_vector(&wa, Vec3::Z) + apply_vector(&wb, Vec3::Z)).normalized().unwrap();
            let across = |v: Vec3| (v - up * v.dot(up)).normalized().unwrap();
            let ea = across(apply_vector(&wa, v3(if a_end { fa.d * -1.0 } else { fa.d })));
            let eb = across(apply_vector(&wb, v3(if b_end { fb.d * -1.0 } else { fb.d })));
            let m = (ea - eb).normalized().unwrap();
            let pa = apply_point(&wa, v3(fa.p + fa.n * s.allowance(fa)) + Vec3::Z * t);
            let pb = apply_point(&wb, v3(fb.p + fb.n * s.allowance(fb)) + Vec3::Z * t);
            let side = up.cross(ea);
            let o = pb + eb * ((pa - pb).dot(side) / eb.dot(side));
            // Near the corner each return keeps g/2 from that plane, its panel's nearest point at g/2.
            for (i, sign) in [(a, 1.0), (b, -1.0)] {
                let near = |bend: bool| material(&s, i, bend).into_iter().filter(|p| (*p - o).len() < 30.0).collect::<Vec<_>>();
                let (all, panel) = (near(true), near(false));
                assert!(!panel.is_empty() && spread(&all, o, m * sign).0 > g / 2.0 - 1e-9, "{a}/{b}, flange {i}");
                assert!((spread(&panel, o, m * sign).0 - g / 2.0).abs() < 1e-9, "{a}/{b}, flange {i}");
            }
            // The 70° return rises out of the 90° one's plane, so the mitre leans.
            assert!(m.z.abs() > 1e-3, "{m:?}");
        }
        holds_the_flat_material(&s, "returns at 90° and 70°");
    }

    #[test]
    fn returns_turned_outward_close_the_v_between_them() {
        use std::f64::consts::{PI, SQRT_2};
        // The tray's 90° walls with returns turned away from the base (picked on the walls' outer
        // faces): a brim, 12 wide. Left as the walls' ends leave them, the brim's strips stop
        // short of each other in a V at every corner.
        let rv = alu();
        let g = rv.gap;
        let (mut s, _) = tray(&rv, 90.0, 20.0);
        let picks: Vec<Vec3> = (0..4).map(|i| far_edge_pick(&s, i, false)).collect();
        let (_, notes) = s.add_flanges_noted(&picks, 12.0, 90f64.to_radians(), 1.0, "inside", false).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        // By hand, for the strip on the wall on y = 0 (e = g/√2): its bend turns about y = −1,
        // z = 17, and it lies in 18 ≤ z ≤ 20 from y = −1 out to y = −10; its neighbour likewise
        // from x = −1. The inner start lines cross at (−1, −1), so the mitre is x = y again and
        // the strip keeps x − y ≥ e. Its bend spans y from −1 to the wall's inner face y = 2, so
        // it may reach x = 2 + e: that is the wall's end, and the bend keeps the wall's whole far
        // edge, 116 − 2e. The panel at y = −1 − h keeps x ≥ e − 1 − h: it starts 3 before the bend
        // and runs on 1 per unit of height.
        let e = g / SQRT_2;
        let r = &s.flanges[4];
        assert!(r.angle < 0.0 && r.p.dist(Vec2::new(2.0 + e, -11.0 - PI)) < 1e-9 && (r.len - (116.0 - 2.0 * e)).abs() < 1e-9, "{r:?}");
        assert!(
            (r.ext.0 - 3.0).abs() < 1e-9 && (r.ext.1 - 3.0).abs() < 1e-9 && (r.slope.0 - 1.0).abs() < 1e-9 && (r.slope.1 - 1.0).abs() < 1e-9,
            "{r:?}"
        );
        for (a, a_end, b, b_end) in TRAY_RETURNS {
            let ((a0, a1), (b0, b1)) = (inner_end_edge(&s, a, a_end), inner_end_edge(&s, b, b_end));
            assert!((a0.z - 18.0).abs() < 1e-9 && (b1.z - 18.0).abs() < 1e-9, "{a0:?} {b1:?}");
            let gap = parallel_gap(a0, a1, b0, b1);
            assert!(gap.is_some_and(|d| (d - g).abs() < 1e-9), "{a}/{b}: {gap:?}");
        }
        let bb = kernel::measure(&s.solid().unwrap()).unwrap().bbox;
        assert!((bb.min - Vec3::new(-10.0, -10.0, 0.0)).len() < 1e-6 && (bb.max - Vec3::new(130.0, 100.0, 20.0)).len() < 1e-6, "{bb:?}");
        holds_the_flat_material(&s, "a brim");
    }

    #[test]
    fn a_lone_return_follows_its_walls_end_and_keeps_clear_of_the_wall_beside_it() {
        use std::f64::consts::SQRT_2;
        // A wall's closed end is not where its bend ends: at 70° the wall leans out and its end
        // runs on past the corner toward its tip, at 110° it is cut back. A return alone on one
        // wall used to take the bend's length from where the wall's far edge started.
        let rv = alu();
        let (t, r, g) = (rv.thickness, rv.bend_radius, rv.gap);
        let e = g / SQRT_2;
        // By hand, for the wall on y = 0 at θ with a level return (at ρ = 180° − θ) and the wall
        // beside it on x = 0 (`closed_corners_end_on_the_bisector_with_the_gap`): the wall's inner
        // face ends at x(w) = e + T·sinθ − w·cosθ, w up its outer face from the virtual sharp, and
        // the return's setback leaves the wall's far edge at w = 20 − (R + T)·tan(ρ/2). The wall
        // beside it has its inner face on x·sinθ + z·cosθ = T, and the first wall's end keeps
        // e·sinθ from that plane (|nb − na| = √2·sinθ), which a return on it has to keep too
        // with what it has inside the first wall's inner face: x ≥ e + (T − z·cosθ)/sinθ at the
        // height z. Walls that stand upright or lean out ask less of it the higher it is, and
        // nothing more than of the wall's far edge it starts on: the return spans that edge,
        // from x = −3.142098 at 70° (past the corner) and 2.353553 at 90°. Walls that lean in
        // ask most at its top, z = 20·sinθ: at 110° the far edge starts at x = 8.354886 and the
        // return stops square at e + T/sinθ − 20·cosθ = 9.322312.
        for (deg, want) in [(70.0f64, -3.142098), (90.0, 2.353553), (110.0, 9.322312)] {
            let (mut s, _) = tray(&rv, deg, 20.0);
            let bend = s.flanges[0].len;
            let (made, notes) = returns(&mut s, &[0], 180.0 - deg, 12.0);
            assert!(notes.is_empty() && made == [4], "{deg}°: {notes:?}");
            let (th, rho) = (deg.to_radians(), (180.0 - deg).to_radians());
            let edge = e + t * th.sin() - (20.0 - (r + t) * (rho / 2.0).tan()) * th.cos();
            let (lo, hi) = s.far_edge(&s.flanges[0]);
            assert!((s.flanges[0].p.x + lo - edge).abs() < 2e-9 && (hi - lo - (120.0 - 2.0 * edge)).abs() < 2e-9, "{deg}°: {edge} vs {lo} {hi}");
            let ret = &s.flanges[4];
            assert!((ret.p.x - want).abs() < 1e-6 && (ret.len - (120.0 - 2.0 * want)).abs() < 1e-6, "{deg}°: {want} vs {ret:?}");
            assert!((ret.len - bend).abs() > 1.0 && ret.ext == (0.0, 0.0) && ret.slope == (0.0, 0.0), "{deg}°: {bend} vs {ret:?}");
            // Up to 90° that is the wall's far edge exactly; at 110° it is 0.967426 in from it.
            let cut = if deg > 90.0 { 0.967426 } else { 0.0 };
            assert!((ret.p.x - edge - cut).abs() < 1e-6, "{deg}°: {edge} vs {ret:?}");
            returns_keep_out_of_the_walls(&s, &format!("a lone return on a wall at {deg}°"));
            holds_the_flat_material(&s, &format!("a lone return on a wall at {deg}°"));
        }
        // Returns on two walls that don't meet (the tray's bottom and top) are each alone.
        let (mut s, _) = tray(&rv, 110.0, 20.0);
        let (made, notes) = returns(&mut s, &[0, 2], 70.0, 12.0);
        assert!(notes.is_empty() && made == [4, 5], "{notes:?}");
        assert!(s.flanges[4..].iter().all(|f| f.ext == (0.0, 0.0) && f.slope == (0.0, 0.0) && (f.len - 101.355376).abs() < 1e-6));
        returns_keep_out_of_the_walls(&s, "returns on opposite walls");
        holds_the_flat_material(&s, "returns on opposite walls");
        // A return at 90° on the wall at 110° points down into the tray. Its setback is 3, so the
        // wall's far edge is 17 up its outer face and starts at x = e + T·sinθ − 17·cosθ = 8.047281.
        // The return's bend turns about an axis at z = 17·sinθ + 3·cosθ = 14.948714. Its outer
        // face is highest at the turn 70° (3 above the axis), still in the wall's thickness; it
        // crosses the wall's inner face at the turn acos(R/(R + T)) = 70.528779°, at
        // z = 14.948714 + 3·cos 0.528779° = 17.948586, and from there on has to keep
        // x ≥ e + (T − z·cosθ)/sinθ: the bend stops square at x = 9.014660. The panel's outer
        // face starts at z = 20·sinθ + 3·cosθ = 17.767792, where it has to keep x ≥ 8.948856, and
        // has fallen to 20·sinθ + 12·cosθ = 14.689611 at its far edge, where 7.828490 would do:
        // before the wall's end, so the panel's far edge starts with the wall's, at 8.047281.
        // The panel begins 9.014660 − 8.948856 = 0.065804 before its bend's end and gains
        // (8.948856 − 8.047281)/9 = 0.100175 per unit of height, the same at both ends.
        let (mut s, _) = tray(&rv, 110.0, 20.0);
        let (_, notes) = returns(&mut s, &[0], 90.0, 12.0);
        assert!(notes.is_empty(), "{notes:?}");
        let ret = &s.flanges[4];
        assert!((ret.p.x - 9.014660).abs() < 1e-6 && (ret.len - (120.0 - 2.0 * 9.014660)).abs() < 2e-6 && (ret.leg - 9.0).abs() < 1e-12, "{ret:?}");
        for (ext, slope) in [(ret.ext.0, ret.slope.0), (ret.ext.1, ret.slope.1)] {
            assert!((ext - 0.065804).abs() < 1e-6 && (slope - 0.100175).abs() < 1e-6, "{ret:?}");
        }
        let (lo, hi) = s.far_edge(ret);
        assert!((ret.p.x + lo - 8.047281).abs() < 1e-6 && (ret.p.x + hi - (120.0 - 8.047281)).abs() < 1e-6, "{lo} {hi}");
        returns_keep_out_of_the_walls(&s, "a return at 90° on a wall at 110°");
        holds_the_flat_material(&s, "a return at 90° on a wall at 110°");
    }

    #[test]
    fn returns_keep_out_of_the_walls_beside_them_whatever_the_angles() {
        // A wall that leans in over the base also leans in over a return on the wall beside it.
        // Returns alone on one wall, and returns on every wall in one call, level and at 90°, on
        // walls from 70° to 135°: none reaches into a wall (sampled), and none is said to be left open.
        let rv = alu();
        for deg in [70.0f64, 90.0, 100.0, 110.0, 120.0, 135.0] {
            for ret in [180.0 - deg, 90.0] {
                for on in [&[0usize][..], &[0, 1, 2, 3][..]] {
                    let what = format!("walls at {deg}°, returns at {ret}° on {on:?}");
                    let (mut s, _) = tray(&rv, deg, 20.0);
                    let (_, notes) = returns(&mut s, on, ret, 12.0);
                    assert!(notes.is_empty(), "{what}: {notes:?}");
                    returns_keep_out_of_the_walls(&s, &what);
                    holds_the_flat_material(&s, &what);
                }
            }
        }
        // Walls at unequal angles, each with a level return, and walls of unequal heights with
        // returns at 90°: the returns of one feature, mitred toward each other, stay out of the
        // walls as well.
        let cases = [
            ([60.0, 90.0, 105.0, 120.0], [20.0; 4], false),
            ([110.0, 90.0, 110.0, 90.0], [20.0; 4], false),
            ([120.0, 100.0, 70.0, 110.0], [20.0; 4], false),
            ([90.0; 4], [20.0, 30.0, 20.0, 30.0], true),
        ];
        for (angles, heights, square) in cases {
            let what = format!("walls at {angles:?}, {heights:?} high");
            let (mut s, notes) = tray_each(&rv, angles, heights);
            assert!(notes.is_empty(), "{what}: {notes:?}");
            let each: Vec<EdgeFlange> = (0..4)
                .map(|i| {
                    let deg: f64 = if square { 90.0 } else { 180.0 - angles[i] };
                    EdgeFlange { pick: far_edge_pick(&s, i, true), height: 12.0, angle: deg.to_radians() }
                })
                .collect();
            let (_, notes) = s.add_flanges_each(&each, 1.0, "inside", false, ReliefOverride::default()).unwrap();
            assert!(notes.is_empty(), "{what}: {notes:?}");
            returns_keep_out_of_the_walls(&s, &what);
            holds_the_flat_material(&s, &what);
        }
    }

    #[test]
    fn returns_that_cannot_close_stay_open_with_a_note_and_others_are_left_alone() {
        // A gap of zero would make the ends touch: the returns stay as their walls' ends leave
        // them, with a note for each corner that names the point on the wall's far edge.
        let mut rv = alu();
        rv.gap = 0.0;
        let (mut s, _) = tray(&rv, 110.0, 20.0);
        let (_, notes) = returns(&mut s, &[0, 1, 2, 3], 70.0, 12.0);
        assert_eq!(notes.len(), 4, "{notes:?}");
        assert!(notes.iter().all(|n| n.contains("left open") && n.contains("gap")), "{notes:?}");
        // The corner at the origin: the wall on y = 0 starts 3·tan 55° = 4.284 along its edge, and
        // its outer face at its far edge (w = 20 − 3·tan 35° = 17.899 up it) is at
        // (−w·cos 110°, w·sin 110°) = (6.122, 16.820).
        assert!(notes[0].starts_with("the flanges at (4.284, 6.122, 16.820) are left open"), "{notes:?}");
        for i in 4..8 {
            let (lo, hi) = s.far_edge(&s.flanges[i - 4]);
            let r = &s.flanges[i];
            assert!(r.ext == (0.0, 0.0) && r.slope == (0.0, 0.0) && (r.len - (hi - lo)).abs() < 1e-9, "{r:?}");
        }
        // A return turned in next to one turned out: they pass each other, so nothing is closed
        // and nothing is said.
        let (mut s, _) = tray(&alu(), 90.0, 20.0);
        let picks = [far_edge_pick(&s, 0, true), far_edge_pick(&s, 3, false)];
        let (made, notes) = s.add_flanges_noted(&picks, 12.0, 90f64.to_radians(), 1.0, "inside", false).unwrap();
        assert!(notes.is_empty() && made == [4, 5], "{notes:?}");
        // Each spans its wall's far edge, 116 − 2e on y = 0 and 86 − 2e on x = 0, square-ended.
        let e = 0.5 / std::f64::consts::SQRT_2;
        assert!(s.flanges[4..].iter().all(|f| f.ext == (0.0, 0.0) && f.slope == (0.0, 0.0)));
        assert!((s.flanges[4].len - (116.0 - 2.0 * e)).abs() < 1e-9 && (s.flanges[5].len - (86.0 - 2.0 * e)).abs() < 1e-9, "{:?}", &s.flanges[4..]);
        holds_the_flat_material(&s, "one return in, one out");
        // Returns of separate features are not closed toward each other, like flanges on the
        // base: the later one does not reach into the earlier feature, and is only kept clear of
        // the walls beside its own (71.355376 = 90 − 2·9.322312 long, see the lone return). Turned
        // the same way, the two then run through each other at the corner, as before.
        let (mut s, _) = tray(&alu(), 110.0, 20.0);
        let (first, notes) = returns(&mut s, &[0], 70.0, 12.0);
        assert!(notes.is_empty() && first == [4], "{notes:?}");
        let kept = s.flanges[4].clone();
        let (second, notes) = returns(&mut s, &[1], 70.0, 12.0);
        assert!(notes.is_empty() && second == [5], "{notes:?}");
        let later = &s.flanges[5];
        assert!(s.flanges[4] == kept && later.ext == (0.0, 0.0) && later.slope == (0.0, 0.0) && (later.len - 71.355376).abs() < 1e-6, "{later:?}");
        returns_keep_out_of_the_walls(&s, "returns of separate features");
    }

    #[test]
    fn returns_close_across_a_cut_off_corner_but_not_across_an_edge_of_the_base() {
        use std::f64::consts::SQRT_2;
        let rv = alu();
        let e = rv.gap / SQRT_2;
        // The tray with its corner at the origin cut off from (10, 0) to (0, 10), and no flange on
        // that cut: the walls on y = 0 and x = 0 don't share a corner point, but their bend lines
        // y = 3 and x = 3 cross at (3, 3) and both bends stop 4 short of it, at (7, 3) and (3, 7).
        // Their returns would run through each other over the cut, so they close as at a whole
        // corner, on x = y through (3, 3): each return's bend keeps its wall's end (x = 7 is past
        // 3 + e) and its panel runs on from 3 + e, 4 − e before the bend, losing 1 per unit of
        // height. At the other ends, whole corners, the bends stop 1 in from the walls' ends at
        // 117 − e and 87 − e.
        let cut = [Vec2::new(10.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0), Vec2::new(0.0, 10.0)];
        let mut s = walls(&rv, &cut, &[0, 1, 2, 3], 90.0, 20.0);
        assert!(s.flanges[0].p.dist(Vec2::new(7.0, 3.0)) < 1e-9 && s.flanges[0].ext.0 == 0.0 && (s.flanges[3].len - 80.0).abs() < 1e-9);
        let (_, notes) = returns(&mut s, &[0, 1, 2, 3], 90.0, 12.0);
        assert!(notes.is_empty(), "{notes:?}");
        let (bottom, left) = (&s.flanges[4], &s.flanges[7]);
        assert!((bottom.p.x - 7.0).abs() < 1e-9 && (bottom.len - (110.0 - e)).abs() < 1e-9, "{bottom:?}");
        assert!((bottom.ext.0 - (4.0 - e)).abs() < 1e-9 && (bottom.slope.0 + 1.0).abs() < 1e-9 && bottom.ext.1.abs() < 1e-9, "{bottom:?}");
        assert!((left.len - (80.0 - e)).abs() < 1e-9 && (left.ext.1 - (4.0 - e)).abs() < 1e-9 && (left.slope.1 + 1.0).abs() < 1e-9, "{left:?}");
        for (a, a_end, b, b_end) in TRAY_RETURNS {
            let ((a0, a1), (b0, b1)) = (inner_end_edge(&s, a, a_end), inner_end_edge(&s, b, b_end));
            let gap = parallel_gap(a0, a1, b0, b1);
            assert!(gap.is_some_and(|d| (d - rv.gap).abs() < 1e-9), "{a}/{b}: {gap:?}");
        }
        holds_the_flat_material(&s, "a cut-off corner");
        // The same cut, 16 wide, with a wall of its own: now it is an edge of the base. The walls
        // on y = 0 and x = 0 still stop the same distance short of where their bend lines cross,
        // but the cut's bend ends between them, so each closes with the wall on the cut (two
        // corners of 135°) and not with the other. `tray_with_level_returns_closes_every_corner`
        // checks every return against the corners of its own edge.
        let walled = [Vec2::new(16.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0), Vec2::new(0.0, 16.0)];
        let s = tray_with_level_returns_closes_every_corner(&rv, &walled, 90.0, "a walled cut");
        assert!(s.flanges[5].ext.0.abs() < 1e-9 && (s.flanges[5].slope.0 + (SQRT_2 - 1.0)).abs() < 1e-9, "{:?}", s.flanges[5]);
        // With returns on those two walls only, the cut's wall stands between them: they don't
        // close with each other through it. Each is alone at both its corners and spans its wall's
        // far edge, square-ended: on the edges of 104 and 74, from 2 + e short of the whole corner
        // to (g/2 + 2·cos 67.5°)/sin 67.5° = 1.099025 short of the corner of 135°.
        let mut s = walls(&rv, &walled, &[0, 1, 2, 3, 4], 90.0, 20.0);
        let (made, notes) = returns(&mut s, &[0, 3], 90.0, 12.0);
        assert!(notes.is_empty() && made == [5, 6], "{made:?} {notes:?}");
        let half = 67.5f64.to_radians();
        let short = (rv.gap / 2.0 + 2.0 * half.cos()) / half.sin();
        for (i, edge) in [(5, 104.0), (6, 74.0)] {
            let f = &s.flanges[i];
            assert!(f.ext == (0.0, 0.0) && f.slope == (0.0, 0.0) && (f.len - (edge - (2.0 + e) - short)).abs() < 1e-9, "{f:?}");
        }
        returns_keep_out_of_the_walls(&s, "beside a walled cut");
        holds_the_flat_material(&s, "beside a walled cut");
        // A trapezoid with walls on its bottom and its two sides only. The sides' bend lines cross
        // above the open top edge, both bends stop 26.04 short of that point, and nothing lies
        // between them: but that is further than the walls reach (π + 14 past their bends), so the
        // top is an edge of the base, and the sides' returns end with their walls there.
        let open = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(75.0, 90.0), Vec2::new(45.0, 90.0)];
        let mut s = walls(&rv, &open, &[0, 1, 3], 90.0, 20.0);
        let (_, notes) = returns(&mut s, &[0, 1, 2], 90.0, 12.0);
        assert!(notes.is_empty(), "{notes:?}");
        // Flanges 0…2 are the walls on the bottom, the right side and the left side, 3…5 their
        // returns: the right side's ends at the top with its far end, the left side's with its start.
        let (right, left) = (&s.flanges[4], &s.flanges[5]);
        assert!(right.ext.1 == 0.0 && right.slope.1 == 0.0 && left.ext.0 == 0.0 && left.slope.0 == 0.0, "{right:?} {left:?}");
        assert!(right.slope.0 < -0.1 && left.slope.1 < -0.1 && (right.len - left.len).abs() < 1e-9, "{right:?} {left:?}");
        holds_the_flat_material(&s, "an open top");
    }

    /// The angles of the sweep below.
    const SWEEP: [f64; 7] = [45.0, 60.0, 75.0, 90.0, 105.0, 120.0, 135.0];

    #[test]
    fn every_pair_of_angles_closes_at_any_base_corner() {
        // A parallelogram base (120 and 90 long) with a corner of 60°, 90° or 120° at the origin;
        // flanges 38.6 high on its two edges there, in one call, at every pair of the angles: 147
        // corners, each closed with the gap, the flanges clear of each other, the part one valid
        // shell holding the flat pattern's material.
        let rv = alu();
        let mut cases = 0;
        for alpha in [60.0f64, 90.0, 120.0] {
            let u = Vec2::new(alpha.to_radians().cos(), alpha.to_radians().sin());
            let pts = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 0.0) + u * 90.0, u * 90.0];
            for a in SWEEP {
                for b in SWEEP {
                    let what = format!("corner {alpha}°, flanges {a}° and {b}°");
                    let each = [
                        EdgeFlange { pick: Vec3::new(60.0, 0.0, 2.0), height: 38.6, angle: a.to_radians() },
                        EdgeFlange { pick: Vec3::new(u.x * 45.0, u.y * 45.0, 2.0), height: 38.6, angle: b.to_radians() },
                    ];
                    let (s, notes) = flanges_each(Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] }, &rv, &each);
                    assert!(notes.is_empty(), "{what}: {notes:?}");
                    assert!((s.flanges[0].angle - a.to_radians()).abs() < 1e-12 && (s.flanges[1].angle - b.to_radians()).abs() < 1e-12, "{what}");
                    let corners = s.corners(&[0, 1]);
                    assert_eq!(corners.len(), 1, "{what}");
                    ends_are_gap_apart(&s, &corners, rv.gap, &what);
                    flanges_keep_clear(&s, &corners[0], &what);
                    holds_the_flat_material(&s, &what);
                    cases += 1;
                }
            }
        }
        assert_eq!(cases, 147);
    }

    #[test]
    fn corner_reliefs_are_cut_between_flanges_of_unequal_angle_and_height() {
        use std::f64::consts::FRAC_PI_2;
        // The relief is cut in the base, so flanges with their own angle and height get the same
        // one: 4 mm into the base at each of its four corners, and every bend 4 mm shorter at both
        // ends, each over its own width BA = θ·(R + K·T).
        let heights = [38.6, 30.0, 45.0, 25.0];
        let (notch, _) = tray_each(&alu(), MIXED, heights);
        for shape in [ReliefShape::Round, ReliefShape::Rectangular] {
            let what = format!("{shape:?}");
            let (s, notes) = tray_each(&RuleValues { corner_relief_shape: shape, ..alu() }, MIXED, heights);
            assert!(notes.is_empty(), "{what}: {notes:?}");
            assert_eq!(s.flanges, notch.flanges, "{what}");
            // The bend on y = 0 runs between the setbacks of its neighbours at 120° and 90°:
            // 120 − 3·tan 60° − 3·tan 45° long, less the two reliefs.
            let (bends, before) = (s.bends(), notch.bends());
            assert_eq!((bends.len(), before.len()), (4, 4), "{what}");
            assert!((bends[0].length - (120.0 - 3.0 * 60f64.to_radians().tan() - 3.0 - 8.0)).abs() < 1e-9, "{what}: {}", bends[0].length);
            for (b, b0) in bends.iter().zip(&before) {
                assert!((b.length - (b0.length - 8.0)).abs() < 1e-9, "{what}: {} vs {}", b.length, b0.length);
            }
            let in_the_bends: f64 = MIXED.iter().map(|deg| 8.0 * deg.to_radians() * 2.0).sum();
            let (removed, want) = (flat_area(&notch) - flat_area(&s), 4.0 * relief_area(shape, 8.0, FRAC_PI_2, 0.0) + in_the_bends);
            assert!((removed - want).abs() < 1e-6, "{what}: {removed} vs {want}");
            holds_the_flat_material(&s, &what);
        }
    }

    /// `alu()` with torn corners.
    fn torn() -> RuleValues {
        RuleValues { corner_relief_shape: ReliefShape::Tear, ..alu() }
    }

    /// The corners of the flat pattern's outline.
    fn outline_points(s: &SheetBody) -> Vec<Vec2> {
        s.flat_regions().unwrap().iter().flat_map(|r| r.outer.segs.iter().map(Seg2::start)).collect()
    }

    /// A tear leaves the base and the flanges as the closing made them (`notch`): it only marks
    /// the bend ends at the corners as torn. `torn`: how many of the ends are.
    fn tears_nothing_but_the_bend_ends(s: &SheetBody, notch: &SheetBody, torn: usize, what: &str) {
        assert_eq!(s.base, notch.base, "{what}");
        assert_eq!(s.flanges.iter().map(|f| f.tear.0 as usize + f.tear.1 as usize).sum::<usize>(), torn, "{what}");
        let whole: Vec<SheetFlange> = s.flanges.iter().map(|f| SheetFlange { tear: (false, false), ..f.clone() }).collect();
        assert_eq!(whole, notch.flanges, "{what}");
    }

    #[test]
    fn a_tear_cuts_each_bend_from_the_corner_to_its_flanges_end() {
        use std::f64::consts::SQRT_2;
        // The tray under `alu()`: T = 2, R = 1, gap g = 0.5, flanges h = 38.6 high. The bends on y = 0
        // and x = 0 start on the lines y = s0 and x = s0, s0 = (R + T)·tan(θ/2), which meet at
        // c = (s0, s0), and are BA = θ·(R + T/2) wide. The flange on y = 0 is closed to
        // x_end(w) = g/√2 + T·sinθ − w·cosθ (`closed_corners_end_on_the_bisector_with_the_gap`): w = s0
        // where its panel starts, at flat y = s0 − BA, and w = h at its far edge. A tear cuts from c
        // straight to (x_end(s0), s0 − BA), where the panel's end goes on; the notch's step at
        // (s0, s0 − BA) is gone. The flange on x = 0 is the mirror image in x = y.
        let (t, r, g, h) = (2.0, 1.0, 0.5, 38.6);
        for deg in [90.0f64, 72.0] {
            let what = format!("{deg}°");
            let (notch, _) = tray(&alu(), deg, h);
            let (s, notes) = tray(&torn(), deg, h);
            assert!(notes.is_empty(), "{what}: {notes:?}");
            tears_nothing_but_the_bend_ends(&s, &notch, 8, &what);
            let th = deg.to_radians();
            let (s0, ba) = ((r + t) * (th / 2.0).tan(), th * (r + 0.5 * t));
            let x_end = |w: f64| g / SQRT_2 + t * th.sin() - w * th.cos();
            let (y0, y1) = (s0 - ba, s0 - ba - (h - s0));
            // The bend on y = 0: its start line between the corners, its far edge as long as the panel's.
            let zone = corners_of(&s.zone(&s.flanges[0]));
            assert_eq!(zone.len(), 4, "{what}");
            for (x, y) in [(s0, s0), (120.0 - s0, s0), (120.0 - x_end(s0), y0), (x_end(s0), y0)] {
                assert!(zone.iter().any(|p| p.dist(Vec2::new(x, y)) < 1e-9), "{what}: ({x}, {y}) not in {zone:?}");
            }
            // The outline at the corner: the panel's end, the cut, the corner of the bends, and on.
            let (cut, stepped) = (outline_points(&s), outline_points(&notch));
            let at = |pts: &[Vec2], x: f64, y: f64| pts.iter().any(|p| p.dist(Vec2::new(x, y)) < 1e-9);
            for (x, y) in [(x_end(h), y1), (x_end(s0), y0), (s0, s0)] {
                assert!(at(&cut, x, y) && at(&cut, y, x), "{what}: ({x}, {y}) not in the outline");
            }
            assert!(at(&stepped, s0, y0) && at(&stepped, y0, s0) && !at(&cut, s0, y0) && !at(&cut, y0, s0), "{what}");
            // Each of the eight bend ends gains the triangle between its square end and the cut.
            let grow = s0 - x_end(s0);
            let gained = flat_area(&s) - flat_area(&notch);
            assert!(grow > 0.0 && (gained - 8.0 * ba * grow / 2.0).abs() < 1e-6, "{what}: {gained}");
            // The bend table: the centre line runs between the middles of the bend's two ends.
            let bend = &s.bends()[0];
            let (start, end) = (Vec2::new(s0 - grow / 2.0, s0 - ba / 2.0), Vec2::new(120.0 - s0 + grow / 2.0, s0 - ba / 2.0));
            assert!(
                (bend.length - (120.0 - 2.0 * s0 + grow)).abs() < 1e-9 && bend.start.dist(start) < 1e-9 && bend.end.dist(end) < 1e-9,
                "{what}: {bend:?}"
            );
            // Folded, the panels' ends are the gap apart as they were. A point of a bend's inner face
            // is R·sin φ out from the bend's start line at the turn φ, and the torn end is there
            // grow·φ/θ past the corner along the bend, so the two ends at this corner (flange 0's
            // and its mirror image, flange 3's) are √2·(R·sin φ − grow·φ/θ) apart: they touch at the
            // corner and are the gap apart where the panels start (grow = R·sinθ − g/√2).
            let corners = s.corners(&[0, 1, 2, 3]);
            assert_eq!(corners.len(), 4, "{what}");
            ends_are_gap_apart(&s, &corners, g, &what);
            let (c, far) = (Vec2::new(s0, s0), Vec2::new(x_end(s0), y0));
            for i in 0..=64 {
                let x = i as f64 / 64.0;
                let q = c + (far - c) * x;
                let apart = s.map3(Piece::Zone(0), q, t).dist(s.map3(Piece::Zone(3), Vec2::new(q.y, q.x), t));
                let want = SQRT_2 * (r * (th * x).sin() - grow * x);
                assert!((apart - want).abs() < 1e-9 && apart > g * x - 1e-9, "{what}: {x}: {apart} vs {want}");
                assert!(s.bend_end(&s.flanges[0], false, th * x, t).dist(s.map3(Piece::Zone(0), q, t)) < 1e-9, "{what}: {x}");
            }
            for k in &corners {
                flanges_keep_clear(&s, k, &what);
            }
            holds_the_flat_material(&s, &what);
            let unfolded = vol(&s.flat_solid().unwrap());
            assert!((unfolded - flat_area(&s) * t).abs() < 1e-3 * unfolded, "{what}: {unfolded}");
        }
    }

    #[test]
    fn a_tear_follows_corners_that_are_not_square() {
        // The trapezoid of the relief tests with 72° flanges: the base fills α = atan(90/20) = 77.47°
        // at its lower corners and β = 180° − α at the upper ones. Seen from above, a point u along a
        // bend from the corner of the bends and v out from the bend's start line is
        // u·sin(α/2) + v·cos(α/2) from the plane that halves the corner, which also halves the inner
        // faces of two flanges at one angle. A panel's inner face starts v = R·sinθ out and keeps
        // g/2 from that plane, so its end starts at u = (g/2 − R·sinθ·cos(α/2)) / sin(α/2): less
        // than nothing, the panel runs on past the corner, and the tear cuts from the corner to there.
        let (r, g) = (1.0, 0.5);
        let th = 72f64.to_radians();
        let (s0, ba) = (3.0 * (th / 2.0).tan(), th * 2.0);
        let (alpha, beta) = (90f64.atan2(20.0), std::f64::consts::PI - 90f64.atan2(20.0));
        let u_end = |a: f64| (g / 2.0 - r * th.sin() * (a / 2.0).cos()) / (a / 2.0).sin();
        let (notch, _) = trapezoid(&alu(), 72.0);
        let (s, notes) = trapezoid(&torn(), 72.0);
        assert!(notes.is_empty(), "{notes:?}");
        tears_nothing_but_the_bend_ends(&s, &notch, 8, "trapezoid");
        let (cut, stepped) = (outline_points(&s), outline_points(&notch));
        let at = |pts: &[Vec2], q: Vec2| pts.iter().any(|p| p.dist(q) < 1e-8);
        // The bend on y = 0 (flange 0) between the lower corners, and the one on y = 90 (flange 2)
        // between the upper ones: the corner of the bends, where the cut ends, and the notch's step.
        let lower = (Vec2::new(s0 / (alpha / 2.0).tan(), s0), Vec2::new(u_end(alpha), -ba));
        let upper = (Vec2::new(20.0 + s0 / (beta / 2.0).tan(), 90.0 - s0), Vec2::new(u_end(beta), ba));
        for (i, (c, to)) in [(0, lower), (2, upper)] {
            let zone = corners_of(&s.zone(&s.flanges[i]));
            let mirror = |q: Vec2| Vec2::new(120.0 - q.x, q.y);
            for q in [c, c + to, mirror(c), mirror(c + to)] {
                assert!(at(&zone, q) && at(&cut, q), "flange {i}: {q:?} not in {zone:?}");
            }
            let step = Vec2::new(c.x, c.y + to.y);
            assert!(at(&stepped, step) && !at(&cut, step), "flange {i}");
        }
        // Each bend end gains the triangle between its square end and the cut: four ends at α, four at β.
        let (gained, want) = (flat_area(&s) - flat_area(&notch), -2.0 * ba * (u_end(alpha) + u_end(beta)));
        assert!((gained - want).abs() < 1e-6, "{gained} vs {want}");
        let corners = s.corners(&[0, 1, 2, 3]);
        assert_eq!(corners.len(), 4);
        ends_are_gap_apart(&s, &corners, g, "trapezoid");
        for k in &corners {
            flanges_keep_clear(&s, k, "trapezoid");
        }
        holds_the_flat_material(&s, "trapezoid");
    }

    #[test]
    fn a_cut_out_over_torn_bend_ends_folds() {
        use std::f64::consts::{FRAC_PI_2, PI};
        // A round cut-out of radius 2 about the corner (s0, s0) of the tray's bends takes the two
        // torn bend ends with it. Of its disc the flat pattern holds all but the wedge between the
        // two cuts, which leave the corner atan(grow/BA) off the lines square to the bends.
        for deg in [90.0f64, 72.0] {
            let what = format!("{deg}°");
            let th = deg.to_radians();
            let (s0, ba, grow) = (3.0 * (th / 2.0).tan(), th * 2.0, th.sin() - 0.25 * 2f64.sqrt());
            let (whole, _) = tray(&torn(), deg, 38.6);
            let mut s = whole.clone();
            s.holes.push(Region2 { outer: Loop2::circle(Vec2::new(s0, s0), 2.0), holes: vec![] });
            let (removed, want) = (flat_area(&whole) - flat_area(&s), 4.0 * (PI - (FRAC_PI_2 - 2.0 * (grow / ba).atan()) / 2.0));
            assert!((removed - want).abs() < 1e-6, "{what}: {removed} vs {want}");
            let body = s.solid().unwrap_or_else(|e| panic!("{what}: no solid: {e}"));
            assert!(body.validity().is_empty() && body.shell_count() == 1, "{what}: {:?}", body.validity());
            let lost = vol(&whole.solid().unwrap()) - vol(&body);
            assert!((lost - want * 2.0).abs() < 0.05, "{what}: {lost} vs {}", want * 2.0);
        }
    }

    /// The plate (0, 0) (100, 0) (85, 20) (0, 20) with a 30 mm inside flange on its slanted edge
    /// (2 mm sheet, R 2, K 0.5: the folded sheet holds its flat area times the thickness), and a
    /// triangle cut out of the flat with one corner on the bend line: where the line meets the
    /// plate's edge, at either end, the triangle on the bend or on the base; in the line's middle,
    /// from either side; and on the plate's straight edge, from inside. At the plate's edge the
    /// sheet is pinched (two wedges of material meet along the thickness); in the middle the
    /// triangle's tip touches the next piece. The refold ran the face through that corner twice
    /// ("This wire is not simple").
    #[test]
    fn a_cut_out_with_a_corner_on_a_bend_line_or_the_sheets_edge_refolds() {
        let rule = RuleValues { bend_radius: 2.0, ..alu() };
        let outline = [Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(85.0, 20.0), Vec2::new(0.0, 20.0)];
        for deg in [90.0f64, 120.0] {
            let mut s = base_flange("Body1", "AL2", &rule, Plane::XY, Region2 { outer: Loop2::polygon(&outline).ccw(), holes: vec![] });
            s.add_flange(Vec3::new(92.5, 10.0, 2.0), 30.0, deg.to_radians(), 2.0, "inside", false).unwrap();
            let (a0, v0) = (flat_area(&s), vol(&s.solid().unwrap()));
            let f = s.flanges[0].clone();
            let (start, end, middle) = (f.p, f.p + f.d * f.len, f.p + f.d * (f.len / 2.0));
            // The bend line runs from the plate's edge y = 0 to y = 20, the bend on its +n side,
            // square across at its ends; the base's corner at either end is at least 53.13°
            // wide, the edge running off at (0.6, -0.8) along and across the line. The triangles'
            // other corners as steps along the line and across it.
            let cases = [
                ("on the bend at the line's start", start, [(2.0, 1.0), (0.4, 2.2)], 2.0),
                ("on the bend at the line's end", end, [(-2.0, 1.0), (-0.4, 2.2)], 2.0),
                ("on the base at the line's start", start, [(4.0, -1.0), (2.0, -2.0)], 3.0),
                ("on the base at the line's middle", middle, [(-1.0, -2.0), (1.0, -2.0)], 2.0),
                ("on the bend at the line's middle", middle, [(-1.0, 2.0), (1.0, 2.0)], 2.0),
            ];
            for (what, c, others, area) in cases {
                let what = format!("{deg}°, {what}");
                let corners: Vec<Vec2> = std::iter::once(c).chain(others.iter().map(|(u, v)| c + f.d * *u + f.n * *v)).collect();
                let mut cut = s.clone();
                cut.holes.push(Region2 { outer: Loop2::polygon(&corners).ccw(), holes: vec![] });
                assert!((a0 - flat_area(&cut) - area).abs() < 1e-6, "{what}: flat area {}", a0 - flat_area(&cut));
                let body = cut.solid().unwrap_or_else(|e| panic!("{what}: {e}"));
                assert!(body.validity().is_empty() && body.shell_count() == 1, "{what}: {:?}", body.validity());
                assert!((v0 - vol(&body) - area * 2.0).abs() < 0.01, "{what}: {} vs {}", v0 - vol(&body), area * 2.0);
            }
            // A 4 mm² triangle standing on the plate's edge y = 20 at x = 40.
            let mut cut = s.clone();
            cut.holes
                .push(Region2 { outer: Loop2::polygon(&[Vec2::new(40.0, 20.0), Vec2::new(38.0, 18.0), Vec2::new(42.0, 18.0)]).ccw(), holes: vec![] });
            assert!((a0 - flat_area(&cut) - 4.0).abs() < 1e-6, "{deg}°, on the edge: flat area {}", a0 - flat_area(&cut));
            let body = cut.solid().unwrap_or_else(|e| panic!("{deg}°, on the edge: {e}"));
            assert!(body.validity().is_empty() && body.shell_count() == 1, "{deg}°, on the edge: {:?}", body.validity());
            assert!((v0 - vol(&body) - 8.0).abs() < 0.01, "{deg}°, on the edge: {} vs 8", v0 - vol(&body));
        }
    }

    /// The plate (0, 0) (100, 0) (85, 20) (0, 20) with a 30 mm inside flange on its slanted edge
    /// (2 mm sheet, R 2, K 0.5: the folded sheet holds its flat area times the thickness), and a
    /// triangle cut out of the flat on one side of the bend line only, with a corner a hair (0.01
    /// or 0.05 µm) from the line, short of it or across: on the base, in the line's middle and
    /// near its ends; and at either end of the line, where it meets the plate's edge, on the base
    /// and on the bend. The planar booleans leave such a corner as corners a hair apart, on the
    /// base and on the bend; joined, they are a corner where the cut-out touches the bend line or
    /// the plate's edge. So the refold needs both: without the joining it failed with "sew: a
    /// degenerate edge", without the touching sew with "This wire is not simple".
    #[test]
    fn a_cut_out_on_one_side_of_a_bend_line_with_a_corner_a_hair_from_it_refolds() {
        let rule = RuleValues { bend_radius: 2.0, ..alu() };
        let outline = [Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(85.0, 20.0), Vec2::new(0.0, 20.0)];
        for deg in [90.0f64, 120.0] {
            let mut s = base_flange("Body1", "AL2", &rule, Plane::XY, Region2 { outer: Loop2::polygon(&outline).ccw(), holes: vec![] });
            s.add_flange(Vec3::new(92.5, 10.0, 2.0), 30.0, deg.to_radians(), 2.0, "inside", false).unwrap();
            let (a0, v0) = (flat_area(&s), vol(&s.solid().unwrap()));
            let f = s.flanges[0].clone();
            // The corner at a fraction of the line, the triangle's other corners as steps along the
            // line and across it (the bend on its +n side; the base's corner at either end of the
            // line is at least 53.13° wide), and the triangle's area.
            let cases = [
                ("on the base in the line's middle", 0.5, [(-1.0, -2.0), (1.0, -2.0)], 2.0),
                ("on the base near the line's start", 0.16, [(-1.0, -2.0), (1.0, -2.0)], 2.0),
                ("on the base near the line's end", 0.84, [(-1.0, -2.0), (1.0, -2.0)], 2.0),
                ("on the base at the line's start", 0.0, [(3.0, -2.0), (2.0, -2.0)], 1.0),
                ("on the base at the line's end", 1.0, [(-2.0, -2.0), (-3.0, -2.0)], 1.0),
                ("on the bend at the line's start", 0.0, [(1.25, 2.0), (0.25, 2.0)], 1.0),
                ("on the bend at the line's end", 1.0, [(-0.25, 2.0), (-1.25, 2.0)], 1.0),
            ];
            for (what, along, others, area) in cases {
                // Across the line toward the bend.
                for off in [-5e-8, -1e-8, 1e-8, 5e-8] {
                    let what = format!("{deg}°, {what}, {off} mm across the line");
                    let c = f.p + f.d * (f.len * along) + f.n * off;
                    let corners: Vec<Vec2> = std::iter::once(c).chain(others.iter().map(|(u, v)| c + f.d * *u + f.n * *v)).collect();
                    let mut cut = s.clone();
                    cut.holes.push(Region2 { outer: Loop2::polygon(&corners).ccw(), holes: vec![] });
                    assert!((a0 - flat_area(&cut) - area).abs() < 1e-5, "{what}: flat area {}", a0 - flat_area(&cut));
                    let body = cut.solid().unwrap_or_else(|e| panic!("{what}: {e}"));
                    assert!(body.validity().is_empty() && body.shell_count() == 1, "{what}: {:?}", body.validity());
                    assert!((v0 - vol(&body) - area * 2.0).abs() < 0.01, "{what}: {} vs {}", v0 - vol(&body), area * 2.0);
                }
            }
        }
    }

    #[test]
    fn a_tear_holds_at_every_angle() {
        use std::f64::consts::{FRAC_PI_2, PI};
        // Every 5° from 45° to 135° on the tray and on the trapezoid: all four corners are torn, the
        // flanges are the ones the notch gives, and each of the eight bend ends gains the triangle
        // between its square end and the cut, BA·(−u_end)/2 with u_end as in
        // `a_tear_follows_corners_that_are_not_square`. The flanges' ends stay the gap apart, no
        // bend runs into the other flange, and the folded part holds the flat pattern's material.
        let alpha = 90f64.atan2(20.0);
        for step in 9..=27 {
            let deg = step as f64 * 5.0;
            let th = deg.to_radians();
            let u_end = |a: f64| (0.25 - th.sin() * (a / 2.0).cos()) / (a / 2.0).sin();
            for (name, (s, notes), (notch, _), angles) in [
                ("tray", tray(&torn(), deg, 38.6), tray(&alu(), deg, 38.6), [FRAC_PI_2; 4]),
                ("trapezoid", trapezoid(&torn(), deg), trapezoid(&alu(), deg), [alpha, alpha, PI - alpha, PI - alpha]),
            ] {
                let what = format!("a tear on the {name} at {deg}°");
                assert!(notes.is_empty(), "{what}: {notes:?}");
                tears_nothing_but_the_bend_ends(&s, &notch, 8, &what);
                let want: f64 = angles.iter().map(|a| -(th * 2.0) * u_end(*a)).sum();
                let gained = flat_area(&s) - flat_area(&notch);
                assert!((gained - want).abs() < 1e-6, "{what}: {gained} vs {want}");
                let corners = s.corners(&[0, 1, 2, 3]);
                assert_eq!(corners.len(), 4, "{what}");
                ends_are_gap_apart(&s, &corners, 0.5, &what);
                for k in &corners {
                    flanges_keep_clear(&s, k, &what);
                }
                holds_the_flat_material(&s, &what);
            }
        }
    }

    #[test]
    fn flanges_of_unequal_angle_are_torn_where_their_bends_stay_apart() {
        // The tray with flanges at 60°, 90°, 105° and 120°, each its own height. Three corners are
        // torn: each bend's start line runs between the corners of the bend lines, and its far edge
        // to the panel's end of `hand_panel` where the end is torn. At the fourth corner the flanges
        // at 120° and 60° lean 60° apart, and the plane halving them leans with them: a cut to the
        // ends it gives would run one bend across the corner into the other, so that corner keeps
        // its notch, with a note.
        let rv = torn();
        let heights = [38.6, 30.0, 45.0, 25.0];
        let (notch, _) = tray_each(&alu(), MIXED, heights);
        let (s, notes) = tray_each(&rv, MIXED, heights);
        let s0 = MIXED.map(|a| 3.0 * (a.to_radians() / 2.0).tan());
        let ba = MIXED.map(|a| a.to_radians() * 2.0);
        let refused = format!("the corner at ({:.3}, {:.3}, 0.000) keeps its notch: a tear would run a bend into the other one", s0[3], s0[0]);
        assert_eq!(notes, [refused]);
        tears_nothing_but_the_bend_ends(&s, &notch, 6, "four angles");
        assert_eq!(s.flanges.iter().map(|f| f.tear).collect::<Vec<_>>(), [(false, true), (true, true), (true, true), (true, false)]);
        // The corners of the bend lines, counter-clockwise from the one at the origin, and the way
        // out of the base across each bend.
        let c = [Vec2::new(s0[3], s0[0]), Vec2::new(120.0 - s0[1], s0[0]), Vec2::new(120.0 - s0[1], 90.0 - s0[2]), Vec2::new(s0[3], 90.0 - s0[2])];
        let out = [Vec2::new(0.0, -1.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0)];
        let mut gain = 0.0;
        for i in 0..4 {
            let (f, panel) = (&s.flanges[i], hand_panel(&rv, MIXED, heights, i));
            let (a, b) = (c[i], c[(i + 1) % 4]);
            let (square_a, square_b) = (a + out[i] * ba[i], b + out[i] * ba[i]);
            let (far_a, far_b) = (if f.tear.0 { panel[0] } else { square_a }, if f.tear.1 { panel[3] } else { square_b });
            let zone = corners_of(&s.zone(f));
            assert_eq!(zone.len(), 4, "flange {i}");
            for q in [a, b, far_b, far_a] {
                assert!(zone.iter().any(|p| p.dist(q) < 1e-9), "flange {i}: {q:?} not in {zone:?}");
            }
            // A torn end gains the triangle between the bend's square end and the cut.
            let along = (b - a).normalized().unwrap();
            gain += ba[i] / 2.0 * ((square_a - far_a).dot(along) + (far_b - square_b).dot(along));
        }
        let (area, want) = (flat_area(&s), hand_flat_area(&rv, MIXED, heights) + gain);
        assert!(gain > 0.0 && (area - want).abs() < 1e-6, "flat area {area} vs {want}");
        let corners = s.corners(&[0, 1, 2, 3]);
        assert_eq!(corners.len(), 4);
        ends_are_gap_apart(&s, &corners, rv.gap, "four angles");
        for k in &corners {
            flanges_keep_clear(&s, k, "four angles");
        }
        holds_the_flat_material(&s, "four angles");
    }

    #[test]
    fn every_pair_of_angles_is_torn_or_keeps_its_notch() {
        // The corners of `every_pair_of_angles_closes_at_any_base_corner` under the tear. Flanges at
        // one angle are always torn. Where the angles differ, the plane that halves the flanges
        // leans, and where their ends start moves with it: the corner is torn while both bends' ends
        // stay on their own side of the corner and open toward the flanges' ends, and else keeps
        // its notch with a note. That is a cautious rule: at the 60° and the 90° corner it tears
        // angles up to 45° apart, at the 120° corner 27 of the 49 pairs. Torn, the flanges are the
        // closed ones, no bend lies in the other flange, and the part is one valid shell.
        let mut torn_pairs = Vec::new();
        for alpha in [60.0f64, 90.0, 120.0] {
            let u = Vec2::new(alpha.to_radians().cos(), alpha.to_radians().sin());
            let pts = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 0.0) + u * 90.0, u * 90.0];
            let mut count = 0;
            for a in SWEEP {
                for b in SWEEP {
                    let what = format!("corner {alpha}°, flanges {a}° and {b}°");
                    let each = [
                        EdgeFlange { pick: Vec3::new(60.0, 0.0, 2.0), height: 38.6, angle: a.to_radians() },
                        EdgeFlange { pick: Vec3::new(u.x * 45.0, u.y * 45.0, 2.0), height: 38.6, angle: b.to_radians() },
                    ];
                    let base = || Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] };
                    let (notch, _) = flanges_each(base(), &alu(), &each);
                    let (s, notes) = flanges_each(base(), &torn(), &each);
                    if !notes.is_empty() {
                        assert!(notes.len() == 1 && notes[0].contains(") keeps its notch: a tear would"), "{what}: {notes:?}");
                        assert!(a != b && s.flanges == notch.flanges, "{what}: {notes:?}");
                        continue;
                    }
                    count += 1;
                    tears_nothing_but_the_bend_ends(&s, &notch, 2, &what);
                    let corners = s.corners(&[0, 1]);
                    assert_eq!(corners.len(), 1, "{what}");
                    ends_are_gap_apart(&s, &corners, 0.5, &what);
                    flanges_keep_clear(&s, &corners[0], &what);
                    holds_the_flat_material(&s, &what);
                }
            }
            torn_pairs.push(count);
        }
        assert_eq!(torn_pairs, [37, 37, 27]);
    }

    #[test]
    fn returns_close_at_torn_corners_as_at_whole_ones() {
        use std::f64::consts::{FRAC_PI_2, PI};
        // Like a relief, a tear leaves the walls' closed ends as they are, so the returns on them
        // meet as at a whole corner: every check of `tray_with_level_returns_closes_every_corner`
        // holds, the flanges are the ones the notch gives, and the walls' bends are torn as without
        // returns (the returns' own bends are not).
        let tray = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0)];
        let trapezoid = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(100.0, 90.0), Vec2::new(20.0, 90.0)];
        let alpha = 90f64.atan2(20.0);
        let cases = [
            ("tray", tray, [FRAC_PI_2; 4], 90.0f64),
            ("tray", tray, [FRAC_PI_2; 4], 110.0),
            ("trapezoid", trapezoid, [alpha, alpha, PI - alpha, PI - alpha], 110.0),
        ];
        for (name, pts, angles, deg) in cases {
            let what = format!("a tear on the {name}, walls at {deg}°");
            let notch = tray_with_level_returns_closes_every_corner(&alu(), &pts, deg, &what);
            let s = tray_with_level_returns_closes_every_corner(&torn(), &pts, deg, &what);
            tears_nothing_but_the_bend_ends(&s, &notch, 8, &what);
            assert!(s.flanges.iter().all(|f| f.tear == (f.parent.is_none(), f.parent.is_none())), "{what}");
            let th = deg.to_radians();
            let want: f64 = angles.iter().map(|a| -(th * 2.0) * (0.25 - th.sin() * (a / 2.0).cos()) / (a / 2.0).sin()).sum();
            let gained = flat_area(&s) - flat_area(&notch);
            assert!((gained - want).abs() < 1e-6, "{what}: {gained} vs {want}");
            returns_keep_out_of_the_walls(&s, &what);
        }
    }

    #[test]
    fn a_corner_left_open_or_made_by_two_features_is_not_torn() {
        // A tear cuts to the flanges' closed ends. With a gap of zero the corners stay open: the
        // notes say so, nothing is torn, and the tear adds no note of its own.
        let (s, notes) = tray(&RuleValues { gap: 0.0, ..torn() }, 72.0, 38.6);
        assert_eq!(notes.len(), 4, "{notes:?}");
        assert!(notes.iter().all(|n| n.contains("left open") && n.contains("gap")), "{notes:?}");
        assert_eq!(s.flanges, tray(&RuleValues { gap: 0.0, ..alu() }, 72.0, 38.6).0.flanges);
        assert!(s.solid().is_ok());
        // Like the closing, the tear belongs to the corners between the flanges of one feature.
        let base = rect(Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 90.0), Vec2::new(0.0, 90.0));
        let mut s = base_flange("Body1", "AL2", &torn(), Plane::XY, base);
        s.add_flanges(&[Vec3::new(60.0, 0.0, 2.0)], 38.6, 72f64.to_radians(), 1.0, "inside", false).unwrap();
        let (_, notes) = s.add_flanges_noted(&[Vec3::new(120.0, 45.0, 2.0)], 25.0, 72f64.to_radians(), 1.0, "inside", false).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert!(s.flanges.iter().all(|f| f.tear == (false, false)));
        assert!(s.solid().is_ok() && s.flat_regions().unwrap().len() == 1);
    }

    #[test]
    fn a_torn_bend_end_folds_to_two_smooth_pieces_on_its_helix() {
        // The tray of `a_tear_cuts_each_bend_from_the_corner_to_its_flanges_end` (T = 2, R = 1):
        // each of its eight torn bend ends is the wall ruled between a helix on the bend's inner
        // face and one on its outer face, `bend_end` at the turn φ from 0 to θ. Cubic pieces took
        // 32 at either angle, which a viewer draws as 32 strips; two pieces of degree 7 hold the
        // helix to within the chains' tolerance.
        for deg in [90.0f64, 72.0] {
            let what = format!("{deg}°");
            let (s, _) = tray(&torn(), deg, 38.6);
            let mut work = FlatWork::new();
            let cells = s.cells_with_work(&mut work).unwrap();
            let faces = s.faces(&cells, 0.0, s.t, &mut work).unwrap();
            let walls: Vec<(&EdgeSpec, &EdgeSpec)> = faces
                .iter()
                .filter_map(|f| match &f.surface {
                    SurfSpec::Ruled { a, b } => Some((a, b)),
                    _ => None,
                })
                .collect();
            assert_eq!(walls.len(), 8, "{what}");
            let th = deg.to_radians();
            for (bottom, top) in walls {
                for (chain, z) in [(bottom, 0.0), (top, s.t)] {
                    let EdgeSpec::Chain { ctrl, .. } = chain else { panic!("{what}: a torn end folds to a chain") };
                    assert_eq!(ctrl.len(), 15, "{what}: two pieces of degree 7");
                    // The torn end this chain runs along, and which way.
                    let (first, last) = (ctrl[0], ctrl[ctrl.len() - 1]);
                    let end = s.flanges.iter().flat_map(|f| [(f, false), (f, true)]).find_map(|(f, at_end)| {
                        let at = |phi: f64| s.bend_end(f, at_end, phi, z);
                        if at(0.0).dist(first) < 1e-9 && at(th).dist(last) < 1e-9 {
                            Some((f, at_end, false))
                        } else if at(th).dist(first) < 1e-9 && at(0.0).dist(last) < 1e-9 {
                            Some((f, at_end, true))
                        } else {
                            None
                        }
                    });
                    let Some((f, at_end, back)) = end else { panic!("{what}: {first:?} to {last:?} is no torn end") };
                    for k in 0..2 {
                        for i in 0..=40 {
                            let t = (k as f64 + i as f64 / 40.0) / 2.0;
                            let exact = s.bend_end(f, at_end, th * if back { 1.0 - t } else { t }, z);
                            let off = bezier_at(&ctrl[7 * k..=7 * k + 7], i as f64 / 40.0).dist(exact);
                            assert!(off < BEND_CHAIN_TOL, "{what}: height {z}, {t}: {off}");
                        }
                    }
                }
            }
            holds_the_flat_material(&s, &what);
        }
    }

    #[test]
    fn a_torn_bend_end_is_no_flat_face() {
        use std::f64::consts::{FRAC_PI_2, SQRT_2};
        // A bend's flat ends are faces a sketch can sit on (`rim.bend0`, `rim.bend1`). The bar's top
        // flange (see `a_bend_relief_at_one_end_leaves_the_corner_at_the_other_as_it_was`) turns
        // about the axis y = 27, z = 3 between the radii 1 and 3. With the notch its bend ends on the
        // planes x = 157, at the corner, and x = 20, beside the leg; the middle of each end is 2 from
        // the axis, halfway round. Torn, the end at the corner is a helix that leaves its plane at
        // the corner of the bends: no flat face is there, the point has no name and the name gives
        // no plane. The end beside the leg is the face it was.
        let picks = [Vec3::new(90.0, 30.0, 2.0), Vec3::new(160.0, 15.0, 2.0)];
        let mid = |x: f64| Vec3::new(x, 27.0 + SQRT_2, 3.0 - SQRT_2);
        let mut notch = bar_with_leg(&alu(), 30.0);
        notch.add_flanges_noted(&picks, 15.0, FRAC_PI_2, 1.0, "inside", false).unwrap();
        let mut s = bar_with_leg(&torn(), 30.0);
        s.add_flanges_noted(&picks, 15.0, FRAC_PI_2, 1.0, "inside", false).unwrap();
        let (corner, at_corner) = notch.panel_face_at(mid(157.0), 1e-6).unwrap();
        let (leg, at_leg) = notch.panel_face_at(mid(20.0), 1e-6).unwrap();
        assert!(corner.starts_with("#0:rim.bend") && leg.starts_with("#0:rim.bend") && corner != leg, "{corner} {leg}");
        assert!(at_corner.normal().dist(Vec3::X) < 1e-12 && at_leg.normal().dist(Vec3::X * -1.0) < 1e-12, "{at_corner:?} {at_leg:?}");
        assert!(s.panel_face_at(mid(157.0), 1e-6).is_none() && s.panel_face(&corner).is_none(), "{corner}");
        assert_eq!(s.panel_face_at(mid(20.0), 1e-6), Some((leg.clone(), at_leg)));
        assert_eq!(s.panel_face(&leg), Some(at_leg));
    }

    #[test]
    fn a_tear_that_cannot_be_made_leaves_the_notch_with_a_note() {
        // Flanges at very different angles on the two edges at the origin of a parallelogram (as in
        // `every_pair_of_angles_is_torn_or_keeps_its_notch`). The bend lines lie s0 = 3·tan(θ/2) in
        // from their edges and meet at ((s0_b + s0_a·cos α) / sin α, s0_a), where the note points.
        // Each of the three reasons leaves the part the notch gives.
        let cases = [
            (90.0f64, 20.0f64, 140.0f64, "a tear would make the bends overlap in the flat pattern"),
            (90.0, 60.0, 120.0, "a tear would run a bend into the other one"),
            (120.0, 105.0, 60.0, "a tear would bring the bends nearer each other than the flanges' ends are"),
        ];
        for (alpha, a, b, why) in cases {
            let what = format!("corner {alpha}°, flanges {a}° and {b}°");
            let alpha = alpha.to_radians();
            let u = Vec2::new(alpha.cos(), alpha.sin());
            let pts = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 0.0) + u * 90.0, u * 90.0];
            let each = [
                EdgeFlange { pick: Vec3::new(60.0, 0.0, 2.0), height: 38.6, angle: a.to_radians() },
                EdgeFlange { pick: Vec3::new(u.x * 45.0, u.y * 45.0, 2.0), height: 38.6, angle: b.to_radians() },
            ];
            let base = || Region2 { outer: Loop2::polygon(&pts).ccw(), holes: vec![] };
            let (notch, none) = flanges_each(base(), &alu(), &each);
            let (s, notes) = flanges_each(base(), &torn(), &each);
            let (s0_a, s0_b) = (3.0 * (a.to_radians() / 2.0).tan(), 3.0 * (b.to_radians() / 2.0).tan());
            let c = Vec2::new((s0_b + s0_a * alpha.cos()) / alpha.sin(), s0_a);
            assert!(none.is_empty(), "{what}: {none:?}");
            assert_eq!(notes, [format!("the corner at ({:.3}, {:.3}, 0.000) keeps its notch: {why}", c.x, c.y)], "{what}");
            assert_eq!((&s.base, &s.flanges), (&notch.base, &notch.flanges), "{what}");
            holds_the_flat_material(&s, &what);
        }
    }

    /// The 100 x 60 plate with a 20 mm flange on its y = 0 edge (2.5 mm, R 2.5, K 0.44): in the
    /// flat the base starts at y = t + R = 5, the bend's zone is (π/2)·(R + K·t) long and the
    /// flange's panel 20 − 5, so its far edge lies at y = 5 − (π/2)·3.6 − 15.
    #[test]
    fn a_cut_names_the_pieces_it_parts_from_the_base_and_no_others() {
        use std::f64::consts::{FRAC_PI_2, PI};
        let far = 5.0 - FRAC_PI_2 * (2.5 + 0.44 * 2.5) - 15.0;
        let mut s = plate(100.0, 60.0);
        s.add_flange(Vec3::new(50.0, 0.0, 2.5), 20.0, FRAC_PI_2, 2.5, "inside", false).unwrap();
        let r = |x0: f64, y0: f64, x1: f64, y1: f64| rect(Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1));
        let ring = |c: Vec2, ro: f64, ri: f64| Region2 { outer: Loop2::circle(c, ro), holes: vec![Loop2::circle(c, ri)] };
        let parted = |before: &SheetBody, holes: &[Region2]| {
            let mut after = before.clone();
            after.holes.extend_from_slice(holes);
            after.parted_from_base(&after.flat_material().unwrap(), before).unwrap()
        };
        // A slot across the flange beside its bend parts the rest of the flange: 100 × (−3 − far).
        let slot = r(-5.0, -3.0, 105.0, -1.0);
        let p = parted(&s, std::slice::from_ref(&slot));
        assert!(p.len() == 1 && (p[0].area() - 100.0 * (-3.0 - far)).abs() < 1e-6, "{p:?}");
        // So does a ring on the flange's far edge: the half of its inside on the flange, π·2²/2.
        let p = parted(&s, &[ring(Vec2::new(50.0, far), 4.0, 2.0)]);
        assert!(p.len() == 1 && (p[0].area() - 2.0 * PI).abs() < 1e-6, "{p:?}");
        // A ring's island, on the base or on the flange, has material round it; a strip across the
        // whole sheet leaves two halves that each keep their part of the base. None is named.
        for holes in [ring(Vec2::new(50.0, 30.0), 10.0, 5.0), ring(Vec2::new(50.0, -9.0), 4.0, 2.0), r(45.0, -30.0, 55.0, 70.0)] {
            assert!(parted(&s, std::slice::from_ref(&holes)).is_empty(), "{holes:?}");
        }
        // Pieces an earlier cut parted are not named again, nor when a later cut splits them.
        let mut cut = s.clone();
        cut.holes.push(slot);
        for holes in [Region2 { outer: Loop2::circle(Vec2::new(30.0, 30.0), 4.0), holes: vec![] }, r(45.0, -30.0, 55.0, -2.0)] {
            assert!(parted(&cut, std::slice::from_ref(&holes)).is_empty(), "{holes:?}");
        }
        // Nothing is named once no piece holds the base: a cut that takes the base and both bends
        // of a sheet with flanges on two sides leaves the two flanges.
        let mut two = plate(100.0, 60.0);
        two.add_flanges(&[Vec3::new(50.0, 0.0, 2.5), Vec3::new(50.0, 60.0, 2.5)], 20.0, FRAC_PI_2, 2.5, "inside", false).unwrap();
        let mut after = two.clone();
        after.holes.push(r(-5.0, -3.0, 105.0, 63.0));
        let material = after.flat_material().unwrap();
        assert_eq!(material.len(), 2);
        assert!(after.parted_from_base(&material, &two).unwrap().is_empty());
    }
}
