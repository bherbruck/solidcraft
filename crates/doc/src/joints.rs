//! Joints between occurrences: joint origins (snap frames on component geometry, re-found on
//! every solve so they follow edits), joints (rigid, revolute, slider, cylindrical, pin-slot,
//! planar, ball) with their motion values and limits, motion links (gears, racks and screws are
//! links whose ratio comes from radii or a pitch; a cam's follower follows a table worked out from
//! its profile, `joints_cam.rs`), and the solver that places occurrences from the joint graph.
//!
//! A joint makes B's joint frame coincide with A's frame moved by the joint's motion:
//! `T_B · F_b = T_A · F_a · M(q) · Flip`, so `T_B = T_A · F_a · M(q) · Flip · F_b⁻¹`, where `T`
//! are occurrence world transforms, `F` the joint frames in each component's own frame, and `q`
//! the joint's values (angles in radians, distances in mm). The solver walks the graph from
//! grounded occurrences (and the root, which never moves). Joints that close a loop (a four-bar
//! linkage, a slider-crank) are met by moving the values of the other joints around the loop;
//! those that still disagree are reported.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

use solidcraft_sketch::linalg;

use crate::{DocError, Document, IDENTITY, Mat, ModelState, Result, apply_point, apply_vector, mat_inverse, mat_mul};

/// Joints the solver (and the URDF export) reads at most.
pub const MAX_JOINTS: usize = 10_000;

/// Where a joint origin sits on a component's geometry (component coordinates).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Snap {
    /// An explicit frame (origin, z axis, x axis).
    Frame {
        origin: Vec3,
        z: Vec3,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<Vec3>,
    },
    /// The centre of the planar face nearest `pick`, z along its outward normal. `face` is the
    /// face's persistent name (found first).
    FaceCenter {
        pick: Vec3,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        face: Option<String>,
    },
    /// The centre of the circular end nearest `pick` of the cylindrical face there, z along its
    /// axis (pointing out of the face's end).
    CircleCenter {
        pick: Vec3,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        face: Option<String>,
    },
    /// A point, z along `z` (default +Z).
    Point {
        point: Vec3,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<Vec3>,
    },
}

/// A joint origin: a snap on an occurrence's component (occurrence 0 = the root design).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JointOrigin {
    pub occurrence: u64,
    pub snap: Snap,
}

/// A named joint origin (Joint Origin command), referenced by joints by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedOrigin {
    pub name: String,
    pub origin: JointOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JointKind {
    Rigid,
    Revolute,
    Slider,
    Cylindrical,
    PinSlot,
    Planar,
    Ball,
}

impl JointKind {
    pub fn parse(s: &str) -> Option<JointKind> {
        Some(match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "rigid" => JointKind::Rigid,
            "revolute" | "rotate" => JointKind::Revolute,
            "slider" | "slide" => JointKind::Slider,
            "cylindrical" => JointKind::Cylindrical,
            "pinslot" => JointKind::PinSlot,
            "planar" => JointKind::Planar,
            "ball" => JointKind::Ball,
            _ => return None,
        })
    }
    /// Names of the joint's values, in order (angles in radians, distances in mm).
    pub fn dofs(self) -> &'static [&'static str] {
        match self {
            JointKind::Rigid => &[],
            JointKind::Revolute => &["angle"],
            JointKind::Slider => &["distance"],
            JointKind::Cylindrical => &["angle", "distance"],
            JointKind::PinSlot => &["angle", "slide"],
            JointKind::Planar => &["x", "y", "angle"],
            JointKind::Ball => &["yaw", "pitch", "roll"],
        }
    }
    pub fn is_angle(self, i: usize) -> bool {
        matches!(self.dofs().get(i), Some(&("angle" | "yaw" | "pitch" | "roll")))
    }
}

/// A joint between two occurrences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Joint {
    pub id: u64,
    pub name: String,
    pub kind: JointKind,
    pub a: JointOrigin,
    pub b: JointOrigin,
    /// Current values of the joint's degrees of freedom.
    #[serde(default)]
    pub values: Vec<f64>,
    /// Per value: (min, max) limits.
    #[serde(default)]
    pub limits: Vec<Option<(f64, f64)>>,
    /// Don't mate: B's z along A's instead of against it (by default the origins mate, as two
    /// faces meeting).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip: bool,
    /// Distance from A's frame to B's along z, and a turn about it (radians).
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub angle: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suppressed: bool,
}

/// Value of joint `b` (index `ib`) = ratio · value of joint `a` (index `ia`) + offset, or for a
/// cam, its `lift` table at a's value + offset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionLink {
    pub a: u64,
    #[serde(default)]
    pub ia: usize,
    pub b: u64,
    #[serde(default)]
    pub ib: usize,
    pub ratio: f64,
    #[serde(default)]
    pub offset: f64,
    /// Gears, a rack and pinion, a screw or a cam: what the ratio (or a cam's table) comes from.
    /// It is worked out from it again whenever the design changes (`coupling_values`), so it
    /// follows the radii, the pitch and the cam's profile; the last one worked out is kept in
    /// `ratio` and `lift`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coupling: Option<Coupling>,
    /// A cam's follower values over one turn of a (`cam::CAM_SAMPLES` of them, in b's units, the
    /// offset left out); empty for every other link.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lift: Vec<f64>,
}

impl MotionLink {
    /// b's value for a's value `x` (before b's limits). A cam's table that is not whole (a
    /// hand-edited file) gives no value, so b keeps where it is; a table on any other link is
    /// not read.
    pub fn follow(&self, x: f64) -> f64 {
        match &self.coupling {
            Some(Coupling::Cam(_)) if self.lift.len() == cam::CAM_SAMPLES => cam::lift_at(&self.lift, x) + self.offset,
            Some(Coupling::Cam(_)) => f64::NAN,
            _ => self.ratio * x + self.offset,
        }
    }
}

/// A length a coupling's ratio comes from: millimetres, an expression over the design's
/// parameters, or the radius of the round face picked on an occurrence (a circle-centre snap,
/// found again on every solve like a joint origin).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Size {
    Length(f64),
    Expr(String),
    Circle(JointOrigin),
}

/// The geometry a motion link's ratio comes from. `reverse` turns the follower the other way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Coupling {
    /// Two gears on revolute joints, `ra` on a's and `rb` on b's (pitch radii, or any two lengths
    /// in their proportion such as tooth counts): b turns ra/rb as far as a, the other way round,
    /// or the same way when one is an internal (ring) gear.
    Gear {
        ra: Size,
        rb: Size,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        internal: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reverse: bool,
    },
    /// A pinion of pitch radius `radius` on a revolute joint and a rack on a slider: the rack
    /// travels the radius times the pinion's turn in radians (either joint may drive).
    RackPinion {
        radius: Size,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reverse: bool,
    },
    /// A screw: the travel advances `pitch` per turn (a cylindrical joint's angle and distance,
    /// or a revolute joint and a slider). Right-handed unless reversed.
    Screw {
        pitch: Size,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reverse: bool,
    },
    /// A cam on a revolute joint (a) and its follower (b, a slider or a revolute): b rides the
    /// cam's profile (`joints_cam.rs`).
    Cam(cam::Cam),
}

impl Coupling {
    /// The lengths the ratio comes from: a's gear and b's, the pinion's radius, the pitch, or a
    /// cam's roller radius.
    pub fn sizes(&self) -> Vec<&Size> {
        match self {
            Coupling::Gear { ra, rb, .. } => vec![ra, rb],
            Coupling::RackPinion { radius: s, .. } | Coupling::Screw { pitch: s, .. } => vec![s],
            Coupling::Cam(c) => match &c.follower {
                cam::Follower::Roller { radius } => vec![radius],
                _ => Vec::new(),
            },
        }
    }

    pub fn sizes_mut(&mut self) -> Vec<&mut Size> {
        match self {
            Coupling::Gear { ra, rb, .. } => vec![ra, rb],
            Coupling::RackPinion { radius: s, .. } | Coupling::Screw { pitch: s, .. } => vec![s],
            Coupling::Cam(c) => match &mut c.follower {
                cam::Follower::Roller { radius } => vec![radius],
                _ => Vec::new(),
            },
        }
    }
}

/// Largest length a coupling takes (mm).
const MAX_SIZE: f64 = 1e7;

/// A coupling length in millimetres: positive and finite.
fn size_of(doc: &Document, st: &ModelState, s: &Size) -> Result<f64> {
    let v = match s {
        Size::Length(v) => *v,
        Size::Expr(e) => doc.eval(e, crate::expr::Kind::Length)?,
        Size::Circle(o) => circle_at(doc, st, o)?.1.radius,
    };
    if v.is_finite() && v > 1e-9 && v <= MAX_SIZE {
        Ok(v)
    } else {
        Err(DocError::Invalid(format!("a gear, rack, screw or roller needs a positive length, not {v}")))
    }
}

/// The ratio a link's coupling gives it (b's value per a's, in radians and mm) and a cam's table
/// (`MotionLink::lift`; empty for the others), worked out from the geometry and parameters it is
/// defined by; `None` for a plain link.
pub fn coupling_values(doc: &Document, st: &ModelState, l: &MotionLink) -> Option<Result<(f64, Vec<f64>)>> {
    l.coupling.as_ref().map(|c| match c {
        Coupling::Cam(c) => cam::table(doc, st, l, c).map(|t| (1.0, t)),
        c => coupled(doc, st, l, c).map(|r| (r, Vec::new())),
    })
}

fn coupled(doc: &Document, st: &ModelState, l: &MotionLink, c: &Coupling) -> Result<f64> {
    let angle = |id: u64, i: usize| doc.assembly.joints.iter().find(|j| j.id == id).is_some_and(|j| j.kind.is_angle(i));
    let (turn_a, turn_b) = (angle(l.a, l.ia), angle(l.b, l.ib));
    // Travel per radian, for a turn coupled to a travel either way round.
    let travel = |k: f64| match (turn_a, turn_b) {
        (true, false) => Ok(k),
        (false, true) => Ok(1.0 / k),
        _ => Err(DocError::Invalid("a rack or a screw couples a turn and a travel".into())),
    };
    let sign = |reverse: bool| if reverse { -1.0 } else { 1.0 };
    match c {
        Coupling::Gear { ra, rb, internal, reverse } => {
            if !(turn_a && turn_b) {
                return Err(DocError::Invalid("gears couple two turns".into()));
            }
            Ok(size_of(doc, st, ra)? / size_of(doc, st, rb)? * sign(!internal) * sign(*reverse))
        }
        Coupling::RackPinion { radius, reverse } => Ok(travel(size_of(doc, st, radius)?)? * sign(*reverse)),
        Coupling::Screw { pitch, reverse } => Ok(travel(size_of(doc, st, pitch)? / std::f64::consts::TAU)? * sign(*reverse)),
        // A cam's follower follows its table (`coupling_values`), not a ratio.
        Coupling::Cam(_) => Ok(1.0),
    }
}

/// Joints, joint origins and motion links of a design.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Assembly {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joints: Vec<Joint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origins: Vec<NamedOrigin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<MotionLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contact_sets: Vec<crate::motion::ContactSet>,
    /// Contact sets take effect.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub contact_enabled: bool,
    /// Every occurrence is in contact with every other (Enable All Contact).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub contact_all: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub studies: Vec<crate::motion::MotionStudy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exploded: Vec<crate::motion::ExplodedView>,
}

impl Assembly {
    pub fn is_empty(&self) -> bool {
        self.joints.is_empty()
            && self.origins.is_empty()
            && self.links.is_empty()
            && self.contact_sets.is_empty()
            && !self.contact_enabled
            && !self.contact_all
            && self.studies.is_empty()
            && self.exploded.is_empty()
    }
}

/// A frame: origin and orthonormal axes, as a transform (local → parent).
fn frame(o: Vec3, z: Vec3, x: Option<Vec3>) -> Option<Mat> {
    let z = z.normalized()?;
    let x = x.and_then(|x| (x - z * x.dot(z)).normalized()).unwrap_or_else(|| z.any_perp());
    let y = z.cross(x).normalized()?;
    Some([[x.x, x.y, x.z, 0.0], [y.x, y.y, y.z, 0.0], [z.x, z.y, z.z, 0.0], [o.x, o.y, o.z, 1.0]])
}

fn rot_z(a: f64) -> Mat {
    let (s, c) = a.sin_cos();
    [[c, s, 0.0, 0.0], [-s, c, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
}
fn rot_y(a: f64) -> Mat {
    let (s, c) = a.sin_cos();
    [[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]]
}
fn shift(x: f64, y: f64, z: f64) -> Mat {
    let mut m = IDENTITY;
    m[3] = [x, y, z, 1.0];
    m
}

/// The joint's motion in A's frame for its values.
pub fn motion(kind: JointKind, q: &[f64]) -> Mat {
    let v = |i: usize| q.get(i).copied().unwrap_or(0.0);
    match kind {
        JointKind::Rigid => IDENTITY,
        JointKind::Revolute => rot_z(v(0)),
        JointKind::Slider => shift(0.0, 0.0, v(0)),
        JointKind::Cylindrical => mat_mul(&rot_z(v(0)), &shift(0.0, 0.0, v(1))),
        JointKind::PinSlot => mat_mul(&shift(v(1), 0.0, 0.0), &rot_z(v(0))),
        JointKind::Planar => mat_mul(&shift(v(0), v(1), 0.0), &rot_z(v(2))),
        JointKind::Ball => mat_mul(&mat_mul(&rot_z(v(0)), &rot_y(v(1))), &rot_z(v(2))),
    }
}

/// Bodies of a component in the model (its own frame).
fn component_bodies<'a>(doc: &Document, st: &'a ModelState, component: u64) -> Vec<&'a crate::ModelBody> {
    st.bodies.iter().filter(|b| doc.model_body_component(b) == component).collect()
}

/// The component an occurrence places (0 for the root).
fn component_of(doc: &Document, occ: u64) -> Result<u64> {
    if occ == 0 {
        return Ok(0);
    }
    doc.occurrences.iter().find(|o| o.id == occ).map(|o| o.component).ok_or_else(|| DocError::Unknown(format!("occurrence {occ}")))
}

/// Resolve a joint origin to a frame in its component's coordinates.
/// A snap's pick point moved onto its named face (an error when the face is gone: a joint must
/// not quietly attach elsewhere).
fn named_pick(doc: &Document, st: &ModelState, pick: Vec3, face: Option<&str>) -> Result<Vec3> {
    let Some(n) = face else { return Ok(pick) };
    let gone = || DocError::Invalid(format!("joint origin: the face `{n}` no longer exists (edit the joint to pick again)"));
    let hit = crate::name_lookup::locate_face(st, n, pick).ok_or_else(gone)?;
    // A body moved in from another component keeps its geometry where it was made (its
    // offset places it): the pick, in the component's coordinates, is taken to the body's own
    // and the point found brought back.
    match st.bodies.get(hit.body).and_then(|b| doc.model_body_offset(b).map(|m| (b, m))) {
        Some((b, m)) => {
            let q = body_point(m, pick).ok_or_else(gone)?;
            crate::naming::face_on_body(b, n, q).map(|(_, at, _)| apply_point(m, at)).ok_or_else(gone)
        }
        None => Ok(hit.point),
    }
}

/// A point in a component's coordinates taken into the coordinates of a body placed by `offset`.
fn body_point(offset: &Mat, p: Vec3) -> Option<Vec3> {
    mat_inverse(offset).map(|inv| apply_point(&inv, p))
}

/// The cylindrical face of a component's body at `pick` (the component's coordinates), in the
/// component's coordinates: a body moved in from another component is looked up where its
/// geometry is, through its offset.
pub fn cylinder_face_at(doc: &Document, b: &crate::ModelBody, pick: Vec3) -> Option<solidcraft_kernel::CylinderFace> {
    placed_cylinder(doc, b, pick, solidcraft_kernel::cylinder_face_at)
}

/// [`cylinder_face_at`] for a cylinder one of whose end circles is centred at `pick`.
pub fn cylinder_end_at(doc: &Document, b: &crate::ModelBody, pick: Vec3) -> Option<solidcraft_kernel::CylinderFace> {
    placed_cylinder(doc, b, pick, solidcraft_kernel::cylinder_end_at)
}

fn placed_cylinder(
    doc: &Document,
    b: &crate::ModelBody,
    pick: Vec3,
    find: fn(&solidcraft_kernel::Body, Vec3) -> Option<solidcraft_kernel::CylinderFace>,
) -> Option<solidcraft_kernel::CylinderFace> {
    let Some(m) = doc.model_body_offset(b) else { return find(&b.body, pick) };
    let c = find(&b.body, body_point(m, pick)?)?;
    Some(solidcraft_kernel::CylinderFace { axis_point: apply_point(m, c.axis_point), axis: apply_vector(m, c.axis), ..c })
}

pub fn resolve_origin(doc: &Document, st: &ModelState, o: &JointOrigin) -> Result<Mat> {
    let bad = || DocError::Invalid("joint origin: degenerate frame".into());
    match &o.snap {
        Snap::Frame { origin, z, x } => frame(*origin, *z, *x).ok_or_else(bad),
        Snap::Point { point, z } => frame(*point, z.unwrap_or(Vec3::Z), None).ok_or_else(bad),
        Snap::FaceCenter { pick, face } => {
            let pick = named_pick(doc, st, *pick, face.as_deref())?;
            let comp = component_of(doc, o.occurrence)?;
            let mut best: Option<(f64, Vec3, Vec3)> = None;
            for b in component_bodies(doc, st, comp) {
                let tol = (b.body.size() * 2e-3).max(1e-3);
                let m = doc.model_body_offset(b).copied().unwrap_or(IDENTITY);
                for f in b.body.faces(tol)? {
                    let Some(n) = f.plane_normal else { continue };
                    let (c, n) = (apply_point(&m, f.centroid), apply_vector(&m, n));
                    let d = (pick - c).dot(n).abs() + 1e-3 * (pick - c).len();
                    if best.as_ref().is_none_or(|(bd, _, _)| d < *bd) {
                        best = Some((d, c, n));
                    }
                }
            }
            let (_, c, n) = best.ok_or_else(|| DocError::Invalid("joint origin: no planar face there".into()))?;
            frame(c, n, None).ok_or_else(bad)
        }
        Snap::CircleCenter { .. } => {
            let (pick, cyl) = circle_at(doc, st, o)?;
            let s = (pick - cyl.axis_point).dot(cyl.axis);
            let (at, dir) = if (s - cyl.start).abs() <= (cyl.end - s).abs() { (cyl.start, -1.0) } else { (cyl.end, 1.0) };
            frame(cyl.axis_point + cyl.axis * at, cyl.axis * dir, None).ok_or_else(bad)
        }
    }
}

/// The pick of a circle-centre snap (on its named face) and the cylindrical face there
/// (component coordinates). Any other snap is refused.
fn circle_at(doc: &Document, st: &ModelState, o: &JointOrigin) -> Result<(Vec3, solidcraft_kernel::CylinderFace)> {
    let Snap::CircleCenter { pick, face } = &o.snap else { return Err(DocError::Invalid("pick a round face".into())) };
    let pick = named_pick(doc, st, *pick, face.as_deref())?;
    let comp = component_of(doc, o.occurrence)?;
    let cyl = component_bodies(doc, st, comp)
        .into_iter()
        .filter_map(|b| cylinder_face_at(doc, b, pick))
        .min_by(|a, b| {
            // Off the wall, and past its ends: coaxial holes of two bodies have one radius, and
            // the pick is on the wall of only one of them.
            let off = |c: &solidcraft_kernel::CylinderFace| {
                let v = pick - c.axis_point;
                let along = v.dot(c.axis);
                ((v - c.axis * along).len() - c.radius).abs() + (c.start - along).max(along - c.end).max(0.0)
            };
            off(a).total_cmp(&off(b))
        })
        .ok_or_else(|| DocError::Invalid("joint origin: no cylindrical face there".into()))?;
    Ok((pick, cyl))
}

/// World transform of an occurrence (through its parents).
pub fn occurrence_world(doc: &Document, occ: u64) -> Mat {
    let mut m = IDENTITY;
    let mut cur = doc.occurrences.iter().find(|o| o.id == occ);
    for _ in 0..1000 {
        let Some(o) = cur else { break };
        m = mat_mul(&o.transform, &m);
        if o.parent == 0 {
            break;
        }
        cur = doc.occurrence_of(o.parent);
    }
    m
}

/// Clamp values to the joint's limits.
pub fn clamp(j: &Joint, q: &mut [f64]) {
    for (i, v) in q.iter_mut().enumerate() {
        if let Some(Some((lo, hi))) = j.limits.get(i) {
            *v = v.clamp(lo.min(*hi), hi.max(*lo));
        }
    }
}

/// The result of placing occurrences by their joints.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Solution {
    /// New world transforms of occurrences the joints place.
    #[serde(skip)]
    pub world: BTreeMap<u64, Mat>,
    /// New values of the joints that moved to close loops (joint id → values).
    #[serde(skip)]
    pub values: BTreeMap<u64, Vec<f64>>,
    /// Joints that disagree with the placement found through others: (joint id, name, why).
    pub conflicts: Vec<(u64, String, String)>,
    /// Joints that could not be applied (bad origin…): (joint id, name, why).
    pub errors: Vec<(u64, String, String)>,
    /// Remaining degrees of freedom per occurrence (6 = free, 0 = fixed).
    pub dof: BTreeMap<u64, usize>,
    /// Occurrences fixed in place (grounded, or the root's).
    pub fixed: Vec<u64>,
}

/// Joint values by joint id.
type Values = BTreeMap<u64, Vec<f64>>;

/// How far two placements differ: (translation mm, rotation rad).
fn mismatch(a: &Mat, b: &Mat) -> (f64, f64) {
    let t = Vec3::new(a[3][0] - b[3][0], a[3][1] - b[3][1], a[3][2] - b[3][2]).len();
    // The turn between them from both its cosine and its sine: the cosine alone loses small
    // angles to rounding.
    let c = |i: usize, k: usize| (0..3).map(|n| a[i][n] * b[k][n]).sum::<f64>();
    let sin2 = Vec3::new(c(1, 2) - c(2, 1), c(2, 0) - c(0, 2), c(0, 1) - c(1, 0)).len();
    (t, sin2.atan2(c(0, 0) + c(1, 1) + c(2, 2) - 1.0))
}

/// Passes over the motion links at most: a chain of up to this many links carries its value
/// through, whatever order its links are stored in.
const MAX_LINK_PASSES: usize = 64;

/// Motion links, pass after pass until nothing changes (a chain stored from its end takes a pass
/// per link), each linked value within its joint's limits as it is set. The result is the same
/// whatever the values were before and however the links are stored, so applying them again
/// changes nothing (links that form a cycle stop after `MAX_LINK_PASSES`).
fn apply_links(asm: &Assembly, vals: &mut Values) {
    let limits: BTreeMap<(u64, usize), (f64, f64)> = asm
        .links
        .iter()
        .filter_map(|l| {
            let (lo, hi) = asm.joints.iter().find(|j| j.id == l.b)?.limits.get(l.ib).copied().flatten()?;
            Some(((l.b, l.ib), (lo.min(hi), hi.max(lo))))
        })
        .collect();
    // The linked values, to see whether a pass changed any (several links writing one value
    // change it within a pass, but settle from one pass to the next).
    let linked = |vals: &Values| -> Vec<Option<f64>> { asm.links.iter().map(|l| vals.get(&l.b).and_then(|q| q.get(l.ib)).copied()).collect() };
    for _ in 0..MAX_LINK_PASSES {
        let before = linked(vals);
        for l in &asm.links {
            let Some(src) = vals.get(&l.a).and_then(|q| q.get(l.ia)).copied() else { continue };
            let mut next = l.follow(src);
            // A hand-edited table (or source) that gives no number leaves b where it is.
            if !next.is_finite() {
                continue;
            }
            if let Some((lo, hi)) = limits.get(&(l.b, l.ib)) {
                next = next.max(*lo).min(*hi);
            }
            if let Some(v) = vals.get_mut(&l.b).and_then(|q| q.get_mut(l.ib)) {
                *v = next;
            }
        }
        if linked(vals) == before {
            break;
        }
    }
}

/// Effective values of every joint after motion links and limits.
pub fn joint_values(asm: &Assembly) -> BTreeMap<u64, Vec<f64>> {
    let mut vals: Values = asm
        .joints
        .iter()
        .map(|j| {
            let mut q = j.values.clone();
            q.resize(j.kind.dofs().len(), 0.0);
            (j.id, q)
        })
        .collect();
    apply_links(asm, &mut vals);
    for j in &asm.joints {
        if let Some(q) = vals.get_mut(&j.id) {
            clamp(j, q);
        }
    }
    vals
}

/// The turn and offset from A's joint frame to B's: mated (B's z against A's, as faces meet)
/// unless flipped.
fn flip_of(j: &Joint) -> Mat {
    let mut m = rot_z(j.angle);
    m[3][2] = j.offset;
    if !j.flip { mat_mul(&m, &[[1.0, 0.0, 0.0, 0.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, -1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]) } else { m }
}

/// B's world transform from A's (or A's from B's, going the other way) for the joint's values.
fn through(j: &Joint, (fa, fb): &(Mat, Mat), q: &[f64], from_a: bool, t: &Mat) -> Option<Mat> {
    let link = mat_mul(&mat_mul(&motion(j.kind, q), &flip_of(j)), &mat_inverse(fb)?);
    if from_a {
        Some(mat_mul(&mat_mul(t, fa), &link))
    } else {
        // T_A = T_B · link⁻¹ · F_a⁻¹
        Some(mat_mul(&mat_mul(t, &mat_inverse(&link)?), &mat_inverse(fa)?))
    }
}

/// The spanning tree the solver places occurrences along, out from the fixed ones.
struct Tree<'a> {
    joints: BTreeMap<u64, &'a Joint>,
    /// Joint frames in component coordinates (A's, B's).
    frames: BTreeMap<u64, (Mat, Mat)>,
    fixed: BTreeMap<u64, Mat>,
    /// Placed occurrence → (the occurrence it is placed from, the joint, from the joint's A side).
    parent: BTreeMap<u64, (u64, u64, bool)>,
    /// Placed occurrences in placement order.
    order: Vec<u64>,
}

impl Tree<'_> {
    fn step(&self, joint: u64, from_a: bool, t: &Mat, vals: &Values) -> Option<Mat> {
        let q = vals.get(&joint).map(Vec::as_slice).unwrap_or(&[]);
        through(self.joints.get(&joint)?, self.frames.get(&joint)?, q, from_a, t)
    }

    /// Where every placed occurrence is for these values.
    fn place(&self, vals: &Values) -> BTreeMap<u64, Mat> {
        let mut placed = self.fixed.clone();
        for occ in &self.order {
            let Some(&(from, joint, from_a)) = self.parent.get(occ) else { continue };
            if let Some(m) = placed.get(&from).and_then(|t| self.step(joint, from_a, t, vals)) {
                placed.insert(*occ, m);
            }
        }
        placed
    }

    /// The fixed occurrence an occurrence is placed from, and the tree joints from it up to that one.
    fn path(&self, mut occ: u64) -> (u64, Vec<(u64, bool)>) {
        let mut up = Vec::new();
        while let Some(&(from, joint, from_a)) = self.parent.get(&occ) {
            if up.len() > self.order.len() {
                break;
            }
            up.push((joint, from_a));
            occ = from;
        }
        (occ, up)
    }

    /// One occurrence's world placement for these values.
    fn world(&self, occ: u64, vals: &Values) -> Option<Mat> {
        let (root, up) = self.path(occ);
        let mut t = *self.fixed.get(&root)?;
        for (joint, from_a) in up.iter().rev() {
            t = self.step(*joint, *from_a, &t, vals)?;
        }
        Some(t)
    }
}

/// Values the loop solver moves at most in one group of loops, and loop-closing joints it takes
/// on at most: larger ones are only checked.
const MAX_LOOP_VALUES: usize = 64;
const MAX_LOOPS: usize = 100;
/// Residual evaluations one solve may spend on all its loops.
const MAX_LOOP_WORK: usize = 200_000;

/// Held joint values by (joint, value index): the value each moves from.
pub type Held = BTreeMap<(u64, usize), f64>;

/// The values that set the `around` joints' values through motion links, followed back through
/// every link to them (several links may write one value; `apply_links`' order decides which
/// wins on the way) for as many links as `apply_links` passes: all the values reached, and those
/// that no link sets (the ones a solve can move).
fn link_chains(asm: &Assembly, tree: &Tree, around: &BTreeSet<u64>, linked: &BTreeSet<(u64, usize)>) -> (BTreeSet<(u64, usize)>, Vec<(u64, usize)>) {
    let (mut feeds, mut starts) = (BTreeSet::new(), Vec::new());
    let mut frontier: BTreeSet<(u64, usize)> =
        around.iter().filter_map(|id| tree.joints.get(id)).flat_map(|j| (0..j.kind.dofs().len()).map(|i| (j.id, i))).collect();
    let mut seen = frontier.clone();
    for _ in 0..MAX_LINK_PASSES {
        let next: BTreeSet<(u64, usize)> =
            asm.links.iter().filter(|l| frontier.contains(&(l.b, l.ib))).map(|l| (l.a, l.ia)).filter(|v| !seen.contains(v)).collect();
        if next.is_empty() {
            break;
        }
        for &(id, i) in &next {
            feeds.insert((id, i));
            if !linked.contains(&(id, i)) && tree.joints.get(&id).is_some_and(|j| i < j.kind.dofs().len()) {
                starts.push((id, i));
            }
        }
        seen.extend(next.iter().copied());
        frontier = next;
    }
    (feeds, starts)
}

/// The held values among `touches` that move: (joint, index, from, to), `from` within the
/// joint's limits. A suppressed joint still sets values through its motion links.
fn held_moves(asm: &Assembly, held: &Held, touches: &BTreeSet<(u64, usize)>, vals: &Values) -> Vec<(u64, usize, f64, f64)> {
    let mut moves = Vec::new();
    for ((id, i), from) in held.iter().filter(|(v, _)| touches.contains(v)) {
        let (Some(j), Some(to)) = (asm.joints.iter().find(|j| j.id == *id), vals.get(id).and_then(|q| q.get(*i))) else { continue };
        let from = match j.limits.get(*i).copied().flatten() {
            Some((lo, hi)) => from.max(lo.min(hi)).min(hi.max(lo)),
            None => *from,
        };
        if from != *to {
            moves.push((*id, *i, from, *to));
        }
    }
    moves
}

/// Close the loops, each group of loops that share joints on its own: see `close_group`.
/// Returns the moved joints' new stored values (and updates `vals`), and the closing joints of
/// the groups whose held values moved but which can't stay closed on the way (nothing in those
/// moves; a group that can't close without moving is left to the conflict check).
fn close_loops(tree: &Tree, closing: &[u64], asm: &Assembly, held: &Held, vals: &mut Values) -> (Values, BTreeSet<u64>) {
    let (mut changed, mut stuck) = (BTreeMap::new(), BTreeSet::new());
    if closing.len() > MAX_LOOPS {
        return (Values::new(), stuck);
    }
    let linked: BTreeSet<(u64, usize)> = asm.links.iter().map(|l| (l.b, l.ib)).collect();
    let groups = loop_groups(tree, closing, asm, &linked);
    let spent = std::cell::Cell::new(0usize);
    for (group, around, touches) in &groups {
        let moves = held_moves(asm, held, touches, vals);
        match close_group(tree, group, around, &moves, asm, held, &linked, &spent, vals) {
            Some(m) => changed.extend(m),
            None if !moves.is_empty() => stuck.extend(group.iter().copied()),
            None => {}
        }
    }
    // The stored values with what each group moved (groups may move different values of one
    // joint).
    let mut moved = Values::new();
    for ((id, i), v) in changed {
        let Some(j) = tree.joints.get(&id) else { continue };
        let q = moved.entry(id).or_insert_with(|| {
            let mut q = j.values.clone();
            q.resize(j.kind.dofs().len(), 0.0);
            q
        });
        if let Some(slot) = q.get_mut(i) {
            *slot = v;
        }
    }
    (moved, stuck)
}

/// A group of loops solved together: its closing joints, the joints around them, and the values
/// it touches.
type Group = (Vec<u64>, BTreeSet<u64>, BTreeSet<(u64, usize)>);

/// The loops' groups. Each loop: its closing joint and the joints on the tree paths between its
/// sides; loops that share a value (of a joint, or one feeding them through links) are solved
/// together.
fn loop_groups(tree: &Tree, closing: &[u64], asm: &Assembly, linked: &BTreeSet<(u64, usize)>) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for id in closing {
        let Some(j) = tree.joints.get(id) else { continue };
        // A joint between an occurrence and itself (only a hand-edited file has one) is only checked.
        if j.a.occurrence == j.b.occurrence {
            continue;
        }
        let side = |occ: u64| -> BTreeSet<u64> { tree.path(occ).1.iter().map(|(k, _)| *k).collect() };
        let mut around: BTreeSet<u64> = side(j.a.occurrence).symmetric_difference(&side(j.b.occurrence)).copied().collect();
        around.insert(*id);
        // The values the loop depends on: its joints' and those its links read. Loops tie together
        // through a shared value only (a rigid joint on both paths has none; two values of one
        // joint can feed two independent loops).
        let mut touches = link_chains(asm, tree, &around, linked).0;
        touches.extend(around.iter().filter_map(|id| tree.joints.get(id)).flat_map(|j| (0..j.kind.dofs().len()).map(|i| (j.id, i))));
        let mut members = vec![*id];
        groups.retain(|(c, a, t)| {
            if t.is_disjoint(&touches) {
                return true;
            }
            members.extend(c.iter().copied());
            around.extend(a.iter().copied());
            touches.extend(t.iter().copied());
            false
        });
        groups.push((members, around, touches));
    }
    groups
}

/// The values a solve of the loops `around` moves: every value of their joints but the `held`
/// ones and motion-link targets, and the values the targets' link chains start from.
fn unknowns(tree: &Tree, around: &BTreeSet<u64>, asm: &Assembly, linked: &BTreeSet<(u64, usize)>, held: &Held) -> Vec<(u64, usize)> {
    let mut vars: Vec<(u64, usize)> = Vec::new();
    for j in around.iter().filter_map(|id| tree.joints.get(id)) {
        vars.extend((0..j.kind.dofs().len()).map(|i| (j.id, i)).filter(|v| !linked.contains(v) && !held.contains_key(v)));
    }
    for v in link_chains(asm, tree, around, linked).1 {
        if !held.contains_key(&v) && !vars.contains(&v) {
            vars.push(v);
        }
    }
    vars
}

/// Each value's limits, or none.
fn limit_bounds(tree: &Tree, vars: &[(u64, usize)]) -> Vec<(f64, f64)> {
    vars.iter()
        .map(|(id, i)| match tree.joints.get(id).and_then(|j| j.limits.get(*i).copied().flatten()) {
            Some((lo, hi)) => (lo.min(hi), hi.max(lo)),
            None => (f64::NEG_INFINITY, f64::INFINITY),
        })
        .collect()
}

/// The loops' size: the diagonal of the box around their joints' A frames, 1 mm to 1 km. Points
/// that far apart weigh turns like moves.
fn loop_scale(tree: &Tree, around: &BTreeSet<u64>, vals: &Values) -> f64 {
    let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
    for id in around {
        let (Some(j), Some((fa, _))) = (tree.joints.get(id), tree.frames.get(id)) else { continue };
        let Some(t) = tree.world(j.a.occurrence, vals) else { continue };
        let p = apply_point(&mat_mul(&t, fa), Vec3::ZERO);
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    if lo.x <= hi.x { (hi - lo).len().clamp(1.0, 1e6) } else { 1.0 }
}

/// How far each closing joint is from meeting: three points of B's joint frame, `scale` apart,
/// where the joint wants them against where the tree puts them (appended to `out`).
fn closure_gaps(tree: &Tree, closing: &[u64], vals: &Values, scale: f64, out: &mut Vec<f64>) -> Option<()> {
    for id in closing {
        let (j, (_, fb)) = (tree.joints.get(id)?, tree.frames.get(id)?);
        let tb = tree.world(j.b.occurrence, vals)?;
        let want = tree.step(*id, true, &tree.world(j.a.occurrence, vals)?, vals)?;
        for p in [Vec3::ZERO, Vec3::X * scale, Vec3::Y * scale] {
            let at = apply_point(fb, p);
            let d = apply_point(&want, at) - apply_point(&tb, at);
            out.extend([d.x, d.y, d.z]);
        }
    }
    Some(())
}

/// Occurrences whose degrees of freedom are worked out through their loops, at most; the rest
/// keep the tree's count.
const MAX_DOF_OCCURRENCES: usize = 256;

/// How far `nearby_pose` moves each value: radians for an angle, a share of the loops' size for a
/// distance. Far enough for a part's motion that vanishes at a dead point to show in a rank, near
/// enough to stay on the mechanism's branch.
const NUDGE: f64 = 1e-3;

/// A pose of the loops near `vals`: each value they move nudged by an amount of its own within
/// its limits (so the nudge points in no special direction), then each group of loops closed
/// again. A group that can't close again, or only further away than a few nudges (on another
/// branch), keeps its values. It has a work budget of its own.
fn nearby_pose(tree: &Tree, groups: &[Group], asm: &Assembly, linked: &BTreeSet<(u64, usize)>, vals: &Values) -> Values {
    let mut near = vals.clone();
    let mut spent = 0usize;
    for (closing, around, _) in groups {
        let vars = unknowns(tree, around, asm, linked, &Held::new());
        if vars.is_empty() || vars.len() > MAX_LOOP_VALUES {
            continue;
        }
        let scale = loop_scale(tree, around, vals);
        let step: Vec<f64> =
            vars.iter().map(|(id, i)| NUDGE * if tree.joints.get(id).is_some_and(|j| j.kind.is_angle(*i)) { 1.0 } else { scale }).collect();
        let bounds = limit_bounds(tree, &vars);
        let x0: Vec<f64> = vars.iter().map(|(id, i)| near.get(id).and_then(|q| q.get(*i)).copied().unwrap_or(0.0)).collect();
        let mut x: Vec<f64> = x0
            .iter()
            .zip(step.iter().zip(&bounds))
            .enumerate()
            .map(|(k, (v, (d, (lo, hi))))| {
                // 0.5 to 1.5 nudges, alternately up and down.
                let w = 0.5 + (k as f64 * 0.618_034).fract();
                (v + d * if k % 2 == 0 { w } else { -w }).max(*lo).min(*hi)
            })
            .collect();
        let mut work = near.clone();
        let mut gaps = |x: &[f64]| -> Option<Vec<f64>> {
            spent += 1;
            if spent > MAX_LOOP_WORK {
                return None;
            }
            for ((id, i), v) in vars.iter().zip(x) {
                if let Some(slot) = work.get_mut(id).and_then(|q| q.get_mut(*i)) {
                    *slot = *v;
                }
            }
            apply_links(asm, &mut work);
            let mut out = Vec::with_capacity(closing.len() * 9);
            closure_gaps(tree, closing, &work, scale, &mut out)?;
            out.iter().all(|v| v.is_finite()).then_some(out)
        };
        if least_squares(&mut gaps, &mut x, &bounds, (1e-10 * scale).min(1e-7))
            && x.iter().zip(&x0).zip(&step).all(|((a, b), d)| (a - b).abs() <= 4.0 * d)
            && gaps(&x).is_some()
        {
            near = work;
        }
    }
    near
}

/// The degrees of freedom of the occurrences loops constrain: how many independent ways each can
/// still move with every loop kept closed, the rank of its placement over the joint values once
/// the loops' closure is taken out (rank [J_loops; J_placement] - rank J_loops). An occurrence's
/// values are those on its tree path and the unknowns of the loops on that path. It is counted at
/// the pose and at a pose nearby, and the larger count stands: at a dead point of its loop (an
/// actuator in line with the path of its tip pin) a part's motion vanishes to first order, so the
/// pose alone counts 0. The tree's count stands where a loop group is over the solver's caps or
/// the work runs out.
fn loop_dof(tree: &Tree, closing: &[u64], asm: &Assembly, vals: &Values, dof: &mut BTreeMap<u64, usize>) {
    if closing.len() > MAX_LOOPS {
        return;
    }
    let linked: BTreeSet<(u64, usize)> = asm.links.iter().map(|l| (l.b, l.ib)).collect();
    let groups = loop_groups(tree, closing, asm, &linked);
    // The joints loops depend on: those around them, those whose values they read, and those a
    // motion link carries any of these values into.
    let mut relevant: BTreeSet<u64> = groups.iter().flat_map(|(_, a, t)| a.iter().copied().chain(t.iter().map(|(j, _)| *j))).collect();
    for _ in 0..MAX_LINK_PASSES {
        let more: Vec<u64> = asm.links.iter().filter(|l| relevant.contains(&l.a) && !relevant.contains(&l.b)).map(|l| l.b).collect();
        if more.is_empty() {
            break;
        }
        relevant.extend(more);
    }
    // The occurrences a loop may hold or move: placed by such a joint, or below one that is
    // (worked out down the tree, parents first). The others keep the tree's count.
    let mut candidates: BTreeSet<u64> = BTreeSet::new();
    for occ in &tree.order {
        if let Some(&(from, joint, _)) = tree.parent.get(occ)
            && (relevant.contains(&joint) || candidates.contains(&from))
        {
            candidates.insert(*occ);
        }
    }
    let (mut spent, mut counted) = (0usize, 0usize);
    // A pose nearby, when one differs.
    let near = (!candidates.is_empty()).then(|| nearby_pose(tree, &groups, asm, &linked, vals)).filter(|n| n != vals);
    for occ in tree.order.iter().filter(|o| candidates.contains(o)) {
        let path: Vec<u64> = tree.path(*occ).1.iter().map(|(k, _)| *k).collect();
        let (mut values, mut have): (Vec<(u64, usize)>, BTreeSet<(u64, usize)>) = (Vec::new(), BTreeSet::new());
        let mut add = |values: &mut Vec<(u64, usize)>, v: (u64, usize)| {
            if have.insert(v) {
                values.push(v);
            }
        };
        for j in path.iter().filter_map(|k| tree.joints.get(k)) {
            for v in (0..j.kind.dofs().len()).map(|i| (j.id, i)).filter(|v| !linked.contains(v)) {
                add(&mut values, v);
            }
            if values.len() > MAX_LOOP_VALUES {
                break;
            }
        }
        if values.len() > MAX_LOOP_VALUES {
            continue;
        }
        // A value on the path a motion link sets moves with the values its link chain starts from.
        let on_path: BTreeSet<u64> = path.iter().copied().collect();
        for v in link_chains(asm, tree, &on_path, &linked).1 {
            add(&mut values, v);
        }
        // The loops on the path, and every loop whose values those reach (a linked source may be
        // held by a loop of its own), each once.
        let (mut closers, mut around) = (Vec::new(), BTreeSet::new());
        let mut taken: BTreeSet<usize> = BTreeSet::new();
        loop {
            let more: Vec<usize> = groups
                .iter()
                .enumerate()
                .filter(|(g, _)| !taken.contains(g))
                .filter(|(_, (_, a, t))| path.iter().any(|k| a.contains(k)) || t.iter().any(|v| values.contains(v)))
                .map(|(g, _)| g)
                .collect();
            if more.is_empty() {
                break;
            }
            for (c, a, _) in more.iter().filter_map(|g| groups.get(*g)) {
                closers.extend(c.iter().copied());
                around.extend(a.iter().copied());
                for v in unknowns(tree, a, asm, &linked, &Held::new()) {
                    add(&mut values, v);
                }
            }
            taken.extend(more);
        }
        // No loop reaches this occurrence's values (not even through a motion link it drives):
        // the tree's count stands.
        if taken.is_empty() {
            continue;
        }
        counted += 1;
        if counted > MAX_DOF_OCCURRENCES || spent > MAX_LOOP_WORK {
            return;
        }
        if values.len() > MAX_LOOP_VALUES {
            continue;
        }
        let scale = loop_scale(tree, &around, vals);
        let (n, gaps) = (values.len(), closers.len() * 9);
        // How many ways the occurrence moves at a pose, with the loops kept closed.
        let mut free_at = |at: &Values| -> Option<usize> {
            let mut work = at.clone();
            // The loops' gaps, then three points of the occurrence `scale` apart.
            let mut f = |x: &[f64]| -> Option<Vec<f64>> {
                spent += 1;
                for ((id, i), v) in values.iter().zip(x) {
                    if let Some(slot) = work.get_mut(id).and_then(|q| q.get_mut(*i)) {
                        *slot = *v;
                    }
                }
                apply_links(asm, &mut work);
                let mut out = Vec::with_capacity(closers.len() * 9 + 9);
                closure_gaps(tree, &closers, &work, scale, &mut out)?;
                let t = tree.world(*occ, &work)?;
                for p in [Vec3::ZERO, Vec3::X * scale, Vec3::Y * scale] {
                    let q = apply_point(&t, p);
                    out.extend([q.x, q.y, q.z]);
                }
                out.iter().all(|v| v.is_finite()).then_some(out)
            };
            let x0: Vec<f64> = values.iter().map(|(id, i)| at.get(id).and_then(|q| q.get(*i)).copied().unwrap_or(0.0)).collect();
            let jac = jacobian(&mut f, &x0, gaps + 9)?;
            let rank = |rows: usize| {
                let mut a = linalg::Mat::zeros(rows, n);
                let mut big = 0.0f64;
                for r in 0..rows {
                    for c in 0..n {
                        let v = jac.get(r * n + c).copied().unwrap_or(0.0);
                        big = big.max(v.abs());
                        a.set(r, c, v);
                    }
                }
                linalg::rref(&mut a, 1e-6 * big.max(1.0)).len()
            };
            Some(rank(gaps + 9).saturating_sub(rank(gaps)))
        };
        let here = free_at(vals);
        let there = near.as_ref().and_then(&mut free_at);
        let Some(free) = here.max(there) else { continue };
        dof.insert(*occ, free.min(6));
    }
}

/// Close one group of loops: move the values of the joints `around` them (all but the `held`
/// values, motion-link targets and rigid joints, within their limits; a linked value moves
/// through the start of its link chain) until every loop-closing joint meets. A loop open at the
/// start is closed first, from where it is. Then the held values `moves` go from the values they
/// are held from to their current ones in small steps, each solved from the last, so the mechanism
/// stays on its branch (an open four-bar does not flip crossed). Returns the values that changed
/// from what is stored, by (joint, index), and updates `vals` (empty when there is nothing to do
/// or the group is too big to solve), or `None` when the loops can't close, or can't stay closed
/// on the way.
fn close_group(
    tree: &Tree,
    closing: &[u64],
    around: &BTreeSet<u64>,
    moves: &[(u64, usize, f64, f64)],
    asm: &Assembly,
    held: &Held,
    linked: &BTreeSet<(u64, usize)>,
    spent: &std::cell::Cell<usize>,
    vals: &mut Values,
) -> Option<BTreeMap<(u64, usize), f64>> {
    let vars = unknowns(tree, around, asm, linked, held);
    if vars.len() > MAX_LOOP_VALUES {
        return Some(BTreeMap::new());
    }
    let bounds = limit_bounds(tree, &vars);
    let scale = loop_scale(tree, around, vals);
    // Closer than the conflict check needs: 1e-7 mm, and turns of 1e-10 rad across the loop.
    let tol = (1e-10 * scale).min(1e-7);
    // The values with the held ones `t` of the way along (and the unknowns at `x`).
    let place = |work: &mut Values, x: &[f64], t: f64| {
        for (id, i, from, to) in moves {
            if let Some(slot) = work.get_mut(id).and_then(|q| q.get_mut(*i)) {
                *slot = from + (to - from) * t;
            }
        }
        for ((id, i), (v, (lo, hi))) in vars.iter().zip(x.iter().zip(&bounds)) {
            if let Some(slot) = work.get_mut(id).and_then(|q| q.get_mut(*i)) {
                *slot = v.max(*lo).min(*hi);
            }
        }
        apply_links(asm, work);
    };
    let x0: Vec<f64> = vars.iter().map(|(id, i)| vals.get(id).and_then(|q| q.get(*i)).copied().unwrap_or(0.0)).collect();
    // Steps that move no value around the loops (links included) more than 5 degrees or a
    // twentieth of the loops' size.
    // How many such steps apart two sets of values are, over the loops' values.
    let stride = |a: &Values, b: &Values| -> f64 {
        let mut most = 0.0f64;
        for j in around.iter().filter_map(|id| tree.joints.get(id)) {
            for i in 0..j.kind.dofs().len() {
                let (Some(p), Some(q)) = (a.get(&j.id).and_then(|v| v.get(i)), b.get(&j.id).and_then(|v| v.get(i))) else { continue };
                let step = if j.kind.is_angle(i) { 5f64.to_radians() } else { 0.05 * scale };
                most = most.max((q - p).abs() / step);
            }
        }
        most
    };
    let effective = |x: &[f64], t: f64| {
        let mut v = vals.clone();
        place(&mut v, x, t);
        v
    };
    let mut first = 1.0f64;
    if !moves.is_empty() {
        first = first.min(1.0 / stride(&effective(&x0, 0.0), &effective(&x0, 1.0)));
    }
    let mut work = vals.clone();
    // How far each closing joint is from meeting with the held values `t` of the way along:
    // three points of B's joint frame, where the joint wants them against where the tree puts them.
    let mut gaps = |x: &[f64], t: f64| -> Option<Vec<f64>> {
        spent.set(spent.get() + 1);
        if spent.get() > MAX_LOOP_WORK {
            return None;
        }
        place(&mut work, x, t);
        let mut out = Vec::with_capacity(closing.len() * 9);
        closure_gaps(tree, closing, &work, scale, &mut out)?;
        out.iter().all(|v| v.is_finite()).then_some(out)
    };
    // Already closed, with no held value moving. (A moving one is followed all the way: a
    // rocker driven a full turn ends where it started, but can't get there.)
    if moves.is_empty() && gaps(&x0, 1.0).is_some_and(|r| largest(&r) <= tol) {
        return Some(BTreeMap::new());
    }
    // A loop open at the start closes first.
    let mut x = x0;
    if largest(&gaps(&x, 0.0)?) > tol && !least_squares(&mut |x| gaps(x, 0.0), &mut x, &bounds, tol) {
        return None;
    }
    let (mut t, mut dt) = (if moves.is_empty() { 1.0 } else { 0.0 }, first);
    let mut at = effective(&x, t);
    while t < 1.0 {
        if dt < first * 1e-4 {
            return None;
        }
        let t1 = (t + dt).min(1.0);
        let mut trial = x.clone();
        if least_squares(&mut |x| gaps(x, t1), &mut trial, &bounds, tol) {
            // A step that moved some loop value too far (through a link and a limit, or close to
            // a dead point) is taken again in halves.
            let next = effective(&trial, t1);
            if stride(&at, &next) > 2.0 {
                dt *= 0.5;
                continue;
            }
            (x, t, dt, at) = (trial, t1, (dt * 2.0).min(first), next);
        } else if spent.get() > MAX_LOOP_WORK {
            return None;
        } else {
            dt *= 0.5;
        }
    }
    if largest(&gaps(&x, 1.0)?) > tol {
        return None;
    }
    // The values that changed from what is stored.
    let moved = vars
        .iter()
        .zip(&x)
        .filter(|((id, i), v)| tree.joints.get(id).is_some_and(|j| j.values.get(*i).is_none_or(|s| (s - *v).abs() > 1e-12)))
        .map(|(var, v)| (*var, *v))
        .collect();
    *vals = work;
    Some(moved)
}

fn largest(r: &[f64]) -> f64 {
    r.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}

/// Levenberg–Marquardt: move `x` (kept within `bounds`) until every value of `f` is within
/// `tol` of zero. False when it can't get there.
/// The Jacobian of `f` (`m` values) at `x` by central differences, row-major.
fn jacobian(f: &mut dyn FnMut(&[f64]) -> Option<Vec<f64>>, x: &[f64], m: usize) -> Option<Vec<f64>> {
    const H: f64 = 1e-6;
    let n = x.len();
    let mut jac = vec![0.0; m * n];
    for k in 0..n {
        let (mut xp, mut xm) = (x.to_vec(), x.to_vec());
        if let (Some(p), Some(q)) = (xp.get_mut(k), xm.get_mut(k)) {
            *p += H;
            *q -= H;
        }
        let (rp, rm) = (f(&xp)?, f(&xm)?);
        for (i, (a, b)) in rp.iter().zip(&rm).enumerate().take(m) {
            if let Some(slot) = jac.get_mut(i * n + k) {
                *slot = (a - b) / (2.0 * H);
            }
        }
    }
    Some(jac)
}

fn least_squares(f: &mut dyn FnMut(&[f64]) -> Option<Vec<f64>>, x: &mut Vec<f64>, bounds: &[(f64, f64)], tol: f64) -> bool {
    let n = x.len();
    let norm = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    let Some(mut r) = f(x) else { return false };
    if n == 0 {
        return largest(&r) <= tol;
    }
    let mut lambda = 1e-3;
    for _ in 0..50 {
        if largest(&r) <= tol {
            return true;
        }
        // The Jacobian by central differences, then the normal equations JᵀJ δ = −Jᵀr.
        let m = r.len();
        let Some(jac) = jacobian(f, x, m) else { return false };
        let mut jtj = linalg::Mat::zeros(n, n);
        let mut g = vec![0.0; n];
        for (i, ri) in r.iter().enumerate() {
            let row = jac.get(i * n..(i + 1) * n).unwrap_or(&[]);
            for (p, a) in row.iter().enumerate().filter(|(_, a)| **a != 0.0) {
                if let Some(gp) = g.get_mut(p) {
                    *gp -= a * ri;
                }
                for (q, b) in row.iter().enumerate().skip(p) {
                    jtj.add(p, q, a * b);
                }
            }
        }
        for p in 0..n {
            for q in 0..p {
                jtj.set(p, q, jtj.get(q, p));
            }
        }
        let mut stepped = false;
        while lambda < 1e12 {
            let mut a = jtj.clone();
            for p in 0..n {
                a.add(p, p, lambda * (1.0 + jtj.get(p, p)));
            }
            if let Some(d) = linalg::solve(a, g.clone()) {
                let next: Vec<f64> = x.iter().zip(&d).zip(bounds).map(|((v, dv), (lo, hi))| (v + dv).max(*lo).min(*hi)).collect();
                if let Some(rn) = f(&next)
                    && norm(&rn) < norm(&r)
                {
                    *x = next;
                    r = rn;
                    lambda = (lambda / 3.0).max(1e-9);
                    stepped = true;
                    break;
                }
            }
            lambda *= 4.0;
        }
        if !stepped {
            return false;
        }
    }
    largest(&r) <= tol
}

/// Place occurrences from the joint graph.
pub fn solve(doc: &Document, st: &ModelState) -> Solution {
    solve_holding(doc, st, &BTreeMap::new())
}

/// Place occurrences from the joint graph. Joints that close loops are met by moving the values
/// of the other joints around the loop. The `held` values (the ones being driven) keep theirs,
/// moving there from the values given (where the loops last closed) so the others follow them.
pub fn solve_holding(doc: &Document, st: &ModelState, held: &Held) -> Solution {
    let asm = &doc.assembly;
    let mut sol = Solution::default();
    let mut vals = joint_values(asm);
    let Built { tree, mut placed, closing, mut dof } = build(doc, st, &vals, &mut sol);
    let mut stuck = BTreeSet::new();
    if !closing.is_empty() {
        (sol.values, stuck) = close_loops(&tree, &closing, asm, held, &mut vals);
        if !sol.values.is_empty() {
            placed = tree.place(&vals);
        }
    }
    for id in &closing {
        let Some(j) = tree.joints.get(id) else { continue };
        let (Some(ta), Some(tb)) = (placed.get(&j.a.occurrence), placed.get(&j.b.occurrence)) else { continue };
        let (Some(want), Some((_, fb))) = (tree.step(*id, true, ta, &vals), tree.frames.get(id)) else { continue };
        // Measured at the joint, where the loop solver closes it.
        let (dt, da) = mismatch(&mat_mul(&want, fb), &mat_mul(tb, fb));
        if dt > 1e-6 || da > 1e-8 {
            sol.conflicts.push((
                j.id,
                j.name.clone(),
                format!("off by {dt:.4} mm and {:.3}° from the placement the other joints give", da.to_degrees()),
            ));
        } else if stuck.contains(id) {
            // It meets there, but the loop would have to come apart on the way (a rocker
            // driven a full turn ends where it started).
            sol.conflicts.push((j.id, j.name.clone(), "the loop can't stay closed on the way to these values".into()));
        }
    }
    if !closing.is_empty() {
        loop_dof(&tree, &closing, asm, &vals, &mut dof);
    }
    for o in &doc.occurrences {
        dof.entry(o.id).or_insert(6);
    }
    placed.remove(&0);
    sol.world = placed;
    sol.dof = dof;
    sol
}

/// The joint graph laid out for a solve: the tree, where it places each occurrence for the
/// values it was built with, the joints that close loops, and each occurrence's degrees of
/// freedom along the tree.
struct Built<'a> {
    tree: Tree<'a>,
    placed: BTreeMap<u64, Mat>,
    closing: Vec<u64>,
    dof: BTreeMap<u64, usize>,
}

/// Lay out the joint graph: the spanning tree out from the fixed occurrences (the root, grounded
/// ones; with neither involved, the first joint's A side) and the joints left over, which close
/// loops. Joints whose origins can't be resolved go to `sol.errors`; `sol.fixed` is set.
fn build<'a>(doc: &'a Document, st: &ModelState, vals: &Values, sol: &mut Solution) -> Built<'a> {
    let asm = &doc.assembly;
    let joints: Vec<&Joint> = asm.joints.iter().filter(|j| !j.suppressed).take(MAX_JOINTS).collect();
    // Joint frames in component coordinates (errors leave the joint out).
    let mut frames: BTreeMap<u64, (Mat, Mat)> = BTreeMap::new();
    for j in &joints {
        match (resolve_origin(doc, st, &j.a), resolve_origin(doc, st, &j.b)) {
            (Ok(fa), Ok(fb)) => {
                frames.insert(j.id, (fa, fb));
            }
            (Err(e), _) | (_, Err(e)) => sol.errors.push((j.id, j.name.clone(), e.to_string())),
        }
    }
    // Fixed: the root, grounded occurrences; with neither involved, the first joint's A side.
    let mut placed: BTreeMap<u64, Mat> = BTreeMap::new();
    let mut dof: BTreeMap<u64, usize> = BTreeMap::new();
    placed.insert(0, IDENTITY);
    dof.insert(0, 0);
    for o in doc.occurrences.iter().filter(|o| o.grounded) {
        placed.insert(o.id, occurrence_world(doc, o.id));
        dof.insert(o.id, 0);
    }
    let involved: BTreeSet<u64> = joints.iter().flat_map(|j| [j.a.occurrence, j.b.occurrence]).collect();
    if !involved.iter().any(|o| placed.contains_key(o))
        && let Some(j) = joints.first()
    {
        placed.insert(j.a.occurrence, occurrence_world(doc, j.a.occurrence));
        dof.insert(j.a.occurrence, 0);
    }
    sol.fixed = placed.keys().copied().collect();
    let mut tree =
        // Every joint's values are known, suppressed ones too (their motion links still set values).
        Tree { joints: asm.joints.iter().map(|j| (j.id, j)).collect(), frames, fixed: placed.clone(), parent: BTreeMap::new(), order: Vec::new() };
    let mut used: BTreeSet<u64> = BTreeSet::new();
    let mut queue: VecDeque<u64> = placed.keys().copied().collect();
    // The joints at each occurrence, in the joints' order (a large assembly is not walked joint by
    // joint for every occurrence).
    let mut at: BTreeMap<u64, Vec<&Joint>> = BTreeMap::new();
    for j in joints.iter().copied() {
        at.entry(j.a.occurrence).or_default().push(j);
        if j.b.occurrence != j.a.occurrence {
            at.entry(j.b.occurrence).or_default().push(j);
        }
    }
    while let Some(occ) = queue.pop_front() {
        let Some(t) = placed.get(&occ).copied() else { continue };
        for j in at.get(&occ).map(Vec::as_slice).unwrap_or_default() {
            if used.contains(&j.id) || !tree.frames.contains_key(&j.id) {
                continue;
            }
            let (other, from_a) = if j.a.occurrence == occ {
                (j.b.occurrence, true)
            } else if j.b.occurrence == occ {
                (j.a.occurrence, false)
            } else {
                continue;
            };
            if placed.contains_key(&other) {
                continue;
            }
            let Some(m) = tree.step(j.id, from_a, &t, vals) else { continue };
            used.insert(j.id);
            placed.insert(other, m);
            tree.parent.insert(other, (occ, j.id, from_a));
            tree.order.push(other);
            dof.insert(other, dof.get(&occ).copied().unwrap_or(0) + j.kind.dofs().len());
            queue.push_back(other);
        }
    }
    // Joints that close loops: the solve moves the loops' values to meet them, then checks they
    // agree.
    let closing: Vec<u64> = joints
        .iter()
        .filter(|j| !used.contains(&j.id) && tree.frames.contains_key(&j.id))
        .filter(|j| placed.contains_key(&j.a.occurrence) && placed.contains_key(&j.b.occurrence))
        .map(|j| j.id)
        .collect();
    Built { tree, placed, closing, dof }
}

impl Document {
    /// Apply a solution: occurrence transforms (in their parents) for the occurrences it places,
    /// and the values of the joints that moved to close loops. Returns true when something moved.
    pub fn apply_joint_solution(&mut self, sol: &Solution) -> bool {
        let mut changed = false;
        for (id, q) in &sol.values {
            if let Some(j) = self.assembly.joints.iter_mut().find(|j| j.id == *id) {
                j.values.clone_from(q);
                changed = true;
            }
        }
        let parents: BTreeMap<u64, u64> = self.occurrences.iter().map(|o| (o.id, o.parent)).collect();
        for (occ, world) in &sol.world {
            let parent = parents.get(occ).copied().unwrap_or(0);
            let pw = if parent == 0 { IDENTITY } else { self.component_transform(parent) };
            let Some(local) = mat_inverse(&pw).map(|inv| mat_mul(&inv, world)) else { continue };
            if let Some(o) = self.occurrences.iter_mut().find(|o| o.id == *occ) {
                let (dt, da) = mismatch(&o.transform, &local);
                if dt > 1e-9 || da > 1e-10 {
                    o.transform = local;
                    changed = true;
                }
            }
        }
        changed
    }

    /// A frame for an as-built joint: the world frame `w` seen from an occurrence.
    pub fn frame_in_occurrence(&self, occ: u64, w: &Mat) -> Option<Snap> {
        let t = if occ == 0 { IDENTITY } else { occurrence_world(self, occ) };
        let local = mat_mul(&mat_inverse(&t)?, w);
        let o = apply_point(&local, Vec3::ZERO);
        let z = apply_vector(&local, Vec3::Z);
        let x = apply_vector(&local, Vec3::X);
        Some(Snap::Frame { origin: o, z, x: Some(x) })
    }

    /// A joint origin's frame in the world (for as-built joints and display).
    pub fn origin_world(&self, st: &ModelState, o: &JointOrigin) -> Result<Mat> {
        let f = resolve_origin(self, st, o)?;
        let t = if o.occurrence == 0 { IDENTITY } else { occurrence_world(self, o.occurrence) };
        Ok(mat_mul(&t, &f))
    }
}

#[path = "joints_cam.rs"]
pub mod cam;

#[path = "joints_reach.rs"]
mod reach;
pub use reach::{REACH_TOL, Reach, Reached, reach};

/// The frame matrix for an explicit origin / z / x (public for commands).
pub fn frame_of(o: Vec3, z: Vec3, x: Option<Vec3>) -> Option<Mat> {
    frame(o, z, x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Mat, b: &Mat) -> bool {
        let (t, r) = mismatch(a, b);
        t < 1e-9 && r < 1e-9
    }

    /// Revolute joints 1…n (joint 1 at `start`, the others at 0) linked 1 → 2 → … → n with
    /// ratio 2, the links stored from the end of the chain; joint 3 limited to ±50.
    fn chain(n: u64, start: f64) -> Assembly {
        let origin = JointOrigin { occurrence: 0, snap: Snap::Point { point: Vec3::ZERO, z: None } };
        let joints = (1..=n)
            .map(|id| Joint {
                id,
                name: format!("J{id}"),
                kind: JointKind::Revolute,
                a: origin.clone(),
                b: origin.clone(),
                values: vec![if id == 1 { start } else { 0.0 }],
                limits: vec![(id == 3).then_some((-50.0, 50.0))],
                flip: false,
                offset: 0.0,
                angle: 0.0,
                suppressed: false,
            })
            .collect();
        let links =
            (1..n).rev().map(|a| MotionLink { a, ia: 0, b: a + 1, ib: 0, ratio: 2.0, offset: 0.0, coupling: None, lift: Vec::new() }).collect();
        Assembly { joints, links, ..Assembly::default() }
    }

    /// Links carry a value down a chain stored from its end, each linked value within its limits
    /// as it is set, and applying them again changes nothing.
    #[test]
    fn links_settle_whatever_order_they_are_stored_in() {
        let asm = chain(8, 10.0);
        let vals = joint_values(&asm);
        let at = |id: u64| vals.get(&id).and_then(|q| q.first()).copied().unwrap_or(f64::NAN);
        // 10 → 20 → 40 (within joint 3's limit) → 80 → … → 1280.
        assert_eq!((at(1), at(2), at(3), at(4), at(8)), (10.0, 20.0, 40.0, 80.0, 1280.0));
        let asm = chain(8, 40.0);
        let vals = joint_values(&asm);
        let at = |id: u64| vals.get(&id).and_then(|q| q.first()).copied().unwrap_or(f64::NAN);
        // Joint 3 would be 160: held at 50, and what follows it follows the 50.
        assert_eq!((at(3), at(4), at(8)), (50.0, 100.0, 1600.0));
        let mut again = vals.clone();
        apply_links(&asm, &mut again);
        assert_eq!(again, vals);
    }

    /// A coupling saves as its definition: sizes as numbers, expressions or picked faces, flags
    /// only when set; a link without one (as every design before couplings has) reads as before.
    #[test]
    fn couplings_read_and_write_their_definition() {
        let old: MotionLink = serde_json::from_str(r#"{"a": 1, "b": 2, "ratio": 2}"#).unwrap();
        assert_eq!((old.coupling, old.offset, old.ia), (None, 0.0, 0));
        let text = r#"{"a": 1, "b": 2, "ratio": -0.5, "coupling": {"type": "gear", "ra": 10, "rb": "r2 * 2",
            "internal": true}}"#;
        let l: MotionLink = serde_json::from_str(text).unwrap();
        let Some(Coupling::Gear { ra, rb, internal, reverse }) = &l.coupling else { panic!("{l:?}") };
        assert_eq!((ra, rb, *internal, *reverse), (&Size::Length(10.0), &Size::Expr("r2 * 2".into()), true, false));
        let back = serde_json::to_value(&l).unwrap();
        assert_eq!(back["coupling"], serde_json::json!({"type": "gear", "ra": 10.0, "rb": "r2 * 2", "internal": true}));
        let picked: Size = serde_json::from_str(r#"{"occurrence": 3, "snap": {"type": "circle_center", "pick": [1, 0, 0]}}"#).unwrap();
        assert!(matches!(picked, Size::Circle(JointOrigin { occurrence: 3, snap: Snap::CircleCenter { .. } })));
        let screw: Coupling = serde_json::from_str(r#"{"type": "screw", "pitch": 1.5, "reverse": true}"#).unwrap();
        assert_eq!(screw, Coupling::Screw { pitch: Size::Length(1.5), reverse: true });
    }

    /// Ratios from the sizes: gears turn by their radii (the other way unless internal), a rack
    /// and a screw travel per radian whichever of the two joints drives.
    #[test]
    fn coupling_ratios_follow_their_sizes() {
        let mut doc = Document { assembly: chain(3, 0.0), ..Document::default() };
        if let Some(j) = doc.assembly.joints.get_mut(2) {
            j.kind = JointKind::Slider;
        }
        let st = ModelState::default();
        let ratio = |a: u64, b: u64, c: Coupling| {
            let l = MotionLink { a, ia: 0, b, ib: 0, ratio: 0.0, offset: 0.0, coupling: Some(c), lift: Vec::new() };
            coupling_values(&doc, &st, &l).map(|r| r.map(|(ratio, _)| ratio).map_err(|e| e.to_string()))
        };
        let len = Size::Length;
        assert_eq!(ratio(1, 2, Coupling::Gear { ra: len(10.0), rb: len(40.0), internal: false, reverse: false }), Some(Ok(-0.25)));
        assert_eq!(ratio(1, 2, Coupling::Gear { ra: len(10.0), rb: len(40.0), internal: true, reverse: false }), Some(Ok(0.25)));
        assert_eq!(ratio(1, 3, Coupling::RackPinion { radius: len(8.0), reverse: false }), Some(Ok(8.0)));
        assert_eq!(ratio(3, 1, Coupling::RackPinion { radius: len(8.0), reverse: true }), Some(Ok(-0.125)));
        let screw = ratio(1, 3, Coupling::Screw { pitch: len(std::f64::consts::TAU), reverse: false });
        assert_eq!(screw, Some(Ok(1.0)));
        assert!(matches!(ratio(1, 2, Coupling::RackPinion { radius: len(8.0), reverse: false }), Some(Err(_))));
        assert!(matches!(ratio(1, 3, Coupling::Gear { ra: len(1.0), rb: len(1.0), internal: false, reverse: false }), Some(Err(_))));
        for bad in [0.0, -2.0, f64::NAN, f64::INFINITY, 1e300] {
            assert!(matches!(ratio(1, 2, Coupling::Gear { ra: len(bad), rb: len(1.0), internal: false, reverse: false }), Some(Err(_))), "{bad}");
        }
        assert!(matches!(ratio(1, 2, Coupling::Gear { ra: Size::Expr("nope".into()), rb: len(1.0), internal: false, reverse: false }), Some(Err(_))));
    }

    #[test]
    fn motions_compose() {
        assert!(close(&motion(JointKind::Revolute, &[std::f64::consts::FRAC_PI_2]), &rot_z(std::f64::consts::FRAC_PI_2)));
        let m = motion(JointKind::Cylindrical, &[0.0, 5.0]);
        assert!((m[3][2] - 5.0).abs() < 1e-12);
        let m = motion(JointKind::PinSlot, &[0.0, 3.0]);
        assert!((m[3][0] - 3.0).abs() < 1e-12);
        assert_eq!(JointKind::Ball.dofs().len(), 3);
        assert!(JointKind::parse("Pin Slot").is_some() && JointKind::parse("nope").is_none());
        let f = frame(Vec3::new(1.0, 2.0, 3.0), Vec3::Z, None).unwrap();
        assert!(close(&mat_mul(&f, &mat_inverse(&f).unwrap()), &IDENTITY));
    }

    #[test]
    fn mismatch_resolves_tiny_turns() {
        let (_, a) = mismatch(&rot_z(1e-9), &IDENTITY);
        assert!((a - 1e-9).abs() < 1e-15, "{a}");
        let (_, a) = mismatch(&rot_z(3.0), &IDENTITY);
        assert!((a - 3.0).abs() < 1e-12, "{a}");
    }
}
