//! Command dialogs (shown at the right of the viewport). A dialog collects values and selection
//! inputs (geometry picked in the viewport, never typed), then runs its command; it never
//! changes the design itself. Things selected before the command starts become its input.

use std::hash::{Hash, Hasher};

use egui::{Color32, RichText, Stroke, vec2};
use serde_json::{Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::Session;
use solidcraft_engine::doc::expr::Kind as ValueKind;
use solidcraft_engine::doc::{AxisRef, Direction, FeatureKind, FilletStyle, HoleKind, Operation, PatternKind, PlaneRef, ProfileSel, ThinSide};
use solidcraft_engine::geom::{Aabb3, Vec3};

use crate::SolveApp;
use crate::selection::{self, AXES, Accept, BODIES, CURVES, EDGES, FACES, PLANAR_FACES, PLANES, PROFILES, SelInput};
use crate::theme::Tokens;
use crate::viewport::Hit;

pub(crate) const OPS: [&str; 4] = ["new", "join", "cut", "intersect"];
pub(crate) const OP_LABELS: [&str; 4] = ["New Body", "Join", "Cut", "Intersect"];
const DIRS: [&str; 4] = ["positive", "negative", "symmetric", "positive"];
/// The "Two sides" direction (a second distance the other way).
pub const TWO_SIDES: usize = 3;
/// Thin Extrude wall locations, as `solid.extrude` names them, and their labels.
const WALL_SIDES: [&str; 4] = ["center", "inside", "outside", "two_sides"];
const WALL_LABELS: [&str; 4] = ["Center", "Inside", "Outside", "Two Sides"];
/// What an extrude's first input picks: profiles or planar faces, or curves too when thin.
const EXTRUDE_ACCEPT: Accept = PROFILES | PLANAR_FACES;
const THIN_ACCEPT: Accept = PROFILES | CURVES;
const HOLE_TYPES: [&str; 3] = ["simple", "counterbore", "countersink"];
const HOLE_LABELS: [&str; 3] = ["Simple", "Counterbore", "Countersink"];

#[derive(Clone, Debug)]
pub enum Kind {
    /// Pick a plane or planar face; the sketch starts on it at once.
    Sketch,
    Extrude {
        distance: String,
        direction: usize,
        operation: usize,
        /// Into a body: cut; additions: join when a solid exists, otherwise new.
        /// until the user picks one.
        auto_op: bool,
        /// The direction from the picked side and the latched operation (#1257).
        smart: crate::smart_extrude::Smart,
        /// The other side's distance (two sides).
        distance2: String,
        /// Taper angle (empty or zero: none).
        taper: String,
        /// An explicitly authored zero taper is still a model parameter.
        taper_present: bool,
        /// Optional distance beyond the selected end object.
        to_offset: String,
        /// Start offset from the profile plane (empty: start on the profile plane).
        start: String,
        /// Extent type All (through everything) instead of a distance.
        all: bool,
        /// Thin Extrude: a wall along the picked curves (or profiles' outlines).
        thin: bool,
        /// The wall thickness (inside, with Two Sides).
        wall: String,
        /// Wall location: an index into [`WALL_SIDES`].
        wall_side: usize,
        /// The thickness outside, with Two Sides.
        wall2: String,
    },
    Revolve {
        angle: String,
        operation: usize,
        /// An arbitrary stored line until the user deliberately picks a replacement.
        axis: Option<Value>,
    },
    Fillet {
        radius: String,
        chamfer: bool,
        chain: bool,
        /// Chamfer type: 0 equal distance, 1 two distances, 2 distance and angle; with the
        /// second distance, the angle and which face the first distance is on.
        ctype: usize,
        distance2: String,
        angle: String,
        flip: bool,
    },
    Shell {
        thickness: String,
    },
    Draft {
        angle: String,
    },
    Mirror,
    /// Rectangular pattern of bodies: along one direction, optionally a second.
    PatternRect {
        count: String,
        spacing: String,
        count2: String,
        spacing2: String,
    },
    /// Circular pattern of bodies around an axis.
    PatternCirc {
        count: String,
        angle: String,
    },
    /// Loft through profiles of different sketches, in pick order.
    Loft {
        operation: usize,
    },
    /// Sweep a profile along a path of sketch curves.
    Sweep {
        operation: usize,
        opts: crate::dialogs_sweep::SweepOpts,
    },
    /// A construction plane offset from a plane.
    OffsetPlane {
        offset: String,
    },
    /// Halfway between two planes or planar faces.
    Midplane,
    /// A construction plane through an axis at an angle to a plane.
    AnglePlane {
        angle: String,
    },
    /// A plane perpendicular to one selected path at a fractional distance.
    PathPlane {
        t: String,
        sketch: Option<u64>,
    },
    /// Split a body by a plane.
    Split,
    /// Scale bodies uniformly.
    Scale {
        factor: String,
    },
    /// Move planar faces along their normals.
    OffsetFaces {
        distance: String,
    },
    /// A cosmetic thread on a cylindrical face.
    Thread {
        designation: String,
        length: String,
        /// The last automatic size; a different value is a manual override. Keep automatic
        /// features automatic when applying, even though their resolved size is visible.
        suggested: Option<String>,
        /// The picks used for the suggestion, so an unchanged dialog does no geometry work.
        of: Vec<Sel>,
    },
    /// A rib (one wall along open curves) or a web (a wall per curve).
    Rib {
        web: bool,
        thickness: String,
        depth: String,
        flip: bool,
        /// Grow along the sketch normal rather than across the plane.
        normal: bool,
    },
    /// Raise or sink profiles of a sketch on a face.
    Emboss {
        depth: String,
        deboss: bool,
    },
    /// Move planar faces onto a target plane or face.
    ReplaceFace,
    /// Boundary Fill: the cells the tool bodies make (a point inside, a description, filled
    /// or not), found again when the tools change.
    BoundaryFill {
        operation: usize,
        cells: Vec<(Vec3, String, bool)>,
        of: Vec<Sel>,
    },
    /// Move bodies so a face or point meets another.
    Align {
        flip: bool,
    },
    /// Remove bodies from here on in the timeline.
    Remove,
    /// Pattern bodies along a path of sketch curves.
    PathPattern {
        count: String,
        spacing: String,
    },
    /// A pipe (round, optionally hollow) along sketch curves.
    Pipe {
        diameter: String,
        wall: String,
    },
    /// A stock block around bodies.
    Stock {
        margin: String,
    },
    /// Physical material (and with it the appearance) of bodies.
    Material {
        index: usize,
    },
    /// Section Analysis: cut the view by a plane, moved along its normal.
    Section {
        offset: String,
        flip: bool,
    },
    /// Measure one or two picked items (vertices, edges, faces): the result of the last
    /// measurement and the items it was for.
    Measure {
        result: Option<Value>,
        of: Vec<Sel>,
        /// The measure chosen in the panel (centre, min or max); `None` follows the result's
        /// own default (centre where a round item gives one).
        show: Option<Shown>,
    },
    /// Move bodies by a distance along X, Y and Z.
    /// Move/Copy (see `dialogs_move`): the move type, X/Y/Z distances (Point to Position: the
    /// target), X/Y/Z angles (Free Move) and the angle (Rotate).
    Move {
        mode: usize,
        x: String,
        y: String,
        z: String,
        rx: String,
        ry: String,
        rz: String,
        angle: String,
    },
    Hole {
        diameter: String,
        depth: String,
        kind: usize,
        cb_diameter: String,
        cb_depth: String,
        cs_diameter: String,
        cs_angle: String,
        opts: HoleOpts,
    },
    Primitive {
        cmd: &'static str,
        fields: Vec<(&'static str, String)>,
        operation: usize,
        /// Where it goes: a picked point on a face or vertex, with the face's normal when the
        /// face is planar. `None` keeps the command's own placement (the origin, or the edited
        /// feature's).
        at: Option<(Vec3, Option<Vec3>)>,
    },
    Combine {
        operation: usize,
        keep_tools: bool,
    },
    Params {
        new_name: String,
        new_expr: String,
    },
    EditParam {
        name: String,
        expr: String,
    },
    Rename {
        feature: u64,
        name: String,
    },
    /// Delete a feature that others depend on.
    ConfirmDelete {
        features: Vec<u64>,
        with: Vec<String>,
        fail: Vec<String>,
    },
    /// Joints and components (`dialogs_assembly`).
    Assembly(crate::dialogs_assembly::Asm),
    /// Sheet metal (`dialogs_sheet`).
    Sheet(crate::dialogs_sheet::Sm),
    /// Surface tools and Split Face (`dialogs_surface`).
    Surface(crate::dialogs_surface::Surf),
    SaveMesh(crate::dialogs_save_mesh::SaveMesh),
    ViewDxf {
        dashed: bool,
        tolerance: f64,
    },
    /// Plastic features (`dialogs_plastic`).
    Plastic(crate::dialogs_plastic::Pl),
    /// Appearance (`dialogs_appearance`).
    Appearance(crate::dialogs_appearance::Ap),
    /// Contact sets, motion studies, exploded views, configurations (`dialogs_motion`).
    Motion(crate::dialogs_motion::Mo),
    /// Insert Part from the standard parts library (`dialogs_parts`).
    Part(crate::dialogs_parts::Pt),
    /// Edit Text: a sketch text's content and placement (`sketch_text`).
    Text(Box<crate::sketch_text::Tx>),
    TextMove(crate::sketch_text_move::TextMove),
    /// Sketch-space modify tools: scale and offset, and later fillet, chamfer, the patterns,
    /// mirror and move (`sketch_modify_ui`).
    SketchModify(crate::sketch_modify_ui::Mod),
}

/// The Single hole placement's input: places on planar faces. A pick on or near a sketch point
/// that lies on the face (a circle's centre) is taken at that point, as Multiple picks it.
/// Holes go on any face (flat or curved; the engine drills against the face's normal there).
const HOLE_FACES: Accept = PLANAR_FACES | FACES | selection::POINTS;

/// The rest of the Hole dialog: placement, extents, tap type and drill point.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleOpts {
    /// Placement at several sketch points instead of one face point.
    pub multiple: bool,
    /// Extents: through all instead of a distance.
    pub all: bool,
    /// Drill point: an angled tip instead of a flat bottom.
    pub angled: bool,
    pub tip_angle: String,
    /// Tap type: tapped (a cosmetic thread) instead of simple.
    pub tapped: bool,
    pub thread: String,
}

impl Default for HoleOpts {
    fn default() -> Self {
        HoleOpts { multiple: false, all: false, angled: true, tip_angle: "118 deg".into(), tapped: false, thread: "M5".into() }
    }
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub kind: Kind,
    /// Selection inputs, in order; clicks in the viewport go to the active one.
    pub inputs: Vec<SelInput>,
    pub active: usize,
    pub error: Option<String>,
    /// Editing an existing feature (its id, and the timeline marker to restore afterwards).
    pub editing: Option<(u64, Option<usize>)>,
    /// A user pick or clear makes the profile selector explicit, even if all are reselected.
    pub profile_selection_changed: bool,
    /// Give the on-canvas value box the keyboard (and select its text) on the next frame.
    pub focus: bool,
    /// Folded to its header (the "−" glyph).
    pub collapsed: bool,
    /// Parameters the dialog doesn't show but the command needs (kept when editing).
    pub extra: serde_json::Map<String, Value>,
    /// What the inputs were when OK was refused: `error` stays only while they still are.
    pub refused: Option<Refused>,
    /// Create Sketch opened by a sketch tool's shortcut: that tool starts once the sketch is open.
    pub then_start: Option<&'static str>,
}

/// OK computing off the window thread: the dialog's commands evaluated on a scratch copy of the
/// session, which leaves the feature in the evaluation cache the session shares. Once that is
/// in, OK runs them for real and finds the feature there; meanwhile the window stays live and
/// the dialog says it is computing.
pub struct Applying {
    pub(crate) done: std::sync::mpsc::Receiver<()>,
    /// When OK was pressed (ms).
    pub(crate) started: f64,
    /// The commands being computed.
    pub(crate) cmds: Vec<(String, Value)>,
}

impl Applying {
    /// The evaluation is in (or its worker ended).
    fn finished(&self) -> bool {
        !matches!(self.done.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty))
    }
}

/// How long the desktop app's OK waits for its evaluation before it lets the window go on and
/// shows that it is computing ([`SolveApp::apply_off_thread`]): a feature quicker than this
/// applies at once, as it always did.
pub const APPLY_AT_ONCE: std::time::Duration = std::time::Duration::from_millis(150);

/// Start OK's evaluation off the window thread, for a feature dialog. `None` when OK should
/// just run: not a feature, not asked for ([`SolveApp::apply_off_thread`]), no threads (the
/// web), or done within the wait.
#[cfg(not(target_arch = "wasm32"))]
fn begin_applying(app: &SolveApp, d: &Dialog) -> Option<Applying> {
    let wait = app.apply_off_thread?;
    if !d.previews() {
        return None;
    }
    let cmds = apply_commands(app, d).ok()?;
    let scratch = app.session.scratch();
    let (tx, rx) = std::sync::mpsc::channel();
    let computed = cmds.clone();
    std::thread::Builder::new()
        .name("apply".into())
        .spawn(move || {
            let _ = scratch.preview(&computed);
            let _ = tx.send(());
        })
        .ok()?;
    match rx.recv_timeout(wait) {
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Some(Applying { done: rx, started: crate::now_ms(), cmds }),
        _ => None,
    }
}

#[cfg(target_arch = "wasm32")]
fn begin_applying(_app: &SolveApp, _d: &Dialog) -> Option<Applying> {
    None
}

/// The commands a dialog's inputs make (or why they make none), its picks, and the design's
/// revision (a parameter the commands name can change without changing them).
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    commands: Result<Vec<(String, Value)>, String>,
    picks: Vec<SelInput>,
    revision: u64,
}

impl Refused {
    fn now(app: &SolveApp, d: &Dialog) -> Refused {
        Refused { commands: apply_commands(app, d), picks: d.inputs.clone(), revision: app.session.revision }
    }
}

/// Can a selection go into an input that accepts `a`? (Planar-ness is checked when picking.)
fn fits(a: Accept, s: &Sel) -> bool {
    match s {
        Sel::Profile { .. } => a & PROFILES != 0,
        Sel::Edge { .. } => a & EDGES != 0,
        Sel::Face { .. } => a & (FACES | PLANAR_FACES) != 0,
        Sel::Body { .. } => a & BODIES != 0,
        Sel::Plane { .. } => a & PLANES != 0,
        Sel::Axis { .. } => a & AXES != 0,
        Sel::SketchCurve { .. } => a & (AXES | CURVES) != 0,
        Sel::Vertex { .. } => a & selection::VERTICES != 0,
        Sel::Feature { .. } => a & selection::FEATURES != 0,
        // Sketch points go into a sketch command's own inputs (Entities, a base point), so a
        // box selection and a pre-selection reach them the way a click already does.
        Sel::SketchPoint { .. } => a & (selection::POINTS | selection::SKETCH_POINTS) != 0,
        Sel::SketchConstraint { .. } | Sel::Occurrence { .. } | Sel::ExternalSketchCurve { .. } | Sel::ExternalSketchPoint { .. } => false,
    }
}

impl Dialog {
    /// Show why OK (or starting the sketch) was refused, for as long as the inputs stay as they are.
    fn refuse(&mut self, app: &SolveApp, error: String) {
        self.refused = Some(Refused::now(app, self));
        self.error = Some(error);
    }

    pub(crate) fn new(kind: Kind, inputs: Vec<SelInput>) -> Dialog {
        Dialog {
            kind,
            inputs,
            active: 0,
            error: None,
            editing: None,
            profile_selection_changed: false,
            focus: true,
            collapsed: false,
            extra: serde_json::Map::new(),
            refused: None,
            then_start: None,
        }
    }

    pub fn for_command(app: &SolveApp, id: &str) -> Option<Dialog> {
        let s = &app.session;
        let has_bodies = !s.world_state().bodies.is_empty();
        let mut d = match id {
            "sketch.create" => Dialog::new(Kind::Sketch, vec![SelInput::new("Plane", PLANES | PLANAR_FACES, false)]),
            "solid.extrude" => Dialog::new(
                Kind::Extrude {
                    distance: "10 mm".into(),
                    direction: 0,
                    operation: usize::from(has_bodies),
                    auto_op: true,
                    smart: Default::default(),
                    distance2: "10 mm".into(),
                    taper: "0 deg".into(),
                    taper_present: false,
                    to_offset: String::new(),
                    start: String::new(),
                    all: false,
                    thin: false,
                    wall: "2 mm".into(),
                    wall_side: 0,
                    wall2: "1 mm".into(),
                },
                vec![SelInput::new("Profiles", EXTRUDE_ACCEPT, true)],
            ),
            "solid.revolve" => Dialog::new(
                Kind::Revolve { angle: "360 deg".into(), operation: 0, axis: None },
                vec![SelInput::new("Profile", PROFILES, true), SelInput::new("Axis", AXES, false)],
            ),
            "solid.fillet" => Dialog::new(fillet_kind("2 mm", false), vec![SelInput::new("Edges", EDGES | FACES, true)]),
            "solid.chamfer" => Dialog::new(fillet_kind("1 mm", true), vec![SelInput::new("Edges", EDGES | FACES, true)]),
            "solid.shell" => {
                let mut d = Dialog::new(Kind::Shell { thickness: "2 mm".into() }, vec![SelInput::new("Faces/Body", FACES, true)]);
                // Fusion opens faces joined smoothly to the picked ones too.
                d.extra.insert("tangent_chain".into(), json!(true));
                d
            }
            "solid.draft" => Dialog::new(
                Kind::Draft { angle: "5 deg".into() },
                vec![SelInput::new("Faces", FACES, true), SelInput::new("Neutral plane", PLANES | PLANAR_FACES, false)],
            ),
            "solid.mirror" => {
                Dialog::new(Kind::Mirror, vec![SelInput::new("Objects", BODIES, true), SelInput::new("Mirror Plane", PLANES | PLANAR_FACES, false)])
            }
            "solid.rib" | "solid.web" => Dialog::new(
                Kind::Rib { web: id == "solid.web", thickness: "2 mm".into(), depth: String::new(), flip: false, normal: false },
                vec![SelInput::new("Curves", CURVES, true)],
            ),
            "solid.emboss" => Dialog::new(
                Kind::Emboss { depth: "1 mm".into(), deboss: false },
                vec![SelInput::new("Profiles", PROFILES, true), SelInput::new("Target Face (optional)", FACES, false)],
            ),
            "solid.replace_face" => Dialog::new(
                Kind::ReplaceFace,
                vec![SelInput::new("Faces", PLANAR_FACES, true), SelInput::new("Target", PLANES | PLANAR_FACES, false)],
            ),
            "solid.boundary_fill" => {
                Dialog::new(Kind::BoundaryFill { operation: 0, cells: Vec::new(), of: Vec::new() }, vec![SelInput::new("Tools", BODIES, true)])
            }
            "solid.align" => Dialog::new(
                Kind::Align { flip: false },
                vec![
                    SelInput::new("Bodies", BODIES, true),
                    SelInput::new("From", FACES | EDGES | selection::VERTICES, false),
                    SelInput::new("To", FACES | EDGES | selection::VERTICES, false),
                ],
            ),
            "solid.remove" => Dialog::new(Kind::Remove, vec![SelInput::new("Bodies", BODIES, true)]),
            "solid.pattern.path" => Dialog::new(
                Kind::PathPattern { count: "4".into(), spacing: "20 mm".into() },
                vec![SelInput::new("Objects", BODIES, true), SelInput::new("Path", CURVES, true)],
            ),
            "solid.pipe" => Dialog::new(Kind::Pipe { diameter: "5 mm".into(), wall: String::new() }, vec![SelInput::new("Path", CURVES, true)]),
            "solid.bounding_solid" => Dialog::new(Kind::Stock { margin: "2 mm".into() }, vec![SelInput::new("Bodies (all if none)", BODIES, true)]),
            "construct.plane.offset" => {
                Dialog::new(Kind::OffsetPlane { offset: "10 mm".into() }, vec![SelInput::new("Plane", PLANES | PLANAR_FACES, false)])
            }
            "construct.plane.midplane" => Dialog::new(
                Kind::Midplane,
                vec![SelInput::new("First plane", PLANES | PLANAR_FACES, false), SelInput::new("Second plane", PLANES | PLANAR_FACES, false)],
            ),
            "construct.plane.along_path" => {
                Dialog::new(Kind::PathPlane { t: "0.5".into(), sketch: None }, vec![SelInput::new("Path", CURVES | EDGES, false)])
            }
            "construct.plane.at_angle" => Dialog::new(
                Kind::AnglePlane { angle: "45 deg".into() },
                vec![SelInput::new("Plane", PLANES, false), SelInput::new("Axis", AXES, false)],
            ),
            "solid.split_body" => Dialog::new(
                Kind::Split,
                vec![SelInput::new("Body to split", BODIES, false), SelInput::new("Splitting tool", PLANES | FACES | AXES, false)],
            ),
            "solid.scale" => Dialog::new(Kind::Scale { factor: "2".into() }, vec![SelInput::new("Bodies", BODIES, true)]),
            "solid.offset_face" => Dialog::new(Kind::OffsetFaces { distance: "2 mm".into() }, vec![SelInput::new("Faces", PLANAR_FACES, true)]),
            "solid.thread" => Dialog::new(
                Kind::Thread { designation: String::new(), length: String::new(), suggested: None, of: Vec::new() },
                vec![SelInput::new("Cylindrical face", FACES, false)],
            ),
            "material.assign" => Dialog::new(Kind::Material { index: 1 }, vec![SelInput::new("Bodies", BODIES, true)]),
            "inspect.section" => {
                Dialog::new(Kind::Section { offset: "0 mm".into(), flip: false }, vec![SelInput::new("Plane", PLANES | PLANAR_FACES, false)])
            }
            "inspect.measure" => Dialog::new(
                Kind::Measure { result: None, of: Vec::new(), show: None },
                vec![SelInput::new("Items", FACES | EDGES | selection::VERTICES | selection::CURVES | PROFILES, true)],
            ),
            "solid.pattern.rectangular" => Dialog::new(
                Kind::PatternRect { count: "3".into(), spacing: "20 mm".into(), count2: "1".into(), spacing2: "20 mm".into() },
                vec![
                    SelInput::new("Objects", BODIES, true),
                    SelInput::new("Direction", AXES | EDGES, false),
                    SelInput::new("Direction 2", AXES | EDGES, false),
                ],
            ),
            "solid.pattern.circular" => Dialog::new(
                Kind::PatternCirc { count: "6".into(), angle: "360 deg".into() },
                vec![SelInput::new("Objects", BODIES, true), SelInput::new("Axis", AXES, false)],
            ),
            "solid.loft" => Dialog::new(Kind::Loft { operation: usize::from(has_bodies) }, vec![SelInput::new("Profiles", PROFILES, true)]),
            "solid.sweep" => Dialog::new(Kind::Sweep { operation: usize::from(has_bodies), opts: crate::dialogs_sweep::SweepOpts::default() }, {
                let [rail, rail2] = crate::dialogs_sweep::rail_inputs();
                vec![
                    SelInput::new("Profile", PROFILES, true),
                    SelInput::new("Path", CURVES, true),
                    crate::dialogs_sweep::direction_input(),
                    rail,
                    rail2,
                ]
            }),
            "solid.move" => Dialog::new(crate::dialogs_move::kind(0), crate::dialogs_move::inputs(0)),
            "solid.hole" => Dialog::new(hole_defaults(), vec![SelInput::new("Face", HOLE_FACES, true)]),
            "solid.box" => Dialog::new(
                Kind::Primitive {
                    cmd: "solid.box",
                    fields: vec![("length", length_default(s, "20")), ("width", length_default(s, "20")), ("height", length_default(s, "20"))],
                    operation: 0,
                    at: None,
                },
                vec![],
            ),
            "solid.cylinder" => Dialog::new(
                Kind::Primitive {
                    cmd: "solid.cylinder",
                    fields: vec![("diameter", length_default(s, "20")), ("height", length_default(s, "20"))],
                    operation: 0,
                    at: None,
                },
                vec![],
            ),
            "solid.sphere" => Dialog::new(
                Kind::Primitive { cmd: "solid.sphere", fields: vec![("diameter", length_default(s, "20"))], operation: 0, at: None },
                vec![],
            ),
            "solid.torus" => Dialog::new(
                Kind::Primitive {
                    cmd: "solid.torus",
                    fields: vec![("major", length_default(s, "20")), ("minor", length_default(s, "5"))],
                    operation: 0,
                    at: None,
                },
                vec![],
            ),
            "solid.coil" => Dialog::new(
                Kind::Primitive {
                    cmd: "solid.coil",
                    fields: vec![
                        ("diameter", length_default(s, "40")),
                        ("revolutions", "4".into()),
                        ("pitch", length_default(s, "8")),
                        ("section_size", length_default(s, "3")),
                    ],
                    operation: 0,
                    at: None,
                },
                vec![],
            ),
            "solid.combine" => Dialog::new(
                Kind::Combine { operation: 1, keep_tools: false },
                vec![SelInput::new("Target Body", BODIES, false), SelInput::new("Tool Bodies", BODIES, true)],
            ),
            "parameters.change" => Dialog::new(Kind::Params { new_name: String::new(), new_expr: String::new() }, vec![]),
            _ => {
                let (kind, inputs) = crate::dialogs_assembly::start(app, id)
                    .or_else(|| crate::dialogs_sheet::start(app, id))
                    .or_else(|| crate::dialogs_surface::start(app, id))
                    .or_else(|| crate::dialogs_save_mesh::start(app, id))
                    .or_else(|| crate::dialogs_view_dxf::start(app, id))
                    .or_else(|| crate::dialogs_plastic::start(app, id))
                    .or_else(|| crate::dialogs_appearance::start(app, id))
                    .or_else(|| crate::dialogs_motion::start(app, id))
                    .or_else(|| crate::dialogs_parts::start(app, id))
                    .or_else(|| crate::sketch_text::start(app, id))
                    .or_else(|| crate::sketch_text_move::start(app, id))
                    .or_else(|| crate::sketch_modify_ui::start(app, id))?;
                Dialog::new(kind, inputs)
            }
        };
        // Patterns of features picked in the timeline: the objects are features.
        if is_pattern(&d.kind)
            && s.selection.iter().any(|x| matches!(x, Sel::Feature { .. }))
            && let Some(inp) = d.inputs.first_mut()
        {
            inp.accept = selection::FEATURES;
            inp.label = objects_label(inp.accept);
        }
        // Pre-selection: what is selected now becomes the input (first input that takes it); a
        // face or edge stands for its body where bodies are wanted. (Edit Text's selection is the
        // text it edits, not its path.)
        let preselection: &[Sel] = if matches!(d.kind, Kind::Text(_)) { &[] } else { &s.selection };
        for sel in preselection {
            let as_body = match sel {
                Sel::Face { body, .. } | Sel::Edge { body, .. } | Sel::Vertex { body, .. } => Some(Sel::Body { name: body.to_string() }),
                _ => None,
            };
            if let Some(b) = as_body
                && let Some(inp) = d.inputs.iter_mut().find(|i| i.accept == BODIES && (i.multi || i.items.is_empty()))
            {
                if !inp.items.contains(&b) {
                    inp.items.push(b);
                }
                continue;
            }
            // Planar-only inputs take planar faces only.
            let planar_ok = match sel {
                Sel::Face { body, index, .. } => planar_face(s, body, *index).is_some(),
                _ => true,
            };
            let takes = |i: &SelInput| {
                fits(i.accept, sel) && (i.multi || i.items.is_empty()) && (planar_ok || i.accept & FACES != 0 || !matches!(sel, Sel::Face { .. }))
            };
            if let Some(inp) = d.inputs.iter_mut().find(|i| takes(i)) {
                // Fillets and chamfers take an edge with its tangent chain, as a pick does.
                let picked = match (&d.kind, sel) {
                    (Kind::Fillet { chain: true, .. }, Sel::Edge { body, index, .. }) => s
                        .model
                        .state()
                        .body(body)
                        .map(|b| {
                            let m = b.mesh();
                            m.tangent_chain(*index, 2f64.to_radians())
                                .into_iter()
                                .filter_map(|i| m.edges.get(i).map(|e| Sel::Edge { body: body.clone(), index: i, point: polyline_mid(e) }))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_else(|| vec![sel.clone()]),
                    (Kind::Measure { .. }, _) => vec![crate::viewport::measurement_selection(s, sel)],
                    _ if inp.multi => crate::viewport::grouped_selection(s, sel),
                    _ => vec![sel.clone()],
                };
                for x in picked {
                    if !inp.items.iter().any(|y| selection::same_item(y, &x)) {
                        inp.items.push(x);
                    }
                }
            }
        }
        // A primitive goes where a face or vertex is picked, standing on a planar face.
        if let Kind::Primitive { at, .. } = &mut d.kind {
            *at = s.selection.iter().find_map(|sel| match sel {
                Sel::Face { body, index, point } => Some((*point, planar_face(s, body, *index).map(|(_, n)| n))),
                Sel::Vertex { point, .. } => Some((*point, None)),
                _ => None,
            });
        }
        // With nothing pre-selected, a single profile in the sketch being used is taken (#684:
        // not one a feature already builds from).
        if matches!(d.kind, Kind::Extrude { .. } | Kind::Revolve { .. })
            && let Some(inp) = d.inputs.first_mut()
            && inp.items.is_empty()
        {
            inp.items = default_profiles(s);
        }
        d.sync_thread(s);
        d.advance();
        Some(d)
    }

    /// Resolve a newly selected face without replacing an authored size or changing the
    /// document. Clearing the face also clears a suggestion that no longer applies.
    fn sync_thread(&mut self, s: &Session) {
        let Kind::Thread { designation, suggested, of, .. } = &mut self.kind else { return };
        if !designation.trim().is_empty() && suggested.as_deref() != Some(designation.as_str()) {
            return;
        }
        let picks = self.inputs.first().map(|i| i.items.as_slice()).unwrap_or_default();
        if picks == of.as_slice() {
            return;
        }
        *of = picks.to_vec();
        *suggested = picks.first().and_then(|sel| match sel {
            Sel::Face { point, .. } => solidcraft_engine::doc::suggested_thread(&s.world_state(), *point).ok(),
            _ => None,
        });
        *designation = suggested.clone().unwrap_or_default();
    }

    pub fn rename(feature: u64, name: &str) -> Dialog {
        Dialog::new(Kind::Rename { feature, name: name.to_string() }, vec![])
    }

    pub fn confirm_delete(feature: u64, with: Vec<String>, fail: Vec<String>) -> Dialog {
        Dialog::confirm_delete_many(vec![feature], with, fail)
    }

    pub fn confirm_delete_many(features: Vec<u64>, with: Vec<String>, fail: Vec<String>) -> Dialog {
        Dialog::new(Kind::ConfirmDelete { features, with, fail }, vec![])
    }

    pub fn edit_param(s: &Session, name: &str) -> Dialog {
        Dialog::new(Kind::EditParam { name: name.into(), expr: s.doc.param(name).map(|p| p.expr.clone()).unwrap_or_default() }, vec![])
    }

    /// Make the first single input still empty (after the filled ones) active.
    fn advance(&mut self) {
        if let Some(i) = self.inputs.iter().position(|i| i.items.is_empty()) {
            self.active = i;
        } else {
            self.active = self.active.min(self.inputs.len().saturating_sub(1));
        }
    }

    /// Does this dialog make a feature that can be previewed live?
    pub fn previews(&self) -> bool {
        match &self.kind {
            Kind::Assembly(k) => return k.previews(),
            Kind::Sheet(k) => return k.previews(),
            Kind::Surface(_) => return true,
            Kind::SketchModify(k) => return k.previews(),
            Kind::Plastic(k) => return k.previews(),
            Kind::Part(_) => return true,
            _ => {}
        }
        matches!(
            self.kind,
            Kind::Extrude { .. }
                | Kind::Revolve { .. }
                | Kind::Fillet { .. }
                | Kind::Shell { .. }
                | Kind::Draft { .. }
                | Kind::Mirror
                | Kind::Split
                | Kind::Rib { .. }
                | Kind::Emboss { .. }
                | Kind::ReplaceFace
                | Kind::Align { .. }
                | Kind::Remove
                | Kind::PathPattern { .. }
                | Kind::Pipe { .. }
                | Kind::Stock { .. }
                | Kind::Scale { .. }
                | Kind::OffsetFaces { .. }
                | Kind::PatternRect { .. }
                | Kind::PatternCirc { .. }
                | Kind::Loft { .. }
                | Kind::Sweep { .. }
                | Kind::PathPlane { .. }
                | Kind::Midplane
                | Kind::Move { .. }
                | Kind::Hole { .. }
                | Kind::Primitive { .. }
                | Kind::Combine { .. }
                | Kind::BoundaryFill { .. }
        )
    }

    /// The main value (shown on the canvas too): label, kind and the expression.
    pub fn primary(&mut self) -> Option<(&'static str, ValueKind, &mut String)> {
        Some(match &mut self.kind {
            Kind::Extrude { to_offset, .. } if to_object_on(&self.inputs) => ("To Offset", ValueKind::Length, to_offset),
            Kind::Extrude { distance, .. } => ("Distance", ValueKind::Length, distance),
            Kind::Revolve { angle, .. } => ("Angle", ValueKind::Angle, angle),
            Kind::Fillet { radius, chamfer, .. } => {
                let label = if *chamfer {
                    "Distance"
                } else if matches!(self.extra.get("type").and_then(Value::as_str), Some("curvature" | "g2")) {
                    "Equivalent Radius"
                } else {
                    "Radius"
                };
                (label, ValueKind::Length, radius)
            }
            Kind::Shell { thickness } => ("Thickness", ValueKind::Length, thickness),
            Kind::Draft { angle } => ("Angle", ValueKind::Angle, angle),
            Kind::Hole { diameter, .. } => ("Diameter", ValueKind::Length, diameter),
            // A move's main value follows its type: a distance along X for the ones that
            // translate, the angle for Rotate. Point to Point is two picks with nothing to type.
            Kind::Move { mode, x, angle, .. } => match *mode {
                crate::dialogs_move::ROTATE => ("Angle", ValueKind::Angle, angle),
                crate::dialogs_move::POINT_TO_POINT => return None,
                _ => ("X", ValueKind::Length, x),
            },
            Kind::PatternRect { spacing, .. } => ("Spacing", ValueKind::Length, spacing),
            Kind::PatternCirc { angle, .. } => ("Angle", ValueKind::Angle, angle),
            Kind::Section { offset, .. } => ("Distance", ValueKind::Length, offset),
            Kind::OffsetPlane { offset } => ("Distance", ValueKind::Length, offset),
            Kind::PathPlane { t, .. } => ("Position (0 to 1)", ValueKind::Unitless, t),
            Kind::Rib { thickness, .. } => ("Thickness", ValueKind::Length, thickness),
            Kind::Emboss { depth, .. } => ("Depth", ValueKind::Length, depth),
            Kind::PathPattern { spacing, .. } => ("Spacing", ValueKind::Length, spacing),
            Kind::Pipe { diameter, .. } => ("Diameter", ValueKind::Length, diameter),
            Kind::Stock { margin } => ("Margin", ValueKind::Length, margin),
            Kind::AnglePlane { angle } => ("Angle", ValueKind::Angle, angle),
            Kind::Scale { factor } => ("Scale", ValueKind::Unitless, factor),
            Kind::OffsetFaces { distance } => ("Distance", ValueKind::Length, distance),
            Kind::Sheet(k) => return k.primary(&self.inputs),
            Kind::Surface(k) => return k.primary(),
            Kind::Plastic(k) => return k.primary(),
            Kind::SketchModify(k) => return k.primary(),
            _ => return None,
        })
    }

    /// Dialogs holding a table get the wide limit.
    pub fn is_wide(&self) -> bool {
        matches!(self.kind, Kind::Params { .. } | Kind::SketchModify(crate::sketch_modify_ui::Mod::PatternRect { .. }))
            || (matches!(self.kind, Kind::PatternRect { .. }) && self.inputs.first().is_some_and(|i| i.accept == selection::FEATURES))
            || matches!(&self.kind, Kind::Motion(k) if k.wide())
    }

    pub fn wants_picks(&self) -> bool {
        !self.inputs.is_empty()
    }

    pub fn active_input(&self) -> Option<&SelInput> {
        self.inputs.get(self.active)
    }

    /// Everything the inputs hold (for highlighting).
    pub fn items(&self) -> Vec<Sel> {
        self.inputs.iter().flat_map(|i| i.items.iter().cloned()).collect()
    }

    pub fn highlight_key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(&self.items()).unwrap_or_default().hash(&mut h);
        self.active.hash(&mut h);
        h.finish()
    }

    /// The selection a hit would add to the active input.
    pub fn candidate(&self, s: &Session, h: &Hit) -> Option<Sel> {
        self.candidate_on(s, h, None)
    }

    /// The face under the cursor (the nearest face the pick ray meets, first among `hits`), looked
    /// up once for all the sketch points among `hits` when the active input snaps face places to
    /// them (the Single hole's); `None` otherwise.
    pub(crate) fn face_snap(&self, s: &Session, hits: &[Hit]) -> Option<FaceSnap> {
        if !self.active_input().is_some_and(|i| snaps_to_points(i.accept)) {
            return None;
        }
        let Some(Hit::Face { body, index, .. }) = hits.iter().find(|h| matches!(h, Hit::Face { .. })) else { return None };
        let st = s.world_state();
        let near: Vec<Vec3> = hits
            .iter()
            .filter_map(|h| match h {
                Hit::SketchPoint { sketch, at, .. } => st.sketch(*sketch).map(|ss| ss.plane.to_world(*at)),
                _ => None,
            })
            .collect();
        if near.is_empty() {
            return None;
        }
        Some(FaceSnap::new(s, body, *index, &near))
    }

    /// The selection a hit would add to the active input, given the face under the cursor
    /// ([`Dialog::face_snap`] of the same hits). A face place picked on or near a sketch point that
    /// lies on that face (a circle's centre) is taken at the point, as a points input picks it.
    pub(crate) fn candidate_on(&self, s: &Session, h: &Hit, snap: Option<&FaceSnap>) -> Option<Sel> {
        if matches!(self.kind, Kind::SketchModify(_)) {
            return crate::sketch_modify_ui::candidate(self, s, h);
        }
        let inp = self.active_input()?;
        // Features are also picked on the canvas: a face stands for the feature that made it.
        if inp.accept == selection::FEATURES {
            return match h {
                Hit::Face { body, index, point } => feature_of_face(s, body, *index, *point).map(|id| Sel::Feature { id }),
                _ => None,
            };
        }
        if snaps_to_points(inp.accept)
            && let Hit::SketchPoint { sketch, at, .. } = h
        {
            let snap = snap?;
            return snap.place_at(s.world_state().sketch(*sketch)?.plane.to_world(*at));
        }
        // The snap looked the face under the cursor up already, planar or not.
        inp.accepts(h, |body, face| snap.and_then(|f| f.planar(body, face)).unwrap_or_else(|| planar_face(s, body, face).is_some()))
    }

    /// A pick in the viewport (already accepted by the active input).
    pub fn pick(&mut self, s: &Session, sel: Sel) {
        if let Some(key) = feature_pattern_reference_key(self, self.active) {
            self.extra.remove(key);
        }
        if let Kind::PathPlane { sketch, .. } = &mut self.kind {
            *sketch = None;
            self.extra.remove("path");
        }
        if self.active == 0 && matches!(self.kind, Kind::Extrude { .. } | Kind::Revolve { .. } | Kind::Emboss { .. }) {
            self.profile_selection_changed = true;
        }
        let measure = matches!(self.kind, Kind::Measure { .. });
        let sel = if measure { crate::viewport::measurement_selection(s, &sel) } else { sel };
        let chain = matches!(self.kind, Kind::Fillet { chain: true, .. });
        let mut picked =
            if !measure && self.active_input().is_some_and(|i| i.multi) { crate::viewport::picked_selection(s, &sel) } else { vec![sel.clone()] };
        // Tangent chain: an edge brings the edges that continue it smoothly.
        if chain
            && let Sel::Edge { body, index, .. } = &sel
            && let Some(b) = s.world_state().body(body)
        {
            let m = b.mesh();
            picked = m
                .tangent_chain(*index, 2f64.to_radians())
                .into_iter()
                .filter_map(|i| m.edges.get(i).map(|e| Sel::Edge { body: body.clone(), index: i, point: polyline_mid(e) }))
                .collect();
        }
        // Extrude and revolve take profiles of one sketch.
        if let Sel::Profile { sketch, .. } = &sel
            && !matches!(self.kind, Kind::Loft { .. })
            && let Some(inp) = self.inputs.get_mut(self.active)
        {
            inp.items.retain(|x| !matches!(x, Sel::Profile { sketch: s2, .. } if s2 != sketch));
        }
        let Some(inp) = self.inputs.get_mut(self.active) else { return };
        inp.toggle(picked);
        self.focus = true;
        // A measurement is between two items: a third pick starts over from the last one.
        if matches!(self.kind, Kind::Measure { .. }) && inp.items.len() > 2 {
            let keep = inp.items.split_off(inp.items.len() - 1);
            inp.items = keep;
        }
        if !inp.multi && !inp.items.is_empty() && self.active + 1 < self.inputs.len() {
            self.advance();
        }
        self.error = None;
        self.sync_thread(s);
    }

    /// Box selection result: replace (or add to) the active input.
    pub fn take_box(&mut self, s: &Session, sels: Vec<Sel>, add: bool) {
        if let Kind::PathPlane { sketch, .. } = &mut self.kind {
            *sketch = None;
            self.extra.remove("path");
        }
        if self.active == 0 && matches!(self.kind, Kind::Extrude { .. } | Kind::Revolve { .. } | Kind::Emboss { .. }) {
            self.profile_selection_changed = true;
        }
        let measure = matches!(self.kind, Kind::Measure { .. });
        let Some(inp) = self.inputs.get_mut(self.active) else { return };
        if !add {
            inp.items.clear();
        }
        for x in sels {
            let x = if measure { crate::viewport::measurement_selection(s, &x) } else { x };
            if !fits(inp.accept, &x) {
                continue;
            }
            if !inp.multi {
                inp.items.clear();
                inp.items.push(x);
                break;
            }
            if !inp.items.iter().any(|y| selection::same_item(y, &x)) {
                inp.items.push(x);
            }
        }
        self.sync_thread(s);
    }
}

/// Profiles of the sketch a feature would use, when there is just one: the active sketch's, else
/// the last sketch's while no feature builds from it yet. A used profile (a cube's sketch on its
/// bottom face) is only taken when it is picked.
fn default_profiles(s: &Session) -> Vec<Sel> {
    let st = s.world_state();
    let sid = s.active_sketch.or_else(|| {
        s.doc.features.iter().rev().find(|f| matches!(f.kind, FeatureKind::Sketch { .. })).map(|f| f.id).filter(|&id| !profiles_used(s, id))
    });
    match sid.and_then(|id| st.sketch(id).map(|ss| (id, ss.profiles.len()))) {
        Some((id, 1)) => vec![Sel::Profile { sketch: id, index: 0 }],
        _ => Vec::new(),
    }
}

/// Does an input take face places that snap to sketch points on the face (the Single hole's)?
pub(crate) fn snaps_to_points(accept: Accept) -> bool {
    accept & selection::POINTS != 0 && accept & PLANAR_FACES != 0
}

/// Which of a sketch's points a face place snaps to (the Single hole's): the points of geometry
/// the sketch shows. Not generated offset nodes or a text outline's points, which are never drawn or picked on their own
/// (as `view::sketch_point_visible` has it), nor projected points while the sketch hides
/// projected geometry (Show Projected off), whose curves aren't picked either. Hidden sketches
/// offer no points at all. Show Points doesn't count: it only hides the markers of the sketch
/// being edited, and a finished sketch's circle centres are snapped to without markers.
pub(crate) struct SnapPoints<'a> {
    /// Show Projected is off: no linked point is taken.
    hide_linked: bool,
    shown: solidcraft_engine::view::SketchPointVisibility<'a>,
}

impl<'a> SnapPoints<'a> {
    pub(crate) fn of(sk: &'a solidcraft_engine::sketch::Sketch) -> Self {
        Self { hide_linked: sk.view.hide_projected, shown: solidcraft_engine::view::SketchPointVisibility::new(sk) }
    }

    /// Does a face place snap to point `p` of the sketch?
    pub(crate) fn takes(&self, p: &solidcraft_engine::sketch::SPoint) -> bool {
        self.shown.takes(p) && !(self.hide_linked && p.link.is_some())
    }
}

/// How near a sketch point must be to a face to lie on it: a micrometre, wherever the part is (a
/// planar face's triangles lie in its plane), by the true distance ([`triangle_dist`]).
const ON_FACE: f64 = 1e-3;

/// The distance from `p` to the triangle `abc`, exactly: from its plane when `p` is over it,
/// otherwise from its nearest side. (`preview::point_triangle_dist` takes a point just past a
/// side for one over the triangle, with an absolute tolerance that is wide for a sliver.)
fn triangle_dist(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f64 {
    let sides = [(a, b), (b, c), (c, a)];
    let to_sides = || sides.iter().map(|(u, v)| p.dist_to_segment(*u, *v)).fold(f64::INFINITY, f64::min);
    // However small the triangle: only one without area (or not finite) has no plane.
    let n = (b - a).cross(c - a);
    let len = n.len();
    if !(len > 0.0 && len.is_finite()) {
        return to_sides();
    }
    let unit = n / len;
    let h = (p - a).dot(unit);
    let q = p - unit * h;
    if sides.iter().all(|(u, v)| (*v - *u).cross(q - *u).dot(unit) >= 0.0) { h.abs() } else { to_sides() }
}

/// The most point-in-triangle tests one pick makes for the snap. Points past it aren't snapped
/// to (the face as picked still is), so no sketch, however dense, can stall the viewport.
const SNAP_BUDGET: usize = 1 << 18;

#[cfg(test)]
thread_local! {
    /// A smaller [`SNAP_BUDGET`] for a test that runs it out.
    static TEST_BUDGET: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Run the snap with a budget of `n` tests per pick on this thread (`None`: [`SNAP_BUDGET`]).
#[cfg(test)]
pub(crate) fn set_snap_budget(n: Option<usize>) {
    TEST_BUDGET.with(|b| b.set(n));
}

fn snap_budget() -> usize {
    #[cfg(test)]
    if let Some(n) = TEST_BUDGET.with(std::cell::Cell::get) {
        return n;
    }
    SNAP_BUDGET
}

/// The face under the cursor, looked up once for all the sketch points one pick offers a face
/// place to (the Single hole's snap): whether it is planar, its plane, and those of its
/// triangles near the points on that plane, with their bounding boxes. A dense sketch over a
/// finely tessellated part costs one planarity lookup and one pass over the body's triangles per
/// pick; then a point off the plane is turned down at once, and a point on it is tested against
/// the face's triangles near the cursor, within [`SNAP_BUDGET`] tests in all.
pub(crate) struct FaceSnap {
    body: String,
    face: usize,
    /// A point of the plane, its unit normal, and how far a point on the face may be from it
    /// (the triangles' own distance plus [`ON_FACE`]); `None` when the face isn't planar.
    plane: Option<(Vec3, Vec3, f64)>,
    tris: Vec<([Vec3; 3], Aabb3)>,
    /// Point-in-triangle tests left for this pick.
    budget: std::cell::Cell<usize>,
}

impl FaceSnap {
    /// Face `face` of `body`, for sketch points at `near` (in the world).
    fn new(s: &Session, body: &str, face: usize, near: &[Vec3]) -> FaceSnap {
        let mut snap = FaceSnap { body: body.to_string(), face, plane: None, tris: Vec::new(), budget: std::cell::Cell::new(snap_budget()) };
        let st = s.world_state();
        let (Some((origin, normal)), Some(b)) = (planar_face(s, body, face), st.body(body)) else { return snap };
        let m = b.mesh();
        #[cfg(test)]
        SNAP_WORK.with(|w| w.set((w.get().0, w.get().1 + m.triangles.len())));
        let tris: Vec<[Vec3; 3]> = m.triangles.iter().zip(&m.tri_face).filter(|(_, f)| **f as usize == face).filter_map(|(t, _)| m.tri(t)).collect();
        // Anything within ON_FACE of a triangle is within this of the plane.
        let stray = tris.iter().flatten().map(|v| (*v - origin).dot(normal).abs()).fold(0.0, f64::max);
        let plane = (origin, normal, stray + ON_FACE);
        let mut region = Aabb3::EMPTY;
        for p in near.iter().filter(|p| near_plane(plane, **p)) {
            region.add(*p);
        }
        snap.tris = tris
            .into_iter()
            .map(|t| {
                let mut b = Aabb3::EMPTY;
                t.iter().for_each(|v| b.add(*v));
                (t, b)
            })
            .filter(|(_, b)| boxes_meet(b, &region, ON_FACE))
            .collect();
        snap.plane = Some(plane);
        snap
    }

    /// Is face `face` of `body` planar, when it is this snap's face?
    fn planar(&self, body: &str, face: usize) -> Option<bool> {
        (self.face == face && self.body == body).then_some(self.plane.is_some())
    }

    /// The face place at `p` (one of the points the snap was made for), when `p` lies on the face.
    fn place_at(&self, p: Vec3) -> Option<Sel> {
        let plane = self.plane?;
        if !(p.is_finite() && near_plane(plane, p)) {
            return None;
        }
        let at = Aabb3 { min: p, max: p };
        for ([a, b, c], bb) in &self.tris {
            let left = self.budget.get();
            if left == 0 {
                return None;
            }
            self.budget.set(left - 1);
            #[cfg(test)]
            SNAP_WORK.with(|w| w.set((w.get().0, w.get().1 + 1)));
            if boxes_meet(bb, &at, ON_FACE) && triangle_dist(p, *a, *b, *c) < ON_FACE {
                return Some(Sel::Face { body: self.body.clone(), index: self.face, point: p });
            }
        }
        None
    }
}

/// Is `p` within reach of the plane (a point of it, unit normal, reach)?
fn near_plane((origin, normal, reach): (Vec3, Vec3, f64), p: Vec3) -> bool {
    (p - origin).dot(normal).abs() <= reach
}

/// Do boxes `a` and `b` meet once `b` is grown by `pad` on every side?
fn boxes_meet(a: &Aabb3, b: &Aabb3, pad: f64) -> bool {
    a.min.x <= b.max.x + pad
        && b.min.x - pad <= a.max.x
        && a.min.y <= b.max.y + pad
        && b.min.y - pad <= a.max.y
        && a.min.z <= b.max.z + pad
        && b.min.z - pad <= a.max.z
}

#[cfg(test)]
thread_local! {
    /// Planar face lookups (each tessellates the body) and triangles the snap looked at, on this
    /// thread: tests bound the work of a pick with them.
    static SNAP_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// (planar face lookups, triangles the snap looked at) on this thread since the last call.
#[cfg(test)]
pub(crate) fn take_snap_work() -> (usize, usize) {
    SNAP_WORK.with(|w| w.replace((0, 0)))
}

#[cfg(test)]
mod face_snap_tests {
    use super::*;

    #[test]
    fn a_point_more_than_a_micrometre_off_a_sliver_is_off_the_face() {
        // A triangle with half-micrometre legs. Points 2 µm below its lower side and 1.6 µm past
        // its long side get past `preview::point_triangle_dist`'s inside test (an absolute
        // tolerance); the snap goes by the true distance and turns them down. Points on a side,
        // or half a micrometre beyond one, are on the face.
        let (a, b, c) = (Vec3::ZERO, Vec3::new(5e-4, 0.0, 0.0), Vec3::new(0.0, 5e-4, 0.0));
        let out = Vec3::new(1.0, 1.0, 0.0) * std::f64::consts::FRAC_1_SQRT_2;
        let mid = Vec3::new(2.5e-4, 2.5e-4, 0.0);
        let off = [Vec3::new(2.5e-4, -2e-3, 0.0), Vec3::new(1.4e-3, 1.4e-3, 0.0)];
        let on = [mid, mid + out * 5e-4, Vec3::new(1e-4, -5e-4, 0.0), Vec3::new(1e-4, 1e-4, 5e-4)];
        let mut bb = Aabb3::EMPTY;
        [a, b, c].into_iter().for_each(|v| bb.add(v));
        let snap = FaceSnap {
            body: "Body1".into(),
            face: 0,
            plane: Some((Vec3::ZERO, Vec3::Z, ON_FACE)),
            tris: vec![([a, b, c], bb)],
            budget: std::cell::Cell::new(SNAP_BUDGET),
        };
        for p in off {
            assert!(crate::preview::point_triangle_dist(p, a, b, c) < ON_FACE, "the inside test alone takes {p:?}");
            assert!(triangle_dist(p, a, b, c) > 1.5e-3, "{p:?}");
            assert_eq!(snap.place_at(p), None, "{p:?}");
        }
        for p in on {
            assert_eq!(snap.place_at(p), Some(Sel::Face { body: "Body1".into(), index: 0, point: p }), "{p:?}");
        }
    }

    #[test]
    fn the_distance_to_a_tiny_triangle_is_from_its_plane_over_it() {
        // Legs of 20 nm: the triangle still has a plane, and a point just under a micrometre
        // above it is that far from it (not from its nearest side), so it is on the face. A
        // triangle without area is measured from its sides; nothing here panics on non-finite input.
        let (a, b, c) = (Vec3::ZERO, Vec3::new(2e-5, 0.0, 0.0), Vec3::new(0.0, 2e-5, 0.0));
        let p = Vec3::new(5e-6, 5e-6, 0.99999e-3);
        assert!((triangle_dist(p, a, b, c) - 0.99999e-3).abs() < 1e-12, "{}", triangle_dist(p, a, b, c));
        assert!((triangle_dist(Vec3::new(0.0, 3.0, 4.0), a, b, b) - 5.0).abs() < 1e-9, "a triangle without area");
        let far = |d: f64| d.is_nan() || d >= ON_FACE;
        assert!(far(triangle_dist(Vec3::new(f64::NAN, 0.0, 0.0), a, b, c)), "a point that isn't finite is never near");
        assert!(far(triangle_dist(p, a, b, Vec3::new(f64::INFINITY, 0.0, 0.0))), "nor is a triangle that isn't finite");
    }
}

/// Does a feature build from this sketch's profiles (extrude, revolve, sweep, loft, emboss…)?
fn profiles_used(s: &Session, sketch: u64) -> bool {
    s.doc.features.iter().any(|f| f.kind.profile_sels().iter().any(|(id, _)| *id == sketch))
}

/// The plane of a planar body face: (point on it, unit normal).
pub fn planar_face(s: &Session, body: &str, face: usize) -> Option<(Vec3, Vec3)> {
    #[cfg(test)]
    SNAP_WORK.with(|w| w.set((w.get().0 + 1, w.get().1)));
    let st = s.world_state();
    let b = st.body(body)?;
    let tol = (b.body.size() * 1e-3).max(1e-3);
    let f = b.body.faces(tol).ok()?.into_iter().find(|f| f.index == face)?;
    Some((f.centroid, f.plane_normal?))
}

fn polyline_mid(pts: &[Vec3]) -> Vec3 {
    let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    let mut acc = 0.0;
    for w in pts.windows(2) {
        let l = w[0].dist(w[1]);
        if acc + l >= len * 0.5 && l > 0.0 {
            return w[0].lerp(w[1], (len * 0.5 - acc) / l);
        }
        acc += l;
    }
    pts.first().copied().unwrap_or_default()
}

fn pt(v: Vec3) -> Value {
    json!([v.x, v.y, v.z])
}

/// A plane input as a command parameter: an origin/construction plane name or a face plane.
fn plane_value(s: &Session, sel: Option<&Sel>) -> Option<Value> {
    match sel? {
        Sel::Plane { name } => Some(json!(name)),
        Sel::Face { body, index, point } => {
            let (_, n) = planar_face(s, body, *index)?;
            Some(json!({"origin": pt(*point), "normal": pt(n)}))
        }
        _ => None,
    }
}

/// A value field; true when Enter was pressed in it.
pub(crate) fn field(ui: &mut egui::Ui, s: &mut String) -> bool {
    let r = ui.text_edit_singleline(s);
    publish_row(&r);
    crate::params_dialog::complete(ui, &r, s);
    r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// A checkbox row. Its row handle (`dialog:<row label>`) lets a scenario click it by the label
/// beside it, the way `field` already publishes one for a value field.
pub(crate) fn check(ui: &mut egui::Ui, on: &mut bool) {
    let r = ui.checkbox(on, "");
    publish_row(&r);
}

pub(crate) fn combo(ui: &mut egui::Ui, id: &str, labels: &[&str], sel: &mut usize) {
    let row = ROW.with(|r| r.borrow().clone());
    let selected = labels.get(*sel).copied().map(tr).unwrap_or("");
    let r = egui::ComboBox::from_id_salt(id).selected_text(selected).width(FIELD_W).show_ui(ui, |ui| {
        for (i, l) in labels.iter().enumerate() {
            let item = ui.selectable_value(sel, i, tr(l));
            crate::scenario::publish_handle(&format!("dialog:{row}:{l}"), item.rect.center());
        }
    });
    publish_row(&r.response);
}

/// A mode choice as a row of icon buttons rather than a drop-down: every mode is on screen at
/// once, each drawn as the shape it makes, with its name as the tooltip. `labels` and `icons`
/// run in step; a mode with no icon gets the fallback square. Publishes the handles a combo
/// does, `dialog:<row label>:<label>` per mode and `dialog:<row label>` for the row.
pub(crate) fn icon_choice(ui: &mut egui::Ui, labels: &[&str], icons: &[&str], sel: &mut usize) {
    let t = Tokens::get();
    let row = ROW.with(|r| r.borrow().clone());
    let mut at = None;
    ui.horizontal(|ui| {
        for (i, label) in labels.iter().enumerate() {
            let on = *sel == i;
            let (r, resp) = ui.allocate_exact_size(vec2(28.0, 24.0), egui::Sense::click());
            if on {
                ui.painter().rect_filled(r, 4.0, t.accent);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, t.hover);
            }
            let ink = if on { Color32::WHITE } else { t.icon };
            crate::icons::paint(ui.painter(), r.shrink(3.0), icons.get(i).copied().unwrap_or(""), ink, t.icon_fill, ink);
            crate::scenario::publish_handle(&format!("dialog:{row}:{label}"), r.center());
            if on {
                at = Some(r.center());
            }
            if resp.on_hover_text(tr(label)).clicked() {
                *sel = i;
            }
        }
    });
    if let Some(p) = at {
        crate::scenario::publish_handle(&format!("dialog:{row}"), p);
    }
}

thread_local! {
    /// The label of the dialog row being laid out (its widgets publish `dialog:<label>` handles).
    static ROW: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn tr(text: &str) -> &str {
    crate::i18n::tr_label(text)
}

/// A dialog row's widget, for scenarios and demos: the handle `dialog:<row label>`.
fn publish_row(r: &egui::Response) {
    ROW.with(|row| crate::scenario::publish_handle(&format!("dialog:{}", row.borrow()), r.rect.center()));
}

fn title(k: &Kind) -> &'static str {
    match k {
        Kind::Sketch => "CREATE SKETCH",
        Kind::Extrude { .. } => "EXTRUDE",
        Kind::Revolve { .. } => "REVOLVE",
        Kind::Fillet { chamfer: false, .. } => "FILLET",
        Kind::Fillet { chamfer: true, .. } => "CHAMFER",
        Kind::Shell { .. } => "SHELL",
        Kind::Draft { .. } => "DRAFT",
        Kind::Mirror => "MIRROR",
        Kind::Measure { .. } => "MEASURE",
        Kind::Material { .. } => "PHYSICAL MATERIAL",
        Kind::OffsetPlane { .. } => "OFFSET PLANE",
        Kind::Midplane => "MIDPLANE",
        Kind::PathPlane { .. } => "PLANE ALONG PATH",
        Kind::Rib { web: false, .. } => "RIB",
        Kind::Rib { web: true, .. } => "WEB",
        Kind::Emboss { .. } => "EMBOSS",
        Kind::ReplaceFace => "REPLACE FACE",
        Kind::Align { .. } => "ALIGN",
        Kind::BoundaryFill { .. } => "BOUNDARY FILL",
        Kind::Remove => "REMOVE",
        Kind::PathPattern { .. } => "PATTERN ON PATH",
        Kind::Pipe { .. } => "PIPE",
        Kind::Stock { .. } => "STOCK",
        Kind::AnglePlane { .. } => "PLANE AT ANGLE",
        Kind::Split => "SPLIT BODY",
        Kind::Scale { .. } => "SCALE",
        Kind::OffsetFaces { .. } => "OFFSET FACE",
        Kind::Thread { .. } => "THREAD",
        Kind::Section { .. } => "SECTION ANALYSIS",
        Kind::PatternRect { .. } => "RECTANGULAR PATTERN",
        Kind::PatternCirc { .. } => "CIRCULAR PATTERN",
        Kind::Loft { .. } => "LOFT",
        Kind::Sweep { .. } => "SWEEP",
        Kind::Move { .. } => "MOVE",
        Kind::Hole { .. } => "HOLE",
        Kind::Primitive { cmd, .. } => match *cmd {
            "solid.box" => "BOX",
            "solid.cylinder" => "CYLINDER",
            "solid.sphere" => "SPHERE",
            "solid.coil" => "COIL",
            _ => "TORUS",
        },
        Kind::Combine { .. } => "COMBINE",
        Kind::Params { .. } => "PARAMETERS",
        Kind::EditParam { .. } => "EDIT DIMENSION",
        Kind::Rename { .. } => "RENAME",
        Kind::ConfirmDelete { .. } => "DELETE FEATURE",
        Kind::Assembly(k) => k.title(),
        Kind::Sheet(k) => k.title(),
        Kind::Surface(k) => k.title(),
        Kind::SaveMesh(_) => "SAVE AS MESH",
        Kind::ViewDxf { .. } => "Export Current View as DXF",
        Kind::Plastic(k) => k.title(),
        Kind::Appearance(_) => "APPEARANCE",
        Kind::Motion(k) => k.title(),
        Kind::Part(_) => "INSERT PART",
        Kind::Text(_) => "EDIT TEXT",
        Kind::TextMove(_) => "MOVE/COPY TEXT",
        Kind::SketchModify(k) => k.title(),
    }
}

fn hint(inp: &SelInput) -> &'static str {
    let a = inp.accept;
    if a & PROFILES != 0 {
        "click profiles in the view"
    } else if a & EDGES != 0 {
        "click edges or faces"
    } else if a & PLANES != 0 {
        "click a plane or planar face"
    } else if a & AXES != 0 {
        "click an axis or sketch line"
    } else if a & CURVES != 0 {
        "click sketch curves"
    } else if a & (selection::POINTS | selection::SKETCH_POINTS) != 0 {
        "click sketch points"
    } else if a & BODIES != 0 {
        "click bodies"
    } else {
        "click faces"
    }
}

/// Width of the label column; longer labels are cut with "…".
const LABEL_W: f32 = 92.0;
/// The label column grows to fit its labels up to this, then long labels wrap.
const LABEL_MAX_W: f32 = 150.0;

/// A row label in the dialog's label column (the grid sizes the column to its widest label).
pub(crate) fn row_label(ui: &mut egui::Ui, text: &str) {
    ROW.with(|r| *r.borrow_mut() = text.to_string());
    let display_text = tr(text);
    let full = egui::WidgetText::from(display_text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body).size().x;
    let w = full.clamp(LABEL_W, LABEL_MAX_W);
    ui.allocate_ui_with_layout(vec2(w, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_width(w);
        ui.add(egui::Label::new(display_text).wrap());
    });
}

thread_local! {
    /// Faces already traced to their feature, by (revision, body, face).
    static FACE_FEATURE: std::cell::RefCell<(u64, std::collections::HashMap<(String, usize), Option<u64>>)> = Default::default();
}

/// The feature that made a face: the latest feature before which the face's point lay on no
/// face of the model.
fn feature_of_face(s: &Session, body: &str, index: usize, p: Vec3) -> Option<u64> {
    let key = (body.to_string(), index);
    if let Some(hit) = FACE_FEATURE.with(|c| {
        let c = c.borrow();
        if c.0 == s.revision { c.1.get(&key).copied() } else { None }
    }) {
        return hit;
    }
    let on_model = |st: &solidcraft_engine::doc::ModelState| {
        st.bodies.iter().any(|b| {
            let m = b.mesh();
            let bb = m.bounds();
            let tol = 1e-4 * (1.0 + bb.diagonal());
            p.x >= bb.min.x - tol
                && p.y >= bb.min.y - tol
                && p.z >= bb.min.z - tol
                && p.x <= bb.max.x + tol
                && p.y <= bb.max.y + tol
                && p.z <= bb.max.z + tol
                && m.triangles.iter().filter_map(|t| m.tri(t)).any(|t| point_tri_dist(p, t) < tol)
        })
    };
    let found = s
        .doc
        .features
        .iter()
        .rev()
        .filter(|f| !matches!(f.kind, FeatureKind::Sketch { .. }) && s.model.result(f.id).is_some_and(|r| r.error.is_none()))
        .find(|f| !on_model(&s.model.state_before(f.id)))
        .map(|f| f.id);
    FACE_FEATURE.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != s.revision {
            *c = (s.revision, std::collections::HashMap::new());
        }
        c.1.insert(key, found);
    });
    found
}

/// Distance from a point to a triangle.
pub(crate) fn point_tri_dist(p: Vec3, [a, b, c]: [Vec3; 3]) -> f64 {
    let n = (b - a).cross(c - a);
    let nn = n.dot(n);
    if nn > 1e-24 {
        let q = p - n * ((p - a).dot(n) / nn);
        let inside = [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).cross(q - *u).dot(n) >= 0.0);
        if inside {
            return (p - q).len();
        }
    }
    p.dist_to_segment(a, b).min(p.dist_to_segment(b, c)).min(p.dist_to_segment(c, a))
}

/// The pattern objects' label: features are picked in the timeline.
fn objects_label(accept: Accept) -> &'static str {
    if accept == selection::FEATURES { "Features" } else { "Objects" }
}

fn is_pattern(k: &Kind) -> bool {
    matches!(k, Kind::PatternRect { .. } | Kind::PatternCirc { .. } | Kind::PathPattern { .. } | Kind::Mirror)
}

/// The Object Type row of a pattern: bodies (picked in the view) or features (picked in the
/// timeline, e.g. a hole).
fn object_type_row(d: &mut Dialog, ui: &mut egui::Ui) {
    let Some(inp) = d.inputs.first_mut() else { return };
    row_label(ui, "Object Type");
    let cur = usize::from(inp.accept == selection::FEATURES);
    let mut ty = cur;
    combo(ui, "pat_obj", &["Bodies", "Features"], &mut ty);
    ui.end_row();
    if ty != cur {
        inp.accept = if ty == 1 { selection::FEATURES } else { BODIES };
        inp.label = objects_label(inp.accept);
        inp.items.clear();
        d.active = 0;
    }
}

/// Rows above the selection inputs: the pattern type and object type, the Hole placement (one
/// face point, or sketch points).
fn placement_row(d: &mut Dialog, ui: &mut egui::Ui) {
    if matches!(d.kind, Kind::Mirror) {
        object_type_row(d, ui);
        return;
    }
    if matches!(d.kind, Kind::Move { .. }) {
        crate::dialogs_move::type_row(d, ui);
        return;
    }
    let current = match d.kind {
        Kind::PatternRect { .. } => Some(0),
        Kind::PatternCirc { .. } => Some(1),
        Kind::PathPattern { .. } => Some(2),
        _ => None,
    };
    if let Some(cur) = current {
        row_label(ui, "Type");
        let mut ty = cur;
        combo(ui, "pat_type", &["Rectangular", "Circular", "Path"], &mut ty);
        ui.end_row();
        if ty != cur {
            // Same objects, the new pattern's other inputs.
            let objects = d.inputs.first().map(|i| i.items.clone()).unwrap_or_default();
            let (kind, mut inputs) = match ty {
                0 => (
                    Kind::PatternRect { count: "3".into(), spacing: "20 mm".into(), count2: "1".into(), spacing2: "20 mm".into() },
                    vec![
                        SelInput::new("Objects", BODIES, true),
                        SelInput::new("Direction", AXES | EDGES, false),
                        SelInput::new("Direction 2", AXES | EDGES, false),
                    ],
                ),
                1 => (
                    Kind::PatternCirc { count: "6".into(), angle: "360 deg".into() },
                    vec![SelInput::new("Objects", BODIES, true), SelInput::new("Axis", AXES, false)],
                ),
                _ => (
                    Kind::PathPattern { count: "4".into(), spacing: "20 mm".into() },
                    vec![SelInput::new("Objects", BODIES, true), SelInput::new("Path", CURVES, true)],
                ),
            };
            let accept = d.inputs.first().map(|i| i.accept).unwrap_or(BODIES);
            if let Some(i) = inputs.first_mut() {
                i.items = objects;
                i.accept = accept;
                i.label = objects_label(accept);
            }
            d.kind = kind;
            d.inputs = inputs;
            d.extra.clear();
            d.advance();
        }
        object_type_row(d, ui);
        return;
    }
    let Kind::Hole { opts, .. } = &mut d.kind else { return };
    row_label(ui, "Placement");
    let mut m = usize::from(opts.multiple);
    combo(ui, "hole_place", &["Single", "Multiple"], &mut m);
    ui.end_row();
    if (m == 1) != opts.multiple {
        opts.multiple = m == 1;
        d.inputs = vec![if opts.multiple { SelInput::new("Points", selection::POINTS, true) } else { SelInput::new("Face", HOLE_FACES, true) }];
        d.active = 0;
        d.extra.remove("direction");
    }
}

fn selection_label(s: &Session, item: &Sel) -> String {
    match item {
        Sel::SketchCurve { id } => id.clone(),
        Sel::SketchPoint { id } => crate::sketch_modify_ui::point_id(s, item).unwrap_or(id).to_string(),
        Sel::Plane { name } | Sel::Axis { name } | Sel::Body { name } => name.clone(),
        Sel::Face { body, index, .. } => format!("{body} · face {index}"),
        Sel::Edge { body, index, .. } => format!("{body} · edge {index}"),
        Sel::Feature { id } => s.doc.feature(*id).map(|f| f.name.clone()).unwrap_or_else(|| format!("Feature {id}")),
        Sel::Vertex { body, point } if body.is_empty() => {
            let state = s.world_state();
            let p = s.active_sketch.and_then(|id| state.sketch(id)).map(|sk| sk.plane.to_local(*point)).unwrap_or_default();
            format!("({:.2}, {:.2}) mm", p.x, p.y)
        }
        _ => format!("{item:?}"),
    }
}

/// Selection input rows: label, "N selected" (or a hint) and a clear button; clicking a row
/// makes it the active input.
fn input_rows(d: &mut Dialog, ui: &mut egui::Ui, s: &Session) {
    input_rows_range(d, ui, s, 0..d.inputs.len());
}

/// Saved vectors/axes are real inputs even when they have no origin-axis selection chip.
fn feature_pattern_reference_key(d: &Dialog, index: usize) -> Option<&'static str> {
    if !d.inputs.first().is_some_and(|i| i.accept == selection::FEATURES) {
        return None;
    }
    match (&d.kind, index) {
        (Kind::PatternRect { .. }, 1) => Some("dir1"),
        (Kind::PatternRect { .. }, 2) => Some("dir2"),
        (Kind::PatternCirc { .. }, 1) => Some("axis"),
        _ => None,
    }
}

fn input_rows_range(d: &mut Dialog, ui: &mut egui::Ui, s: &Session, range: std::ops::Range<usize>) {
    // Edit Text shows its Path input among its own rows.
    if matches!(d.kind, Kind::Text(_)) {
        return;
    }
    let t = Tokens::get();
    let mut clear: Option<usize> = None;
    let mut activate: Option<usize> = None;
    for (i, inp) in d.inputs.iter().enumerate().skip(range.start).take(range.end.saturating_sub(range.start)) {
        let stored_reference = feature_pattern_reference_key(d, i).and_then(|key| d.extra.get(key)).filter(|_| inp.items.is_empty());
        row_label(ui, inp.label);
        ui.horizontal(|ui| {
            let active = i == d.active;
            // A chip: "↖ Select" (outlined) while empty, "N selected" (filled) once picked.
            let custom_axis = i == 1 && inp.items.is_empty() && matches!(d.kind, Kind::Revolve { axis: Some(_), .. });
            let filled = !inp.items.is_empty() || custom_axis || stored_reference.is_some();
            let text = if filled {
                RichText::new(if custom_axis || stored_reference.is_some() {
                    crate::i18n::tr("dialog.custom_axis").into()
                } else if !inp.multi {
                    inp.items.first().map(|item| selection_label(s, item)).unwrap_or_default()
                } else {
                    crate::i18n::format("dialog.selected_count", &[("count", &inp.items.len().to_string())])
                })
                .color(Color32::WHITE)
            } else {
                RichText::new(crate::i18n::tr("dialog.select")).color(t.text)
            };
            let stroke = if active { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, t.dialog_border.gamma_multiply(1.6)) };
            // Not in the Tab order: Tab moves between values.
            let b = egui::Button::new(text)
                .wrap_mode(egui::TextWrapMode::Wrap)
                .fill(if filled { t.chip } else { Color32::TRANSPARENT })
                .stroke(stroke)
                .min_size(vec2(0.0, 22.0))
                .sense(egui::Sense::CLICK);
            let chip = crate::frame::add_outlined(ui, b, stroke.width);
            publish_row(&chip);
            let tooltip = if let Some(reference) = stored_reference {
                reference.to_string()
            } else if custom_axis {
                if let Kind::Revolve { axis: Some(axis), .. } = &d.kind {
                    format!(
                        "Custom axis\nOrigin: {}\nDirection: {}\nClick to select a replacement axis",
                        axis.get("origin").map(ToString::to_string).unwrap_or_default(),
                        axis.get("dir").map(ToString::to_string).unwrap_or_default()
                    )
                } else {
                    hint(inp).to_string()
                }
            } else {
                format!("{} (click to pick into this input)", hint(inp))
            };
            if chip.on_hover_text(tooltip).clicked() {
                activate = Some(i);
            }
            if !inp.items.is_empty() || stored_reference.is_some() {
                let button = ui.small_button("×").on_hover_text(tr("Clear the selection"));
                crate::scenario::publish_handle(&format!("dialog:clear:{}", inp.label), button.rect.center());
                if button.clicked() {
                    clear = Some(i);
                }
            }
        });
        ui.end_row();
    }
    if let Some(i) = activate {
        d.active = i;
    }
    if let Some(i) = clear {
        if let Some(key) = feature_pattern_reference_key(d, i) {
            d.extra.remove(key);
        }
        let Some(inp) = d.inputs.get_mut(i) else { return };
        inp.items.clear();
        if i == 0 {
            d.profile_selection_changed = true;
        }
        d.active = i;
    }
    d.sync_thread(s);
}

/// Each live reference sits with the independent values it drives.
fn rectangular_pattern_rows(d: &mut Dialog, ui: &mut egui::Ui, session: &Session) -> bool {
    use crate::sketch_modify_ui::Mod;
    use solidcraft_engine::sketch::PatternAlignment;
    let mut enter = false;
    ui.vertical(|ui| {
        ui.label(tr("dialog.selection_guidance"));
        egui::Grid::new("pattern_source").spacing(vec2(10.0, 8.0)).show(ui, |ui| {
            if matches!(d.kind, Kind::PatternRect { .. }) {
                placement_row(d, ui);
            }
            input_rows_range(d, ui, session, 0..1);
        });
        ui.separator();
        ui.columns(2, |columns| {
            for (index, ui) in columns.iter_mut().enumerate() {
                ui.strong(tr(if index == 0 { "Direction 1" } else { "Direction 2" }));
                egui::Grid::new(("pattern_direction", index)).spacing(vec2(8.0, 8.0)).show(ui, |ui| {
                    input_rows_range(d, ui, session, index + 1..index + 2);
                    if let Kind::PatternRect { count, spacing, count2, spacing2 } = &mut d.kind {
                        let (count, spacing, count_label, spacing_label) =
                            if index == 0 { (count, spacing, "Quantity", "Spacing") } else { (count2, spacing2, "Quantity 2", "Spacing 2") };
                        row_label(ui, count_label);
                        enter |= field(ui, count);
                        ui.end_row();
                        row_label(ui, spacing_label);
                        enter |= field(ui, spacing);
                        ui.end_row();
                        feature_pattern_alignment_row(ui, &mut d.extra, index);
                        return;
                    }
                    let Kind::SketchModify(Mod::PatternRect { count, spacing, count2, spacing2, alignment, alignment2, reversed, reversed2 }) =
                        &mut d.kind
                    else {
                        return;
                    };
                    let (count, spacing, alignment, reversed, count_label, spacing_label, alignment_label) = if index == 0 {
                        (count, spacing, alignment, reversed, "Quantity", "Spacing", "Alignment")
                    } else {
                        (count2, spacing2, alignment2, reversed2, "Quantity 2", "Spacing 2", "Alignment 2")
                    };
                    row_label(ui, count_label);
                    enter |= field(ui, count);
                    ui.end_row();
                    row_label(ui, spacing_label);
                    enter |= field(ui, spacing);
                    ui.end_row();
                    row_label(ui, alignment_label);
                    ui.horizontal(|ui| {
                        pattern_toggle(ui, alignment_label, "Flip direction", "pattern_flip", reversed);
                        let mut centered = *alignment == PatternAlignment::Center;
                        pattern_toggle(ui, alignment_label, "Center", "pattern_center", &mut centered);
                        *alignment = if centered { PatternAlignment::Center } else { PatternAlignment::Right };
                    });
                    ui.end_row();
                });
            }
        });
        ui.small(tr("Counts include the original. Center balances copies; Flip chooses the extra side for even totals."));
    });
    ui.end_row();
    enter
}

/// Feature patterns use the same source-fixed slot ordering and controls as sketch patterns.
fn feature_pattern_alignment_row(ui: &mut egui::Ui, extra: &mut serde_json::Map<String, Value>, index: usize) {
    use solidcraft_engine::sketch::PatternAlignment;
    let (label, alignment_key, reversed_key) =
        if index == 0 { ("Alignment", "alignment", "reversed") } else { ("Alignment 2", "alignment2", "reversed2") };
    let alignment = extra.get(alignment_key).cloned().and_then(|v| serde_json::from_value::<PatternAlignment>(v).ok()).unwrap_or_default();
    let mut reversed = extra.get(reversed_key).and_then(Value::as_bool).unwrap_or(false);
    // Retain programmatic Left placement until the user changes its alignment.
    let mut centered = alignment == PatternAlignment::Center;
    let was_centered = centered;
    row_label(ui, label);
    ui.horizontal(|ui| {
        pattern_toggle(ui, label, "Flip direction", "pattern_flip", &mut reversed);
        pattern_toggle(ui, label, "Center", "pattern_center", &mut centered);
    });
    extra.insert(reversed_key.into(), json!(reversed));
    if centered != was_centered {
        extra.insert(alignment_key.into(), json!(if centered { PatternAlignment::Center } else { PatternAlignment::Right }));
    }
    ui.end_row();
}

pub(crate) fn pattern_toggle(ui: &mut egui::Ui, row: &str, label: &str, icon: &str, selected: &mut bool) {
    // Tab moves between value fields; Enter after a value edit applies the dialog.
    let response = ui.add(egui::Button::new("").selected(*selected).min_size(vec2(28.0, 24.0)).sense(egui::Sense::CLICK));
    let t = Tokens::get();
    crate::icons::paint(ui.painter(), response.rect.shrink(3.0), icon, t.icon, t.icon_fill, t.accent);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, *selected, tr(label)));
    crate::scenario::publish_handle(&format!("dialog:{row}:{label}"), response.rect.center());
    if response.on_hover_text(tr(label)).clicked() {
        *selected = !*selected;
    }
}

fn error_rows(app: &mut SolveApp, ctx: &egui::Context, d: &mut Dialog, ui: &mut egui::Ui, full_width: bool) {
    if matches!(d.kind, Kind::SketchModify(_))
        && d.previews()
        && let Err(error) = apply_commands(app, d)
    {
        d.refuse(app, error);
    }
    if let Some(error) = d.error.as_ref().or(app.preview.error.as_ref()).cloned() {
        if !full_width {
            row_label(ui, "");
        }
        ui.vertical(|ui| {
            ui.add(egui::Label::new(RichText::new(error.as_str()).color(Tokens::get().error)).wrap());
            let response = ui.button("Create Issue…");
            crate::scenario::publish_handle("dialog:create_issue", response.rect.center());
            if response.clicked() {
                crate::issue_report::begin(app, ctx, Some(d), Some(&error));
            }
        });
        ui.end_row();
    }
}

/// Width limits of a docked dialog: shrink-wrapped to its content within these.
use crate::frame::{DIALOG_MAX_W, DIALOG_MIN_W, DIALOG_WIDE_MAX_W};
/// Width of value fields and choice boxes in a dialog.
pub(crate) const FIELD_W: f32 = 124.0;

/// Find the Boundary Fill cells again when the tool bodies changed (keeping the choices of
/// cells that are still there).
fn update_fill_cells(app: &SolveApp, d: &mut Dialog) {
    let tools: Vec<Sel> = sels(d, 0).to_vec();
    let Kind::BoundaryFill { cells, of, .. } = &mut d.kind else { return };
    if *of == tools {
        return;
    }
    let names: Vec<String> = tools.iter().filter_map(|x| if let Sel::Body { name } = x { Some(name.clone()) } else { None }).collect();
    // Editing: the tools as they were before this feature (it may have removed them).
    let st = match d.editing {
        Some((id, _)) => app.session.model.state_before(id),
        None => app.session.world_state(),
    };
    let found = fill_cells(&st, &names);
    let kept: Vec<String> = cells.iter().filter(|c| c.2).map(|c| c.1.clone()).collect();
    *cells = found.into_iter().map(|(p, label)| (p, label.clone(), kept.is_empty() || kept.contains(&label))).collect();
    *of = tools;
}

/// The cells a set of tool bodies makes: each region inside some of them and outside the rest,
/// as a point inside it and which tools hold it. Found by sampling a grid over the tools.
pub(crate) fn fill_cells(st: &solidcraft_engine::doc::ModelState, tools: &[String]) -> Vec<(Vec3, String)> {
    let bodies: Vec<_> = tools.iter().filter_map(|n| st.body(n).map(|b| (n.clone(), b.mesh()))).take(16).collect();
    if bodies.is_empty() {
        return Vec::new();
    }
    let indexes: Vec<_> = bodies.iter().map(|(_, m)| m.inside_index()).collect();
    let bb = bodies.iter().fold(solidcraft_engine::geom::Aabb3::EMPTY, |b, (_, m)| b.union(&m.bounds()));
    let n = 20;
    let at = |i: usize, lo: f64, hi: f64| lo + (hi - lo) * (i as f64 + 0.5) / n as f64;
    // Signature (which tools hold the point) → the points with it.
    let mut groups: Vec<(u32, Vec<Vec3>)> = Vec::new();
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                let p = Vec3::new(at(i, bb.min.x, bb.max.x), at(j, bb.min.y, bb.max.y), at(k, bb.min.z, bb.max.z));
                let sig = indexes.iter().enumerate().fold(0u32, |acc, (t, ix)| if ix.contains(p) { acc | (1 << t) } else { acc });
                if sig == 0 {
                    continue;
                }
                match groups.iter_mut().find(|(g, _)| *g == sig) {
                    Some((_, v)) => v.push(p),
                    None => groups.push((sig, vec![p])),
                }
            }
        }
    }
    groups.sort_by_key(|(sig, _)| *sig);
    groups
        .into_iter()
        .filter_map(|(sig, pts)| {
            let c = pts.iter().fold(Vec3::ZERO, |a, p| a + *p) * (1.0 / pts.len() as f64);
            // The sample nearest the middle (the middle itself may be outside a curved cell).
            let rep = pts.iter().min_by(|a, b| a.dist(c).total_cmp(&b.dist(c))).copied()?;
            let inside: Vec<&str> = bodies.iter().enumerate().filter(|(t, _)| sig & (1 << t) != 0).map(|(_, (name, _))| name.as_str()).collect();
            Some((rep, format!("Inside {}", inside.join(", "))))
        })
        .collect()
}

/// Measure again when the picked items changed.
fn update_measure(app: &SolveApp, d: &mut Dialog) {
    let items: Vec<Sel> = sels(d, 0).to_vec();
    if let Kind::Measure { result, of, .. } = &mut d.kind
        && *of != items
    {
        *result = if items.is_empty() { None } else { Some(app.session.measure_items(&items)) };
        *of = items;
    }
}

fn fmt_mm(app: &SolveApp, v: &Value) -> String {
    v.as_f64().map(|x| crate::prefs::show_mm(app, x, 1)).unwrap_or_default()
}

/// Which distance of a pair the Measure panel shows (and the canvas draws).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// Centre to centre, or from a round item's axis.
    Centre,
    /// Closest points.
    Min,
    /// Farthest points.
    Max,
}

fn vec3_of(v: &Value) -> Option<Vec3> {
    let a = v.as_array()?;
    Some(Vec3::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?))
}

/// The distance shown for a measured pair: which one, its value and its two ends. The chosen
/// one when the result has it, otherwise the result's default (centre, else the closest).
pub(crate) fn shown_distance(r: &Value, show: Option<Shown>) -> Option<(Shown, f64, Vec3, Vec3)> {
    let get = |which: Shown| -> Option<(Shown, f64, Vec3, Vec3)> {
        let (d, from, to) = match which {
            Shown::Centre => ("center_distance", "center_from", "center_to"),
            Shown::Min => ("distance_mm", "from", "to"),
            Shown::Max => ("max", "max_from", "max_to"),
        };
        Some((which, r.get(d)?.as_f64()?, vec3_of(r.get(from)?)?, vec3_of(r.get(to)?)?))
    };
    let default = if r.get("measure").and_then(Value::as_str) == Some("center") { Shown::Centre } else { Shown::Min };
    show.and_then(get).or_else(|| get(default)).or_else(|| get(Shown::Min))
}

/// The measurement in the dialog.
fn measure_rows(app: &SolveApp, ui: &mut egui::Ui, r: Option<&Value>, show: &mut Option<Shown>) {
    let t = Tokens::get();
    let Some(r) = r else {
        ui.label("");
        ui.label(RichText::new(tr("pick one or two vertices, edges or faces")).color(t.text_dim));
        ui.end_row();
        return;
    };
    if let Some(e) = r.get("error").and_then(Value::as_str) {
        ui.label("");
        ui.label(RichText::new(e).color(t.error));
        ui.end_row();
        return;
    }
    let mm = |v: &Value| fmt_mm(app, v);
    for (i, it) in r["items"].as_array().into_iter().flatten().enumerate() {
        // Round items: Ø first, then R, then the length (circumference, or the arc's length
        // and angle).
        let mut lines: Vec<String> = Vec::new();
        if it.get("diameter").is_some() {
            lines.push(format!("Ø  {}", mm(&it["diameter"])));
            lines.push(format!("R  {}", mm(&it["radius"])));
            if it.get("circumference").is_some() {
                lines.push(format!("{}  {}", tr("Circumference"), mm(&it["circumference"])));
            } else if let Some(a) = it["angle_deg"].as_f64() {
                lines.push(format!("{}  {}", tr("Arc length"), mm(&it["arc_length"])));
                lines.push(format!("{}  {a:.2}°", crate::i18n::tr_label("Angle")));
            }
        }
        if let Some(sl) = it.get("slot") {
            lines.push(format!("{}  {}", tr("Slot width"), mm(&sl["width"])));
            lines.push(format!("{}  {}", tr("Centre to centre"), mm(&sl["center_distance"])));
            lines.push(format!("{}  {}", tr("Overall length"), mm(&sl["overall_length"])));
        }
        let round = it.get("diameter").is_some();
        match it["type"].as_str() {
            Some("edge" | "sketch_curve") if !round => lines.push(format!("{}  {}", tr("Length"), mm(&it["length_mm"]))),
            Some("face" | "profile") => {
                lines.push(format!("{}  {}", tr("Area"), crate::prefs::show_mm(app, it["area_mm2"].as_f64().unwrap_or(0.0), 2)))
            }
            Some("vertex" | "sketch_point") => {
                let p = &it["position"];
                lines.push(format!("At  {}, {}, {}", mm(&p[0]), mm(&p[1]), mm(&p[2])));
            }
            _ => {}
        }
        for (k, line) in lines.into_iter().enumerate() {
            ui.label(if k == 0 { format!("Item {}", i + 1) } else { String::new() });
            ui.label(line);
            ui.end_row();
        }
    }
    if let Some((now, d, a, b)) = shown_distance(r, *show) {
        if r.get("center_distance").is_some() {
            // Centre, min and max all listed; the chosen one is drawn and broken into ΔX/Y/Z.
            for (which, label, key) in
                [(Shown::Centre, "Centre to centre", "center_distance"), (Shown::Min, "Min", "distance_mm"), (Shown::Max, "Max", "max")]
            {
                let Some(v) = r.get(key).filter(|v| v.is_number()) else { continue };
                if ui.radio(now == which, tr(label)).clicked() {
                    *show = Some(which);
                }
                let text = RichText::new(mm(v));
                ui.label(if now == which { text.strong() } else { text });
                ui.end_row();
            }
        } else {
            ui.label(RichText::new(crate::i18n::tr_label("Distance")).strong());
            ui.label(RichText::new(crate::prefs::show_mm(app, d, 1)).strong());
            ui.end_row();
        }
        let delta = b - a;
        for (l, v) in [("ΔX", delta.x), ("ΔY", delta.y), ("ΔZ", delta.z)] {
            ui.label(l);
            ui.label(crate::prefs::show_mm(app, v.abs(), 1));
            ui.end_row();
        }
    }
    if let Some(a) = r["angle_deg"].as_f64()
        && r["items"].as_array().is_some_and(|v| v.len() == 2)
    {
        ui.label(RichText::new(crate::i18n::tr_label("Angle")).strong());
        ui.label(RichText::new(format!("{a:.3}°")).strong());
        ui.end_row();
    }
}

/// Smart Extrude (#1257, see [`crate::smart_extrude`]): a new pick sets the direction from the
/// side it was picked from, and the automatic operation follows the geometry, latched per side.
fn auto_operation(app: &SolveApp, d: &mut Dialog) {
    let Kind::Extrude { smart, .. } = &d.kind else { return };
    let picked = d.inputs.first().map(|i| i.items.as_slice()).unwrap_or_default();
    if smart.picked != picked {
        let new_first = smart.picked.first() != picked.first();
        let picked = picked.to_vec();
        let side_of_pick = if new_first && !smart.manual_dir { picked.first().and_then(|first| picked_side(app, d, first)) } else { None };
        let Kind::Extrude { direction, smart, .. } = &mut d.kind else { return };
        smart.picked = picked;
        smart.latch = None;
        if *direction < 2
            && let Some(dir) = side_of_pick
        {
            *direction = dir;
        }
    }
    let Kind::Extrude { direction, distance, .. } = &d.kind else { return };
    let Ok(dist) = app.session.doc.eval(distance, solidcraft_engine::doc::expr::Kind::Length) else { return };
    let mut side = crate::smart_extrude::side(dist, *direction);
    if !matches!(d.kind, Kind::Extrude { auto_op: true, .. }) {
        return;
    }
    let Some(params) = dialog_commands(app, d).ok().and_then(|c| c.into_iter().next()).map(|(_, p)| p) else { return };
    // To an object, the extrude goes to it on whichever side it is: that is the side (#1633).
    if params.get("to").is_some()
        && let Some(reach) = solidcraft_engine::extrude_reach(&app.session, &params)
    {
        side = crate::smart_extrude::side(reach, 0);
    }
    let Some(fresh) = solidcraft_engine::auto_operation(&app.session, &params).and_then(|op| OPS.iter().position(|o| *o == op)) else {
        return;
    };
    if let Kind::Extrude { operation, smart, .. } = &mut d.kind {
        *operation = crate::smart_extrude::operation(fresh, *operation, side, &mut smart.latch);
    }
}

/// The one-side direction a new extrude takes for its first pick: toward the side of a sketch
/// profile (or curve) the camera looks from; None for picks that keep their own way.
fn picked_side(app: &SolveApp, d: &Dialog, first: &Sel) -> Option<usize> {
    let normal = match first {
        Sel::Profile { sketch, .. } => crate::canvas::profile_plane(app, *sketch)?.normal(),
        Sel::SketchCurve { id } => app.session.world_state().sketches.iter().rev().find(|ss| ss.sketch.curve_index(id).is_some())?.plane.normal(),
        Sel::Face { .. } => return Some(0),
        _ => return None,
    };
    let (at, _) = crate::canvas::anchor(app, d)?;
    Some(crate::smart_extrude::picked_direction(crate::smart_extrude::view_ray(&app.cam, at), normal, false))
}

/// Show the active dialog (if any), docked flush to an edge of the viewport just below the axis
/// view, sized to its content. The right edge is its place, except while a sketch is open: the
/// Sketch Palette keeps the right there, and the dialog takes the left so both stay on screen.
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    app.viewport.dialog_rect = None;
    let Some(mut d) = app.dialog.take() else {
        crate::frame::reset_dock_offset(ctx);
        return;
    };
    // Edit Text lasts as long as its text: other design changes leave it open.
    if let Kind::Text(k) = &mut d.kind
        && !crate::sketch_text::refresh(app, k)
    {
        return;
    }
    crate::sketch_text::sync_path(&mut d);
    let _language = crate::i18n::scope(&app.preferences.language);
    if let Kind::TextMove(k) = &d.kind
        && !crate::sketch_text_move::valid(app, k)
    {
        return;
    }
    auto_operation(app, &mut d);
    d.sync_thread(&app.session);
    update_measure(app, &mut d);
    update_fill_cells(app, &mut d);
    // A refused OK's error is about the inputs it ran with (#641). Once a value (in the dialog,
    // or on the canvas, which the viewport applied earlier this frame), a pick or the design (a
    // parameter the inputs use) changes, it gives way to what the inputs make now: their own
    // error, or nothing and the live preview. `built`: whether they make commands, for OK below.
    let mut built = None;
    if d.error.is_some()
        && let Some(was) = &d.refused
    {
        let now = Refused::now(app, &d);
        built = Some(now.commands.is_ok());
        if now != *was {
            d.error = now.commands.as_ref().err().cloned();
            d.refused = d.error.is_some().then_some(now);
        }
    }
    // Physical materials: the library's, then the design's own (material.create).
    let material_names = app.session.doc.material_names();
    let t = Tokens::get();
    let vp = app.viewport.rect.unwrap_or_else(|| ctx.content_rect());
    let top = (vp.top() + crate::viewport::AXIS_VIEW_CLEARANCE).min((vp.bottom() - 360.0).max(vp.top()));
    // The Sketch Palette owns the right edge for as long as a sketch is being edited. A dialog
    // started there docks at the left instead, so neither panel hides the other.
    let side = if crate::sketch_palette::editing(app) && crate::sketch_palette::fits_beside_a_dialog(app, ctx) {
        crate::frame::Side::Left
    } else {
        crate::frame::Side::Right
    };
    // Dragged by its title band, the dialog keeps that offset from its docked place while it
    // is open (#1335), kept inside the viewport.
    let moved = crate::frame::dock_offset(ctx, title(&d.kind));
    let docked = egui::pos2(side.anchor_x(vp), top);
    let anchor = crate::frame::offset_anchor(docked, moved, vp, side);
    let mut keep = true;
    let mut ok = false;
    let mut applied = false;
    let mut cancel = false;
    let mut enter = false;
    let wide = d.is_wide();
    let pattern_columns = matches!(d.kind, Kind::SketchModify(crate::sketch_modify_ui::Mod::PatternRect { .. }))
        || (matches!(d.kind, Kind::PatternRect { .. }) && d.inputs.first().is_some_and(|i| i.accept == selection::FEATURES));
    let raw_title = title(&d.kind);
    let heading = if d.editing.is_some() { crate::i18n::format("dialog.edit_title", &[("title", tr(raw_title))]) } else { tr(raw_title).to_string() };
    let frame = crate::frame::dock_frame(ctx, side);
    // The area includes the border as well as the inner margins; reserve all of it so a
    // horizontally scrolling table cannot push a wide dialog two pixels past its limit.
    let horizontal_frame = frame.inner_margin.leftf() + frame.inner_margin.rightf() + 2.0 * frame.stroke.width;
    // OK is offered once the inputs make a feature.
    let has_ok = !matches!(d.kind, Kind::Sketch | Kind::Params { .. } | Kind::Measure { .. });
    let valid = match &d.kind {
        Kind::SaveMesh(_) => crate::dialogs_save_mesh::ready(&d.inputs),
        _ => !has_ok || !d.previews() || (built.unwrap_or_else(|| apply_commands(app, &d).is_ok()) && app.preview.error.is_none()),
    };
    let width = if wide { crate::frame::Width::Wide } else { crate::frame::Width::Normal };
    let wkey = egui::Id::new(("sc_dialog_w", wide));
    let response = crate::frame::docked_on(
        RichText::new(heading.clone()).strong().size(13.0),
        egui::Id::new(("sc_dialog", title(&d.kind))),
        anchor,
        vp.bottom() - anchor.y - 8.0,
        width,
        side,
    )
    .frame(frame)
    .open(&mut keep)
    .show(ctx, |ui| {
        ui.spacing_mut().text_edit_width = FIELD_W;
        // Long texts (errors, notes) wrap instead of widening the dialog.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        let margin = horizontal_frame;
        let min_w = if pattern_columns {
            540.0
        } else if wide {
            480.0
        } else {
            DIALOG_MIN_W
        } - margin;
        ui.set_min_width(min_w);
        ui.set_max_width(if wide { DIALOG_WIDE_MAX_W } else { DIALOG_MAX_W } - margin);
        // Rows that end at the right edge line up with last frame's content width.
        let total = crate::frame::content_width(ui, wkey, min_w);
        // Header: "−" folds the dialog, the command name, "»" at the right, on the title band,
        // which also moves the dialog.
        let band_top = ui.cursor().min;
        let band = egui::Rect::from_min_size(band_top, vec2(total, ui.spacing().interact_size.y + 2.0));
        crate::frame::drag_dock(ui, band, title(&d.kind), moved, anchor - docked);
        crate::frame::heading(ui, crate::frame::Level::Title, total, |ui| {
            let total = total - 2.0 * crate::frame::HEADING_MARGIN;
            let fold = if d.collapsed { "+" } else { "−" };
            if ui
                .add(egui::Button::new(RichText::new(fold).size(14.0)).frame(false))
                .on_hover_text(if d.collapsed { "Expand" } else { "Collapse" })
                .clicked()
            {
                d.collapsed = !d.collapsed;
            }
            // Not selectable: a drag on the title moves the dialog.
            ui.add(egui::Label::new(RichText::new(heading.as_str()).strong().size(12.5)).selectable(false));
            let chevron_width = ui.spacing().interact_size.x;
            crate::frame::right_aligned(ui, total, chevron_width, |ui| {
                let chevron = if d.collapsed { "«" } else { "»" };
                if ui
                    .add(egui::Button::new(RichText::new(chevron).color(t.text_dim)).frame(false).min_size(vec2(chevron_width, 0.0)))
                    .on_hover_text(if d.collapsed { "Expand" } else { "Collapse" })
                    .clicked()
                {
                    d.collapsed = !d.collapsed;
                }
            });
        });
        if d.collapsed {
            return;
        }
        ui.add_space(2.0);
        // A sizing pass initially offers almost no height to an auto-sized window.
        // Give only the fields a viewport-derived scroll budget; header and actions
        // stay visible, while short forms still shrink to their natural height.
        let vertical_frame = frame.inner_margin.topf() + frame.inner_margin.bottomf() + 2.0 * frame.stroke.width;
        let footer_height = ui.spacing().interact_size.y.max(24.0) + 6.0 + ui.spacing().item_spacing.y;
        let fields_height = (vp.bottom() - anchor.y - 8.0 - vertical_frame - ui.min_rect().height() - footer_height).max(1.0);
        egui::ScrollArea::vertical().id_salt("feature_fields").min_scrolled_height(fields_height).max_height(fields_height).show(ui, |ui| {
            // While OK computes, its inputs hold: what is applied is what was asked.
            if app.applying.is_some() {
                ui.disable();
            }
            // Configuration tables own their column widths and scrolling; the two-column form
            // grid would squeeze their row names and action buttons into a label-sized cell.
            if let Kind::Motion(k @ crate::dialogs_motion::Mo::Configurations { .. }) = &mut d.kind {
                enter |= crate::dialogs_motion::rows(app, ui, k, &mut d.inputs);
            }
            egui::Grid::new("sc_dialog_grid").num_columns(if pattern_columns { 1 } else { 2 }).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                if pattern_columns {
                    enter |= rectangular_pattern_rows(&mut d, ui, &app.session);
                    error_rows(app, ctx, &mut d, ui, true);
                    return;
                }
                if matches!(&d.kind, Kind::Motion(crate::dialogs_motion::Mo::Configurations { .. })) {
                    return;
                }
                placement_row(&mut d, ui);
                if matches!(&d.kind, Kind::SketchModify(k) if k.previews())
                    || matches!(d.kind, Kind::Mirror | Kind::PatternRect { .. } | Kind::PatternCirc { .. } | Kind::PathPattern { .. })
                {
                    row_label(ui, "dialog.step");
                    let label = d.active_input().map(|input| tr(input.label)).unwrap_or_else(|| tr("Objects"));
                    ui.label(crate::i18n::format(
                        "dialog.selection_step",
                        &[("step", &(d.active + 1).to_string()), ("total", &d.inputs.len().to_string()), ("input", label)],
                    ));
                    ui.end_row();
                    row_label(ui, "");
                    ui.label(tr("dialog.selection_guidance"));
                    ui.end_row();
                }
                input_rows(&mut d, ui, &app.session);
                match &mut d.kind {
                    Kind::Sketch => {
                        if d.error.is_some()
                            && let Some(selection @ Sel::Face { .. }) = d.inputs.first().and_then(|i| i.items.first())
                        {
                            row_label(ui, "Attachment");
                            ui.label(tr("This face cannot provide a durable attachment. Choose another face or explicitly use a fixed plane."));
                            ui.end_row();
                            row_label(ui, "");
                            if ui.button(tr("Use fixed plane")).clicked()
                                && let Some(mut plane) = crate::sketch_attachment_ui::face_plane(app, selection)
                            {
                                plane["fixed"] = json!(true);
                                match app.run("sketch.create", json!({"plane":plane})) {
                                    Ok(_) => {
                                        app.pre_sketch_cam = Some(app.cam);
                                        look_at_sketch(app);
                                        cancel = true;
                                    }
                                    Err(error) => d.refuse(app, error),
                                }
                            }
                            ui.end_row();
                        }
                    }
                    Kind::Extrude {
                        distance,
                        direction,
                        operation,
                        auto_op,
                        smart,
                        distance2,
                        taper,
                        taper_present,
                        to_offset,
                        start,
                        all,
                        thin,
                        wall,
                        wall_side,
                        wall2,
                    } => {
                        row_label(ui, "Thin");
                        if ui.checkbox(thin, tr("Thin Extrude")).changed() {
                            set_thin(&mut d.inputs, *thin);
                        }
                        ui.end_row();
                        if *thin {
                            row_label(ui, "Wall Location");
                            combo(ui, "ex_wall", &WALL_LABELS, wall_side);
                            ui.end_row();
                            row_label(ui, "Wall Thickness");
                            enter |= field(ui, wall);
                            ui.end_row();
                            if *wall_side == 3 {
                                row_label(ui, "Thickness Outside");
                                enter |= field(ui, wall2);
                                ui.end_row();
                            }
                        }
                        row_label(ui, "Start");
                        let old = if object_on(&d.inputs, FROM_OBJECT) { 2 } else { usize::from(!start.is_empty()) };
                        let mut st = old;
                        combo(ui, "ex_start", &["Profile Plane", "Offset", "From Object"], &mut st);
                        if st != old {
                            set_object(&mut d.inputs, &mut d.active, FROM_OBJECT, st == 2);
                            if st == 1 && start.is_empty() {
                                *start = "5 mm".into();
                            }
                            if st == 0 {
                                start.clear();
                            }
                        }
                        ui.end_row();
                        if st != 0 {
                            row_label(ui, "Offset");
                            enter |= field(ui, start);
                            ui.end_row();
                        }
                        row_label(ui, "Direction");
                        // One Side, Two Sides, Symmetric (a flip is a negative distance).
                        let order = [0usize, TWO_SIDES, 2];
                        let mut pos = order.iter().position(|o| o == direction).unwrap_or(0);
                        combo(ui, "ex_dir", &["One Side", "Two Sides", "Symmetric"], &mut pos);
                        if pos != order.iter().position(|o| o == direction).unwrap_or(0) {
                            *direction = order.get(pos).copied().unwrap_or(0);
                            // Selecting Symmetric applies the current total-distance semantics.
                            d.extra.insert("convention".into(), json!(solidcraft_engine::doc::ExtrudeConvention::TotalDistance));
                            // An explicit direction ends the automatic choices (#1257).
                            smart.manual_dir = true;
                            *auto_op = false;
                        }
                        ui.end_row();
                        if *direction < 2 {
                            row_label(ui, "Reverse");
                            let mut reverse = *direction == 1;
                            if ui.checkbox(&mut reverse, tr("Reverse Direction")).changed() {
                                *direction = usize::from(reverse);
                                // An explicit direction ends the automatic choices (#1257).
                                smart.manual_dir = true;
                                *auto_op = false;
                            }
                            ui.end_row();
                        }
                        row_label(ui, "Extent Type");
                        let cur = if to_object_on(&d.inputs) { 1 } else { usize::from(*all) * 2 };
                        let mut ext = cur;
                        combo(ui, "ex_ext", &["Distance", "To Object", "All"], &mut ext);
                        *all = ext == 2;
                        set_to_object(&mut d.inputs, &mut d.active, ext == 1);
                        ui.end_row();
                        if ext == 0 {
                            row_label(ui, "Distance");
                            enter |= field(ui, distance);
                            ui.end_row();
                            if *direction == TWO_SIDES {
                                row_label(ui, "Distance 2");
                                enter |= field(ui, distance2);
                                ui.end_row();
                            }
                        }
                        if ext == 1 {
                            row_label(ui, "To Offset");
                            enter |= field(ui, to_offset);
                            ui.end_row();
                        }
                        if *direction < 2 {
                            row_label(ui, "Taper Angle");
                            let before = taper.clone();
                            enter |= field(ui, taper);
                            if *taper != before {
                                *taper_present = !taper.trim().is_empty();
                            }
                            ui.end_row();
                        }
                        row_label(ui, "Operation");
                        let before = *operation;
                        combo(ui, "ex_op", &OP_LABELS, operation);
                        // A choice made by hand sticks.
                        if *operation != before {
                            *auto_op = false;
                        }
                        ui.end_row();
                    }
                    Kind::Revolve { angle, operation, .. } => {
                        row_label(ui, "Type");
                        let cur = usize::from(to_object_on(&d.inputs));
                        let mut ty = cur;
                        combo(ui, "rv_type", &["Angle", "To Object"], &mut ty);
                        set_to_object(&mut d.inputs, &mut d.active, ty == 1);
                        ui.end_row();
                        if ty == 0 {
                            // One Side, Two Sides (a second angle), Symmetric (the angle split).
                            row_label(ui, "Direction");
                            let cur = if d.extra.contains_key("angle2") {
                                1
                            } else if d.extra.get("direction").is_some() {
                                2
                            } else {
                                0
                            };
                            let mut dir = cur;
                            combo(ui, "rv_dir", &["One Side", "Two Sides", "Symmetric"], &mut dir);
                            if dir != cur {
                                d.extra.remove("angle2");
                                d.extra.remove("direction");
                                match dir {
                                    1 => {
                                        d.extra.insert("angle2".into(), json!("90 deg"));
                                    }
                                    2 => {
                                        d.extra.insert("direction".into(), json!("symmetric"));
                                    }
                                    _ => {}
                                }
                            }
                            ui.end_row();
                            row_label(ui, "Angle");
                            enter |= field(ui, angle);
                            ui.end_row();
                            if dir == 1 {
                                row_label(ui, "Angle 2");
                                enter |= extra_field(ui, &mut d.extra, "angle2", "90 deg");
                                ui.end_row();
                            }
                        }
                        row_label(ui, "Operation");
                        combo(ui, "rv_op", &OP_LABELS, operation);
                        ui.end_row();
                    }
                    Kind::Fillet { radius, chamfer, chain, ctype, distance2, angle, flip } => {
                        if *chamfer {
                            row_label(ui, "Chamfer Type");
                            combo(ui, "ch_type", &CHAMFER_TYPES, ctype);
                            ui.end_row();
                        }
                        let mut rtype = 0;
                        if !*chamfer {
                            row_label(ui, "Radius Type");
                            rtype = extra_choice(
                                ui,
                                &mut d.extra,
                                "fi_type",
                                "type",
                                &[
                                    ("Constant", "constant"),
                                    ("Chord Length", "chord"),
                                    ("Variable", "variable"),
                                    ("Spline", "spline"),
                                    ("Curvature (G2)", "curvature"),
                                ],
                            );
                            ui.end_row();
                        }
                        row_label(
                            ui,
                            if !*chamfer {
                                match rtype {
                                    1 => "Chord Length",
                                    2 | 3 => "Start Radius",
                                    4 => "Equivalent Radius",
                                    _ => "Radius",
                                }
                            } else if *ctype == 0 {
                                "Distance"
                            } else {
                                "Distance 1"
                            },
                        );
                        enter |= field(ui, radius);
                        ui.end_row();
                        // A constant radius too large for the edges: refused, reduced to the
                        // largest that fits (the preview shows what was used), or tapered along
                        // the edges to the room there.
                        if !*chamfer && rtype == 0 {
                            row_label(ui, "Radius Fit");
                            let fit = extra_choice(
                                ui,
                                &mut d.extra,
                                "fi_fit",
                                "fit",
                                &[("Exact", "exact"), ("Reduce to Fit", "reduce"), ("Taper to Fit", "taper")],
                            );
                            ui.end_row();
                            if fit == 1
                                && let Some((lo, hi)) = app.preview.applied
                            {
                                // A range where edges apart from each other fit different radii.
                                row_label(ui, "Applied Radius");
                                let shown = if hi > lo {
                                    format!("{} – {}", crate::prefs::show_mm_bare(app, lo), crate::prefs::show_mm(app, hi, 1))
                                } else {
                                    crate::prefs::show_mm(app, lo, 1)
                                };
                                ui.label(RichText::new(shown).color(t.warning));
                                ui.end_row();
                            }
                        } else if !*chamfer {
                            d.extra.remove("fit");
                        }
                        if *chamfer && *ctype == 1 {
                            row_label(ui, "Distance 2");
                            enter |= field(ui, distance2);
                            ui.end_row();
                        }
                        if *chamfer && *ctype == 2 {
                            row_label(ui, "Angle");
                            enter |= field(ui, angle);
                            ui.end_row();
                        }
                        if *chamfer && *ctype > 0 {
                            row_label(ui, "Flip");
                            ui.checkbox(flip, "");
                            ui.end_row();
                        }
                        if !*chamfer && rtype == 2 {
                            row_label(ui, "End Radius");
                            enter |= extra_field(ui, &mut d.extra, "radius2", "1 mm");
                            ui.end_row();
                            // Which end of the first edge takes the start radius.
                            row_label(ui, "Swap Ends");
                            extra_check(ui, &mut d.extra, "swap_ends");
                            ui.end_row();
                            // Radii between the ends: position along the edge (from the start end)
                            // and radius.
                            let mut mids = mid_points(&d.extra);
                            let mut remove = None;
                            for (k, (t, r)) in mids.iter_mut().enumerate() {
                                row_label(ui, "Mid Radius");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::DragValue::new(t)
                                            .speed(0.01)
                                            .range(0.01..=0.99)
                                            .fixed_decimals(2)
                                            .custom_parser(crate::prefs::parse_number),
                                    )
                                    .on_hover_text(tr("Position along the edge, 0 at the start end, 1 at the other"));
                                    enter |= field(ui, r);
                                    if ui.small_button("×").on_hover_text(tr("Remove this point")).clicked() {
                                        remove = Some(k);
                                    }
                                });
                                ui.end_row();
                            }
                            if let Some(k) = remove {
                                mids.remove(k);
                            }
                            row_label(ui, "");
                            if ui.button(tr("Add Mid Radius")).clicked() && mids.len() < 20 {
                                let t = if mids.is_empty() { 0.5 } else { (mids.iter().map(|m| m.0).fold(0.0, f64::max) + 1.0) * 0.5 };
                                mids.push((t.min(0.99), radius.clone()));
                            }
                            ui.end_row();
                            d.extra.insert("mid".into(), json!(mids.iter().map(|(t, r)| json!([t, r])).collect::<Vec<_>>()));
                            if !mids.is_empty() {
                                row_label(ui, "Smooth");
                                extra_check(ui, &mut d.extra, "smooth");
                                ui.end_row();
                            }
                        }
                        if !*chamfer && rtype == 3 {
                            // The radius along the edge as a B-spline: its control radii from the
                            // start end, also dragged by their handles on the edge.
                            row_label(ui, "Swap Ends");
                            extra_check(ui, &mut d.extra, "swap_ends");
                            ui.end_row();
                            let mut radii = spline_radii(&mut d.extra, radius);
                            let n = radii.len();
                            for (k, r) in radii.iter_mut().enumerate() {
                                row_label(ui, if k + 1 == n { "End Radius" } else { "Control Radius" });
                                enter |= field(ui, r);
                                ui.end_row();
                            }
                            d.extra.insert("radii".into(), json!(radii));
                        }
                        row_label(ui, "Tangent Chain");
                        ui.checkbox(chain, "");
                        ui.end_row();
                    }
                    Kind::Shell { thickness } => {
                        row_label(ui, "Tangent Chain");
                        extra_check(ui, &mut d.extra, "tangent_chain");
                        ui.end_row();
                        let dir = d.extra.get("direction").and_then(Value::as_str).unwrap_or("inside").to_string();
                        row_label(
                            ui,
                            match dir.as_str() {
                                "outside" => "Outside Thickness",
                                "both" => "Thickness",
                                _ => "Inside Thickness",
                            },
                        );
                        enter |= field(ui, thickness);
                        ui.end_row();
                        row_label(ui, "Direction");
                        extra_choice(ui, &mut d.extra, "sh_dir", "direction", &[("Inside", "inside"), ("Outside", "outside"), ("Both", "both")]);
                        ui.end_row();
                    }
                    Kind::Draft { angle } => {
                        row_label(ui, "Angle");
                        enter |= field(ui, angle);
                        ui.end_row();
                    }
                    Kind::Mirror => {
                        // Feature replay retains the source operation; Join is for body copies.
                        if !d.inputs.first().is_some_and(|i| i.accept == selection::FEATURES) {
                            row_label(ui, "Operation");
                            let mut op = usize::from(d.extra.get("combine").and_then(Value::as_bool).unwrap_or(false));
                            combo(ui, "mi_op", &["New Body", "Join"], &mut op);
                            d.extra.insert("combine".into(), json!(op == 1));
                            ui.end_row();
                        }
                    }
                    Kind::Measure { result, show, .. } => measure_rows(app, ui, result.as_ref(), show),
                    Kind::OffsetPlane { offset } => {
                        row_label(ui, "Distance");
                        enter |= field(ui, offset);
                        ui.end_row();
                    }
                    Kind::AnglePlane { angle } => {
                        row_label(ui, "Angle");
                        enter |= field(ui, angle);
                        ui.end_row();
                    }
                    Kind::PathPlane { t, .. } => {
                        row_label(ui, "Position (0 to 1)");
                        enter |= field(ui, t);
                        ui.end_row();
                    }
                    Kind::Split | Kind::ReplaceFace | Kind::Remove | Kind::Midplane => {}
                    Kind::Rib { thickness, depth, flip, normal, .. } => {
                        row_label(ui, "Direction");
                        let mut i = usize::from(*normal);
                        combo(ui, "rib_direction", &["In plane", "Normal to sketch"], &mut i);
                        *normal = i == 1;
                        ui.end_row();
                        row_label(ui, "Thickness");
                        enter |= field(ui, thickness);
                        ui.end_row();
                        row_label(ui, "Depth");
                        let r = ui.add(egui::TextEdit::singleline(depth).hint_text(tr("to the next face")));
                        crate::params_dialog::complete(ui, &r, depth);
                        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.end_row();
                        row_label(ui, "Flip");
                        ui.checkbox(flip, "");
                        ui.end_row();
                    }
                    Kind::Emboss { depth, deboss } => {
                        row_label(ui, "Depth");
                        enter |= field(ui, depth);
                        ui.end_row();
                        row_label(ui, "Type");
                        let mut i = usize::from(*deboss);
                        combo(ui, "emb_mode", &["Emboss", "Deboss"], &mut i);
                        *deboss = i == 1;
                        ui.end_row();
                        ui.label("");
                        ui.add(
                            egui::Label::new(
                                "Orthogonal line profiles tangent to one supported cylinder wrap automatically. Pick a wall to choose a target.",
                            )
                            .wrap(),
                        );
                        ui.end_row();
                    }
                    Kind::Align { flip } => {
                        // Where on the picked face or edge to snap: as picked (a face turns to meet
                        // the other, an edge at its middle), its centre, or a round one's centre.
                        for (label, key) in [("From Snap", "from_snap"), ("To Snap", "to_snap")] {
                            row_label(ui, label);
                            extra_choice(ui, &mut d.extra, key, key, &[("As Picked", "picked"), ("Centre", "center"), ("Circle Centre", "circle")]);
                            ui.end_row();
                        }
                        row_label(ui, "Flip");
                        ui.checkbox(flip, "");
                        ui.end_row();
                    }
                    Kind::BoundaryFill { operation, cells, .. } => {
                        row_label(ui, "Cells");
                        if cells.is_empty() {
                            ui.label(RichText::new(tr("Pick the tool bodies")).color(Tokens::get().text_dim));
                        }
                        ui.end_row();
                        for (i, (_, label, on)) in cells.iter_mut().enumerate() {
                            row_label(ui, &format!("Cell {}", i + 1));
                            ui.checkbox(on, label.as_str());
                            ui.end_row();
                        }
                        row_label(ui, "Operation");
                        combo(ui, "bf_op", &OP_LABELS, operation);
                        ui.end_row();
                        row_label(ui, "Remove Tools");
                        extra_check(ui, &mut d.extra, "remove_tools");
                        ui.end_row();
                    }
                    Kind::PathPattern { count, spacing } => {
                        for (l, v) in [("Quantity", count), ("Spacing", spacing)] {
                            row_label(ui, l);
                            enter |= field(ui, v);
                            ui.end_row();
                        }
                    }
                    Kind::Pipe { diameter, wall } => {
                        row_label(ui, "Diameter");
                        enter |= field(ui, diameter);
                        ui.end_row();
                        row_label(ui, "Wall");
                        let r = ui.add(egui::TextEdit::singleline(wall).hint_text("solid"));
                        crate::params_dialog::complete(ui, &r, wall);
                        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.end_row();
                    }
                    Kind::Stock { margin } => {
                        row_label(ui, "Margin");
                        enter |= field(ui, margin);
                        ui.end_row();
                    }
                    Kind::Scale { factor } => {
                        row_label(ui, "Scale factor");
                        enter |= field(ui, factor);
                        ui.end_row();
                    }
                    Kind::OffsetFaces { distance } => {
                        row_label(ui, "Distance");
                        enter |= field(ui, distance);
                        ui.end_row();
                    }
                    Kind::Thread { designation, length, suggested, of } => {
                        row_label(ui, "Modeled");
                        extra_check(ui, &mut d.extra, "modeled");
                        ui.end_row();
                        row_label(ui, "Full Length");
                        let mut full = length.trim().is_empty();
                        let response = ui.checkbox(&mut full, "");
                        publish_row(&response);
                        if response.changed() {
                            *length = if full { String::new() } else { "10 mm".into() };
                        }
                        ui.end_row();
                        let (submitted, reset) = crate::dialogs_thread::rows(ui, designation, suggested);
                        enter |= submitted;
                        if reset {
                            of.clear();
                        }
                        if !full {
                            row_label(ui, "Length");
                            enter |= field(ui, length);
                            ui.end_row();
                        }
                    }
                    Kind::Material { index } => {
                        let names: Vec<&str> = material_names.iter().map(String::as_str).collect();
                        row_label(ui, "Material");
                        combo(ui, "mat", &names, index);
                        ui.end_row();
                    }
                    Kind::Section { offset, flip } => {
                        row_label(ui, "Distance");
                        enter |= field(ui, offset);
                        ui.end_row();
                        row_label(ui, "Flip");
                        ui.checkbox(flip, "");
                        ui.end_row();
                    }
                    Kind::PatternRect { count, spacing, count2, spacing2 } => {
                        for (l, v) in [("Quantity", count), ("Spacing", spacing), ("Quantity 2", count2), ("Spacing 2", spacing2)] {
                            row_label(ui, l);
                            enter |= field(ui, v);
                            ui.end_row();
                        }
                    }
                    Kind::PatternCirc { count, angle } => {
                        for (l, v) in [("Quantity", count), ("Total angle", angle)] {
                            row_label(ui, l);
                            enter |= field(ui, v);
                            ui.end_row();
                        }
                        if d.inputs.first().is_some_and(|i| i.accept == selection::FEATURES) {
                            feature_pattern_alignment_row(ui, &mut d.extra, 0);
                        }
                    }
                    Kind::Loft { operation } => {
                        row_label(ui, "Operation");
                        combo(ui, "lf_op", &OP_LABELS, operation);
                        ui.end_row();
                    }
                    Kind::Sweep { operation, opts } => {
                        enter |= crate::dialogs_sweep::rows(ui, opts, "sw");
                        row_label(ui, "Operation");
                        combo(ui, "sw_op", &OP_LABELS, operation);
                        ui.end_row();
                    }
                    k @ Kind::Move { .. } => {
                        enter |= crate::dialogs_move::rows(ui, k);
                        row_label(ui, "Create Copy");
                        extra_check(ui, &mut d.extra, "copy");
                        ui.end_row();
                    }
                    Kind::Hole { diameter, depth, kind, cb_diameter, cb_depth, cs_diameter, cs_angle, opts } => {
                        row_label(ui, "Extents");
                        let cur = if to_object_on(&d.inputs) { 1 } else { usize::from(opts.all) * 2 };
                        let mut ext = cur;
                        combo(ui, "hole_ext", &["Distance", "To Object", "All"], &mut ext);
                        opts.all = ext == 2;
                        set_to_object(&mut d.inputs, &mut d.active, ext == 1);
                        ui.end_row();
                        if opts.multiple {
                            // Every point of the picked points' sketches (standalone points and
                            // circle centres), not just the picked ones.
                            row_label(ui, "All Sketch Points");
                            extra_check(ui, &mut d.extra, "all_points");
                            ui.end_row();
                            // The holes go into the part the sketch touches; Flip drills the
                            // other way.
                            row_label(ui, "Flip");
                            extra_check(ui, &mut d.extra, "flip");
                            ui.end_row();
                        }
                        row_label(ui, "Hole Type");
                        combo(ui, "hole_kind", &HOLE_LABELS, kind);
                        ui.end_row();
                        row_label(ui, "Hole Tap Type");
                        let mut tap = usize::from(opts.tapped);
                        combo(ui, "hole_tap", &["Simple", "Tapped"], &mut tap);
                        opts.tapped = tap == 1;
                        ui.end_row();
                        if opts.tapped {
                            row_label(ui, "Size");
                            let r = ui.add(egui::TextEdit::singleline(&mut opts.thread).hint_text("M5"));
                            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            ui.end_row();
                        }
                        if *kind == 0 {
                            row_label(ui, "Drill Point");
                            let mut tip = usize::from(opts.angled);
                            combo(ui, "hole_tip", &["Flat", "Angle"], &mut tip);
                            opts.angled = tip == 1;
                            ui.end_row();
                        }
                        if !opts.all {
                            row_label(ui, "Depth");
                            enter |= field(ui, depth);
                            ui.end_row();
                        }
                        if *kind == 0 && opts.angled {
                            row_label(ui, "Tip Angle");
                            enter |= field(ui, &mut opts.tip_angle);
                            ui.end_row();
                        }
                        row_label(ui, "Diameter");
                        enter |= field(ui, diameter);
                        ui.end_row();
                        if *kind == 1 {
                            row_label(ui, "Counterbore Ø");
                            enter |= field(ui, cb_diameter);
                            ui.end_row();
                            row_label(ui, "Counterbore depth");
                            enter |= field(ui, cb_depth);
                            ui.end_row();
                        }
                        if *kind == 2 {
                            row_label(ui, "Countersink Ø");
                            enter |= field(ui, cs_diameter);
                            ui.end_row();
                            row_label(ui, "Countersink angle");
                            enter |= field(ui, cs_angle);
                            ui.end_row();
                        }
                    }
                    Kind::Primitive { fields, operation, at, .. } => {
                        for (k, v) in fields.iter_mut() {
                            row_label(ui, k);
                            enter |= field(ui, v);
                            ui.end_row();
                        }
                        if let Some((p, _)) = *at {
                            row_label(ui, "Position");
                            ui.horizontal(|ui| {
                                ui.label(format!("{:.2}, {:.2}, {:.2}", p.x, p.y, p.z));
                                if ui.small_button("×").on_hover_text(tr("Place at the origin instead")).clicked() {
                                    *at = None;
                                }
                            });
                            ui.end_row();
                        }
                        row_label(ui, "Operation");
                        combo(ui, "pr_op", &OP_LABELS, operation);
                        ui.end_row();
                    }
                    Kind::Combine { operation, keep_tools } => {
                        row_label(ui, "Operation");
                        let mut op = operation.saturating_sub(1);
                        combo(ui, "cb_op", &OP_LABELS[1..], &mut op);
                        *operation = op + 1;
                        ui.end_row();
                        row_label(ui, "New Component");
                        extra_check(ui, &mut d.extra, "new_component");
                        ui.end_row();
                        row_label(ui, "Keep Tools");
                        ui.checkbox(keep_tools, "");
                        ui.end_row();
                    }
                    Kind::Params { new_name, new_expr } => params_table(app, ui, new_name, new_expr),
                    Kind::EditParam { name, expr } => {
                        ui.label(name.as_str());
                        let r = ui.text_edit_singleline(expr);
                        crate::params_dialog::complete(ui, &r, expr);
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            ok = true;
                        }
                        r.request_focus();
                        ui.end_row();
                    }
                    Kind::Rename { name, .. } => {
                        row_label(ui, "Name");
                        let r = ui.text_edit_singleline(name);
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            ok = true;
                        }
                        r.request_focus();
                        ui.end_row();
                    }
                    Kind::Assembly(k) => enter |= crate::dialogs_assembly::rows(app, ui, k, &mut d.inputs, &mut d.active),
                    Kind::Sheet(k) => enter |= crate::dialogs_sheet::rows(app, ui, k, &mut d.inputs),
                    Kind::Surface(k) => enter |= crate::dialogs_surface::rows(ui, k),
                    Kind::SketchModify(k) => enter |= crate::sketch_modify_ui::rows(ui, k),
                    Kind::SaveMesh(k) => enter |= crate::dialogs_save_mesh::rows(app, ui, k),
                    Kind::ViewDxf { dashed, tolerance } => crate::dialogs_view_dxf::rows(app, ui, dashed, tolerance),
                    Kind::Plastic(k) => enter |= crate::dialogs_plastic::rows(app, ui, k, &d.inputs),
                    Kind::Appearance(k) => enter |= crate::dialogs_appearance::rows(app, ui, k, &mut d.inputs),
                    Kind::Motion(k) => enter |= crate::dialogs_motion::rows(app, ui, k, &mut d.inputs),
                    Kind::Part(k) => enter |= crate::dialogs_parts::rows(app, ui, k, &d.inputs),
                    Kind::Text(k) => enter |= crate::sketch_text::rows(ui, k, &mut d.inputs, &mut d.active),
                    Kind::TextMove(k) => enter |= crate::sketch_text_move::rows(ui, k),
                    Kind::ConfirmDelete { with, fail, .. } => {
                        if !with.is_empty() {
                            row_label(ui, "Also deletes");
                            ui.label(with.join(", "));
                            ui.end_row();
                        }
                        if !fail.is_empty() {
                            row_label(ui, "Will fail");
                            ui.label(RichText::new(fail.join(", ")).color(t.warning));
                            ui.end_row();
                        }
                    }
                }
                error_rows(app, ctx, &mut d, ui, false);
            });
        });
        ui.add_space(6.0);
        // Footer: an info glyph at the left, OK and Cancel at the right.
        ui.horizontal(|ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
            ui.painter().circle_stroke(r.center(), 7.0, Stroke::new(1.2, t.text_dim));
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "i", egui::FontId::proportional(11.0), t.text_dim);
            resp.on_hover_text(heading.as_str());
            // The spinner's place is always taken: a widget that comes and goes would shift
            // the automatic ids of OK and Cancel, and a click pressed in one frame and
            // released in the next would be lost (an animating preview toggles it often).
            let (sr, sresp) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
            if let Some(job) = &app.applying {
                egui::Spinner::new().size(14.0).paint_at(ui, sr);
                let seconds = ((crate::now_ms() - job.started) / 1000.0).floor().max(0.0);
                ui.label(RichText::new(crate::i18n::format("dialog.computing", &[("seconds", &format!("{seconds:.0}"))])).color(t.text_dim));
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            } else if app.preview.busy {
                egui::Spinner::new().size(14.0).paint_at(ui, sr);
                sresp.on_hover_text(tr("Updating the preview"));
            }
            let raw_label = match &d.kind {
                Kind::ConfirmDelete { .. } => "Delete",
                Kind::SaveMesh(k) => crate::dialogs_save_mesh::button(k),
                Kind::ViewDxf { .. } => "Export DXF…",
                _ => "OK",
            };
            let label = tr(raw_label);
            let raw_close = if matches!(d.kind, Kind::Params { .. } | Kind::Measure { .. }) { "Close" } else { "Cancel" };
            let close = tr(raw_close);
            let button_width = |text: &str| {
                let galley = egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
                (galley.size().x + 2.0 * ui.spacing().button_padding.x).max(64.0)
            };
            let (ok_width, close_width) = (button_width(label), button_width(close));
            let buttons = close_width + if has_ok { ok_width + ui.spacing().item_spacing.x } else { 0.0 };
            crate::frame::right_aligned(ui, total, buttons, |ui| {
                if has_ok {
                    let b = egui::Button::new(RichText::new(label).color(if valid { Color32::WHITE } else { t.text_dim }))
                        .fill(if valid { t.accent } else { Color32::TRANSPARENT })
                        .wrap_mode(egui::TextWrapMode::Extend)
                        .min_size(vec2(ok_width, 24.0));
                    let r = ui.add_enabled(valid && app.applying.is_none(), b);
                    crate::scenario::publish_handle(&format!("dialog:{raw_label}"), r.rect.center());
                    if r.clicked() {
                        ok = true;
                    }
                }
                if ui.add(egui::Button::new(close).wrap_mode(egui::TextWrapMode::Extend).min_size(vec2(close_width, 24.0))).clicked() {
                    cancel = true;
                }
            });
        });
        crate::frame::remember_width(ui, wkey);
    });
    app.viewport.dialog_rect = response.map(|r| r.response.rect);
    if let Some(r) = app.viewport.dialog_rect {
        crate::scenario::publish_rect("dialog:outer", r);
    }
    // Keyboard: Enter applies (from a value field, or with nothing focused), Esc cancels.
    let (enter_free, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    let nothing_focused = ctx.memory(|m| m.focused().is_none());
    let canvas_enter = enter_free && ctx.memory(|m| m.had_focus_last_frame(egui::Id::new("sc_canvas_value")));
    // Tables (configurations, a study's keys) take Enter for their cells.
    let enter_applies = !matches!(d.kind, Kind::Sketch | Kind::Params { .. } | Kind::Measure { .. } | Kind::ConfirmDelete { .. })
        && !matches!(&d.kind, Kind::Motion(k) if k.wide());
    let report_active = app.issue_report.draft.is_some();
    if !report_active && enter_applies && app.applying.is_none() && (enter || canvas_enter || (enter_free && nothing_focused)) {
        ok = true;
    }
    if !report_active && esc && !app.esc_handled {
        app.esc_handled = true;
        cancel = true;
    }
    // A plane picked for a new sketch starts it right away.
    let mut then_start = None;
    if matches!(d.kind, Kind::Sketch)
        && d.error.is_none()
        && let Some(sel) = d.inputs.first().and_then(|i| i.items.first()).cloned()
    {
        match start_sketch(app, &sel) {
            Ok(()) => {
                cancel = true;
                then_start = d.then_start;
            }
            Err(error) => d.refuse(app, error),
        }
    }
    if ok && app.applying.is_none() {
        // Edit Text applies its own way: its live changes and OK are one undo step.
        let run = match &mut d.kind {
            Kind::Text(k) => Some(crate::sketch_text::apply(app, k)),
            _ => match begin_applying(app, &d) {
                // Computing: the dialog stays, saying so, until it is in.
                Some(job) => {
                    app.applying = Some(job);
                    None
                }
                None => Some(run_dialog(app, &d)),
            },
        };
        match run {
            Some(Ok(())) => {
                cancel = true;
                applied = true;
            }
            Some(Err(e)) => d.refuse(app, e),
            None => {}
        }
    }
    // OK's evaluation is in: run it, the feature taken from the cache. Inputs changed meanwhile
    // (nothing should change them) are computed in turn, off the window thread too: run here,
    // they would hold the window for the whole boolean.
    if app.applying.as_ref().is_some_and(Applying::finished) && !cancel {
        let computed = app.applying.take().map(|job| job.cmds);
        let current = apply_commands(app, &d).ok();
        let again = if current.is_some() && current != computed { begin_applying(app, &d) } else { None };
        match again {
            Some(job) => app.applying = Some(job),
            None => match run_dialog(app, &d) {
                Ok(()) => {
                    cancel = true;
                    applied = true;
                }
                Err(e) => d.refuse(app, e),
            },
        }
    }
    // Cancel while computing: the result goes (its worker runs on to its end).
    if cancel && !applied {
        app.applying = None;
    }
    if !keep || cancel {
        crate::dialogs_motion::closed(app, &mut d, applied);
        if let Kind::Text(k) = &mut d.kind {
            crate::sketch_text::closed(app, k, applied);
        }
    }
    // The edited text's outlines are new curves: the old selection would name stale ids.
    if applied && matches!(d.kind, Kind::Text(_) | Kind::TextMove(_)) {
        let _ = app.session.execute("select.clear", &json!({}));
    }
    if keep && !cancel {
        // The canvas follows Edit Text's fields.
        if let Kind::Text(k) = &mut d.kind {
            crate::sketch_text::preview(app, k);
        }
        app.dialog = Some(d);
    } else if let Some((_, marker)) = d.editing {
        // Editing done: put the timeline marker back where it was.
        let _ = roll_marker(app, marker);
    }
    // The shortcut that asked for this sketch starts its tool in it.
    if let Some(id) = then_start {
        app.start(id);
    }
}

/// Close the dialog without applying it (Esc, Cancel). An edit puts the timeline marker back.
pub fn cancel(app: &mut SolveApp) {
    app.applying = None;
    if let Some(mut d) = app.dialog.take() {
        crate::dialogs_motion::closed(app, &mut d, false);
        if let Kind::Text(k) = &mut d.kind {
            crate::sketch_text::closed(app, k, false);
        }
        app.dialog = Some(d);
    }
    if let Some(d) = app.dialog.take()
        && let Some((_, marker)) = d.editing
    {
        let _ = roll_marker(app, marker);
    }
}

/// Temporary rollback used by an editor is view state. Preserve both history stacks,
/// including redo and a history already at the undo limit.
pub(crate) fn roll_marker(app: &mut SolveApp, marker: Option<usize>) -> Result<(), String> {
    let undo = std::mem::take(&mut app.session.undo);
    let redo = std::mem::take(&mut app.session.redo);
    let selection = std::mem::take(&mut app.session.selection);
    let result = app.run("timeline.roll_to", marker.map(|m| json!({"position":m})).unwrap_or_else(|| json!({})));
    app.session.undo = undo;
    app.session.redo = redo;
    app.session.selection = selection;
    result.map(|_| ())
}

/// The pick input an extent of To Object adds (a face, vertex or point to go to).
const TO_OBJECT: &str = "To Object";
const FROM_OBJECT: &str = "From Object";

fn object_on(inputs: &[SelInput], label: &str) -> bool {
    inputs.iter().any(|i| i.label == label)
}

fn to_object_on(inputs: &[SelInput]) -> bool {
    inputs.iter().any(|i| i.label == TO_OBJECT)
}

/// Add or remove the To Object input; the active input moves to it when it is added.
fn set_to_object(inputs: &mut Vec<SelInput>, active: &mut usize, on: bool) {
    set_object(inputs, active, TO_OBJECT, on);
}

fn set_object(inputs: &mut Vec<SelInput>, active: &mut usize, label: &'static str, on: bool) {
    if on && !object_on(inputs, label) {
        inputs.push(SelInput::new(label, FACES | selection::VERTICES, false));
        *active = inputs.len() - 1;
    } else if !on && object_on(inputs, label) {
        inputs.retain(|i| i.label != label);
        *active = (*active).min(inputs.len().saturating_sub(1));
    }
}

/// The picked To Object place.
fn to_object_point(inputs: &[SelInput]) -> Option<Value> {
    object_point(inputs, TO_OBJECT)
}

pub(crate) fn object_point(inputs: &[SelInput], label: &str) -> Option<Value> {
    inputs.iter().find(|i| i.label == label)?.items.first().and_then(|x| match x {
        Sel::Face { point, .. } | Sel::Vertex { point, .. } | Sel::Edge { point, .. } => Some(pt(*point)),
        _ => None,
    })
}

/// A To Object input filled from a stored place (editing).
fn with_to_object(d: &mut Dialog, s: &Session, to: Option<Vec3>) {
    with_object(d, s, TO_OBJECT, to);
}

fn with_object(d: &mut Dialog, s: &Session, label: &'static str, to: Option<Vec3>) {
    if let Some(p) = to {
        set_object(&mut d.inputs, &mut d.active, label, true);
        if let Some(inp) = d.inputs.iter_mut().find(|i| i.label == label) {
            inp.items = vec![face_sel(s, p).unwrap_or(Sel::Vertex { body: String::new(), point: p })];
        }
    }
}

/// A choice row stored in the dialog's extra parameters as `key: value` (the first choice, the
/// command's default, is stored as no key at all).
fn extra_choice(ui: &mut egui::Ui, extra: &mut serde_json::Map<String, Value>, id: &str, key: &str, choices: &[(&str, &str)]) -> usize {
    let cur = extra.get(key).and_then(Value::as_str).and_then(|v| choices.iter().position(|(_, x)| *x == v)).unwrap_or(0);
    let mut i = cur;
    let labels: Vec<&str> = choices.iter().map(|(l, _)| *l).collect();
    combo(ui, id, &labels, &mut i);
    if i != cur {
        match choices.get(i) {
            Some((_, v)) if i > 0 => {
                extra.insert(key.into(), json!(v));
            }
            _ => {
                extra.remove(key);
            }
        }
    }
    i
}

/// A checkbox stored in the dialog's extra parameters.
fn extra_check(ui: &mut egui::Ui, extra: &mut serde_json::Map<String, Value>, key: &str) {
    let mut on = extra.get(key).and_then(Value::as_bool).unwrap_or(false);
    let r = ui.checkbox(&mut on, "");
    if r.changed() {
        extra.insert(key.into(), json!(on));
    }
    // The box carries no text of its own, so a scenario reaches it by its row handle.
    publish_row(&r);
}

/// A value field stored in the dialog's extra parameters (`default` until edited).
fn extra_field(ui: &mut egui::Ui, extra: &mut serde_json::Map<String, Value>, key: &str, default: &str) -> bool {
    let mut v = extra.get(key).and_then(Value::as_str).unwrap_or(default).to_string();
    let enter = field(ui, &mut v);
    extra.insert(key.into(), json!(v));
    enter
}

/// A spline fillet's control radii after the start radius, as expressions. A dialog without a
/// spline yet gets a cubic one of two spans, all its control radii `radius`.
pub(crate) fn spline_radii(extra: &mut serde_json::Map<String, Value>, radius: &str) -> Vec<String> {
    let radii: Vec<String> = extra
        .get("radii")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| match r {
            Value::String(x) => Some(x.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .collect();
    let knots = extra.get("knots").and_then(Value::as_array).map_or(0, Vec::len);
    // All control radii (the start radius and these) plus the degree (1…3) plus one.
    if !radii.is_empty() && (radii.len() + 3..=radii.len() + 5).contains(&knots) {
        return radii;
    }
    extra.insert("knots".into(), json!([0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0]));
    vec![radius.to_string(); 4]
}

/// A variable fillet's middle radii as (position, radius expression).
fn mid_points(extra: &serde_json::Map<String, Value>) -> Vec<(f64, String)> {
    extra
        .get("mid")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let t = m.get(0)?.as_f64()?;
            let r = match m.get(1)? {
                Value::String(x) => x.clone(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some((t, r))
        })
        .collect()
}

pub(crate) const CHAMFER_TYPES: [&str; 3] = ["Equal Distance", "Two Distances", "Distance and Angle"];

/// A new Fillet or Chamfer dialog's values.
fn fillet_kind(size: &str, chamfer: bool) -> Kind {
    Kind::Fillet { radius: size.into(), chamfer, chain: true, ctype: 0, distance2: size.into(), angle: "45 deg".into(), flip: false }
}

/// A length field's first text: `number` in the design's units, with the unit written out so the
/// field shows it as the other dialogs show theirs ("20 mm", "20 in"). OK makes the size the bare
/// number would.
fn length_default(s: &Session, number: &str) -> String {
    match s.doc.with_design_unit(number, ValueKind::Length) {
        e if e != number => e,
        // A millimetre design (or one whose units are not a length): bare numbers are mm.
        _ => format!("{} mm", number.trim()),
    }
}

/// A feature's length as its edit field shows it. A bare number is kept in millimetres: where the
/// design reads bare numbers in another unit, the field says "mm" (as the configuration table
/// does), so OK keeps the size. Anything else shows as stored.
fn stored_length(s: &Session, e: &str) -> String {
    if s.doc.with_design_unit(e, ValueKind::Length) == e { e.to_string() } else { format!("{} mm", e.trim()) }
}

fn hole_defaults() -> Kind {
    Kind::Hole {
        diameter: "5 mm".into(),
        depth: "10 mm".into(),
        kind: 0,
        cb_diameter: "9 mm".into(),
        cb_depth: "3 mm".into(),
        cs_diameter: "10 mm".into(),
        cs_angle: "90 deg".into(),
        opts: HoleOpts::default(),
    }
}

fn params_table(app: &mut SolveApp, ui: &mut egui::Ui, new_name: &mut String, new_expr: &mut String) {
    let t = Tokens::get();
    ui.label(RichText::new(tr("Name")).strong());
    ui.label(RichText::new(tr("Expression  ·  value")).strong());
    ui.end_row();
    let (vals, errs) = app.session.doc.param_values();
    let params = app.session.doc.params.clone();
    for p in params {
        ui.label(if p.model { RichText::new(&p.name).color(t.text_dim) } else { RichText::new(&p.name) });
        ui.horizontal(|ui| {
            let id = egui::Id::new(("pexpr", &p.name));
            let mut e = ui.data(|dd| dd.get_temp::<String>(id)).unwrap_or_else(|| p.expr.clone());
            let r = ui.add(egui::TextEdit::singleline(&mut e).desired_width(160.0));
            let refused_id = egui::Id::new(("prefused", &p.name));
            if r.changed() {
                // Typing again clears the reason a previous change was refused.
                ui.data_mut(|dd| {
                    dd.insert_temp(id, e.clone());
                    dd.remove::<String>(refused_id);
                });
            }
            if r.lost_focus() && e != p.expr {
                match change_param(app, &p.name, &e) {
                    Ok(()) => ui.data_mut(|dd| {
                        dd.remove::<String>(id);
                        dd.remove::<String>(refused_id);
                    }),
                    // Refused (it would leave a sketch unsolved, say): the typed expression
                    // stays in the field with the reason beside it, nothing silently reverts.
                    Err(why) => ui.data_mut(|dd| {
                        dd.insert_temp(refused_id, why);
                    }),
                }
            }
            let refused = ui.data(|dd| dd.get_temp::<String>(refused_id));
            let v = vals.get(&p.name).map(|v| if p.unit == "deg" { format!("{:.3}°", v.v.to_degrees()) } else { format!("{:.4} {}", v.v, p.unit) });
            match (v, errs.get(&p.name).or(refused.as_ref())) {
                (_, Some(e)) => ui.label(RichText::new(e).color(t.error)),
                (Some(v), None) => ui.label(RichText::new(v).color(t.text_dim)),
                _ => ui.label(""),
            };
        });
        ui.end_row();
    }
    ui.add(egui::TextEdit::singleline(new_name).hint_text(tr("new name")).desired_width(110.0));
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(new_expr).hint_text(tr("expression, e.g. 25 mm")).desired_width(160.0));
        if ui.button(tr("Add")).clicked()
            && !new_name.is_empty()
            && app.run("parameters.change", json!({"name": new_name, "expression": new_expr})).is_ok()
        {
            new_name.clear();
            new_expr.clear();
        }
    });
    ui.end_row();
}

/// Set a parameter's expression from the parameters table: `Err` with the message to show
/// beside the field when the change is refused (the document keeps its values).
pub(crate) fn change_param(app: &mut SolveApp, name: &str, expression: &str) -> Result<(), String> {
    app.run("parameters.change", json!({"name": name, "expression": expression}))
        .map(|_| ())
        .map_err(|e| crate::friendly_error("parameters.change", &e))
}

/// Start a sketch on a picked plane or planar face and turn the view to face it.
pub fn start_sketch(app: &mut SolveApp, sel: &Sel) -> Result<(), String> {
    let mut params = match sel {
        Sel::Plane { name } => json!({ "plane": name }),
        Sel::Face { .. } => json!({ "plane": crate::sketch_attachment_ui::face_plane(app, sel).ok_or("Selected face placement no longer exists")? }),
        _ => return Err("Select an origin plane, construction plane or planar body face".into()),
    };
    if let Sel::Plane { name } = sel
        && let Some(flip) = flip_toward_viewer(app, name)
    {
        params["flip"] = json!(flip);
    }
    let before = app.cam;
    app.run("sketch.create", params).map_err(|error| crate::friendly_error("sketch.create", &error))?;
    app.pre_sketch_cam = Some(before);
    look_at_sketch(app);
    // On a face: look at the picked point, not the plane's origin (a corner of the face),
    // which left the face filling the view off-centre like a big square.
    if let Sel::Face { point, .. } = sel
        && let Some(mut to) = app.cam_anim.as_ref().map(|a| a.to)
    {
        to.target = *point;
        app.animate_to(to);
    }
    Ok(())
}

/// A new sketch on origin or construction plane `name` faces the viewer: when the camera is
/// behind the plane's normal, the in-plane axis to turn the plane about (`"x"` or `"y"`), chosen
/// so the sketch's y axis stays as upright on screen as it can (x then runs to the right).
fn flip_toward_viewer(app: &SolveApp, name: &str) -> Option<&'static str> {
    let plane = plane_by_name(app, name)?;
    let n = plane.normal();
    // Where the eye is: in perspective its position, in orthographic only its direction counts.
    let side = if app.cam.fov > 0.0 { (app.cam.eye() - plane.origin).dot(n) } else { app.cam.basis().2.dot(n) };
    if side >= -1e-9 {
        return None;
    }
    let up = app.cam.up.dir();
    Some(if plane.y.dot(up) > 1e-9 { "y" } else { "x" })
}

/// The world plane of an origin plane or a construction plane by name.
fn plane_by_name(app: &SolveApp, name: &str) -> Option<solidcraft_engine::geom::Plane> {
    if let Some(p) = solidcraft_engine::geom::Plane::named(name) {
        // In an active component its origin planes are the component's.
        let m = app.session.active_component_frame();
        let (o, x, y) = (
            solidcraft_engine::doc::apply_point(&m, p.origin),
            solidcraft_engine::doc::apply_vector(&m, p.x),
            solidcraft_engine::doc::apply_vector(&m, p.y),
        );
        return solidcraft_engine::geom::Plane::new(o, x, y);
    }
    let st = app.session.world_state();
    st.construct.iter().find(|c| c.name == name).and_then(|c| match &c.geom {
        solidcraft_engine::doc::construct::ConstructGeom::Plane(pl) => Some(*pl),
        _ => None,
    })
}

/// Animate the camera to look straight at the active sketch plane.
pub fn look_at_sketch(app: &mut SolveApp) {
    look_at_sketch_from(app, false);
}

/// Look At asked for by the user (Ctrl+8, the Look At buttons): like [`look_at_sketch`], and
/// from the sketch's other side when the view already faces it (#1886).
pub fn look_at_sketch_again(app: &mut SolveApp) {
    look_at_sketch_from(app, true);
}

fn look_at_sketch_from(app: &mut SolveApp, again: bool) {
    let st = app.session.world_state();
    if let Some(ss) = app.session.active_sketch.and_then(|id| st.sketch(id)) {
        let n = facing(app, ss.plane.normal(), again);
        let mut to = app.cam.looking_from(n);
        // Keep the sketch's own x axis to the right when looking straight down or up.
        if n.z.abs() > 0.999 {
            let x = ss.plane.x;
            to.yaw = if n.z > 0.0 { (-x.y).atan2(x.x) } else { x.y.atan2(x.x) };
        }
        to.target = ss.plane.origin;
        app.animate_to(to);
    }
}

/// The side to look from at a plane with normal `n`: its own side, or with `again` the other
/// side when the view already looks from `n`, so a second Look At turns the view round (#1886).
pub(crate) fn facing(app: &SolveApp, n: Vec3, again: bool) -> Vec3 {
    if again && app.cam.back().dot(n) > 0.999 { -n } else { n }
}

fn sels(d: &Dialog, i: usize) -> &[Sel] {
    d.inputs.get(i).map(|x| x.items.as_slice()).unwrap_or(&[])
}

pub(crate) fn run_dialog(app: &mut SolveApp, d: &Dialog) -> Result<(), String> {
    if let Kind::ViewDxf { dashed, tolerance } = &d.kind {
        return crate::dialogs_view_dxf::run(app, *dashed, *tolerance);
    }
    // Saving asks for a file name first; `Err("")` is the user backing out of that.
    if let Kind::SaveMesh(k) = &d.kind {
        return crate::dialogs_save_mesh::run(app, k, &d.inputs);
    }
    let commands = apply_commands(app, d)?;
    if let Some((_, marker)) = d.editing {
        // Record the actual pre-edit timeline state in the command's undo snapshot.
        roll_marker(app, marker)?;
    }
    for (cmd, params) in commands {
        match app.run(&cmd, params) {
            Ok(reply) => {
                // A drive that stopped short says where, and why.
                if cmd == "joint.drive"
                    && let Some(why) = crate::dialogs_assembly::drive_stop(&reply)
                {
                    app.set_status(why, false);
                }
            }
            Err(error) => {
                if let Some((id, _)) = d.editing {
                    let position = app.session.doc.feature_index(id);
                    let _ = roll_marker(app, position);
                }
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Placement used to edit a definition in the selected authoring occurrence.
pub(crate) fn feature_frame(s: &Session, component: u64) -> solidcraft_engine::doc::Mat {
    if component == s.active_component { s.active_component_frame() } else { s.placed_document().component_transform(component) }
}

/// The feature dialog's current operation name for a reviewed issue draft.
pub(crate) fn report_title(d: &Dialog) -> String {
    title(&d.kind).to_string()
}

/// The commands OK runs (an edit rebuilds its feature in place); the live preview runs the same.
pub fn apply_commands(app: &SolveApp, d: &Dialog) -> Result<Vec<(String, Value)>, String> {
    let cmds = dialog_commands(app, d)?;
    if let Some((id, _)) = d.editing {
        let (cmd, mut params) = cmds.into_iter().next().ok_or("nothing to apply")?;
        if let Some(feature) = app.session.doc.feature(id) {
            let original_all = match &feature.kind {
                FeatureKind::Extrude { sketch, profiles: ProfileSel::All, thin: None, .. }
                | FeatureKind::Revolve { sketch, profiles: ProfileSel::All, .. }
                | FeatureKind::Emboss { sketch, profiles: ProfileSel::All, .. } => Some(*sketch),
                _ => None,
            };
            if let Some(sketch) = original_all
                && !d.profile_selection_changed
                && let Some(ss) = app.session.world_state().sketch(sketch.0)
            {
                let mut selected: Vec<usize> = sels(d, 0)
                    .iter()
                    .filter_map(|item| match item {
                        Sel::Profile { sketch: id, index } if *id == sketch => Some(*index),
                        _ => None,
                    })
                    .collect();
                selected.sort_unstable();
                selected.dedup();
                if selected == (0..ss.profiles.len()).collect::<Vec<_>>() && sels(d, 0).len() == selected.len() {
                    params["profiles"] = json!("all");
                }
            }
        }
        return Ok(vec![("timeline.redefine".into(), json!({"feature": id, "command": cmd, "params": params}))]);
    }
    Ok(cmds)
}

/// The commands the live preview runs: OK's, with a joint's motion while it animates.
pub fn preview_commands(app: &SolveApp, d: &Dialog) -> Result<Vec<(String, Value)>, String> {
    let mut cmds = apply_commands(app, d)?;
    if let Kind::Assembly(k) = &d.kind {
        crate::dialogs_assembly::animate(app, k, &mut cmds);
    }
    Ok(cmds)
}

/// A wrapped Emboss edit keeps its profile guide in the saved author occurrence.
/// An invalid saved context must not draw the guide at a different occurrence.
pub(crate) fn emboss_profile_plane(app: &SolveApp, sketch: u64) -> Result<Option<solidcraft_engine::geom::Plane>, String> {
    let Some(dialog) = app.dialog.as_ref().filter(|d| matches!(d.kind, Kind::Emboss { .. })) else { return Ok(None) };
    let Some((feature, _)) = dialog.editing else { return Ok(None) };
    let Some(feature) = app.session.doc.feature(feature) else { return Err("the edited feature is missing".into()) };
    let FeatureKind::Emboss { sketch: source, face: Some(face), .. } = &feature.kind else { return Ok(None) };
    if *source != sketch {
        return Ok(None);
    }
    let model = app.session.model.state();
    let sketch = model.sketch(sketch).ok_or("the Emboss sketch is missing")?;
    let body = model.body(&face.body).ok_or("the Emboss target body is missing")?;
    let owner = app.session.doc.feature(sketch.feature).map(|feature| feature.component).unwrap_or(0);
    let plane = sketch.plane;
    let frame = face.sketch_frame(&app.session.doc, feature.component, owner, body).map_err(|error| error.to_string())?;
    solidcraft_engine::geom::Plane::new(
        solidcraft_engine::doc::apply_point(&frame, plane.origin),
        solidcraft_engine::doc::apply_vector(&frame, plane.x),
        solidcraft_engine::doc::apply_vector(&frame, plane.y),
    )
    .map(Some)
    .ok_or_else(|| "the Emboss author frame is degenerate".into())
}

/// The commands (id, parameters) a dialog's OK runs.
fn dialog_commands(app: &SolveApp, d: &Dialog) -> Result<Vec<(String, Value)>, String> {
    let s = &app.session;
    let need = |i: usize, what: &str| -> Result<(), String> { if sels(d, i).is_empty() { Err(format!("select {what} first")) } else { Ok(()) } };
    let profiles = || -> (Value, Value) {
        let ps: Vec<(u64, usize)> =
            sels(d, 0).iter().filter_map(|x| if let Sel::Profile { sketch, index } = x { Some((*sketch, *index)) } else { None }).collect();
        (ps.first().map(|x| json!(x.0)).unwrap_or(Value::Null), json!(ps.iter().map(|x| x.1).collect::<Vec<_>>()))
    };
    let body_names =
        |i: usize| -> Vec<String> { sels(d, i).iter().filter_map(|x| if let Sel::Body { name } = x { Some(name.clone()) } else { None }).collect() };
    let face_points = |i: usize| -> Vec<Value> {
        sels(d, i).iter().filter_map(|x| if let Sel::Face { point, .. } = x { Some(pt(*point)) } else { None }).collect()
    };
    let (cmd, params): (&str, Value) = match &d.kind {
        Kind::Extrude {
            distance,
            direction,
            operation,
            distance2,
            taper,
            taper_present,
            to_offset,
            start,
            all,
            thin,
            wall,
            wall_side,
            wall2,
            ..
        } => {
            need(0, if *thin { "curves or profiles" } else { "profiles or a planar face" })?;
            let mut common = json!({"distance": distance, "direction": DIRS.get(*direction).copied().unwrap_or("positive"), "operation": OPS.get(*operation).copied().unwrap_or("new")});
            let tapered = s.doc.eval(taper, ValueKind::Angle).map(|v| v.abs() > 1e-12).unwrap_or(!taper.trim().is_empty());
            if *direction == TWO_SIDES {
                common["distance2"] = json!(distance2);
            } else if *direction < 2 && (*taper_present || tapered) {
                common["taper"] = json!(taper);
            }
            if !start.trim().is_empty() {
                common["start_offset"] = json!(start);
            }
            if *all {
                common["through_all"] = json!(true);
            }
            if to_object_on(&d.inputs) {
                common["to"] = to_object_point(&d.inputs).ok_or("select the object to extrude to")?;
                if !to_offset.trim().is_empty() {
                    common["to_offset"] = json!(to_offset);
                }
            }
            if object_on(&d.inputs, FROM_OBJECT) {
                common["from"] = object_point(&d.inputs, FROM_OBJECT).ok_or("select the object to extrude from")?;
            }
            let with = |extra: Value| -> Value {
                let mut p = common.clone();
                if let (Value::Object(m), Value::Object(e)) = (&mut p, extra) {
                    m.extend(e);
                    for (k, v) in &d.extra {
                        m.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                p
            };
            if *thin {
                // The picked curves, and the outlines (outer and holes) of picked profiles.
                let st = s.world_state();
                let mut curves = curve_ids(d, 0);
                let mut sketch = None;
                for x in sels(d, 0) {
                    if let Sel::Profile { sketch: id, index } = x
                        && let Some(p) = st.sketch(*id).and_then(|ss| ss.profiles.get(*index))
                    {
                        sketch = Some(*id);
                        curves.extend(p.curve_ids());
                    }
                }
                let mut seen = std::collections::BTreeSet::new();
                curves.retain(|c| seen.insert(c.clone()));
                // A picked profile's sketch, or the edited extrude's, when it has every curve.
                let edited = d.editing.and_then(|(id, _)| s.doc.feature(id)).and_then(|f| match f.kind {
                    FeatureKind::Extrude { sketch, .. } => Some(sketch),
                    _ => None,
                });
                let has_all = |id: u64| curves.iter().all(|c| st.sketch(id).is_some_and(|ss| ss.sketch.curve_index(c).is_some()));
                let sketch = match sketch.or(edited.map(u64::from)) {
                    Some(id) if has_all(id) => id,
                    _ => curves_sketch(app, &curves, None).ok_or("the curves must be in one sketch")?,
                };
                let mut wall_params = json!({"sketch": sketch, "thin": true, "curves": curves, "thickness": wall, "side": WALL_SIDES.get(*wall_side).copied().unwrap_or("center")});
                if *wall_side == 3 {
                    wall_params["thickness2"] = json!(wall2);
                }
                return Ok(vec![("solid.extrude".to_string(), with(wall_params))]);
            }
            let mut out = Vec::new();
            let (sketch, idx) = profiles();
            if !sketch.is_null() {
                out.push(("solid.extrude".to_string(), with(json!({"sketch": sketch, "profiles": idx}))));
            }
            // Coplanar faces of one body share one sketch and Boolean. Mixed selections
            // retain their original order and explicit face-body scope.
            let state = s.world_state();
            let mut group: Option<(String, solidcraft_engine::geom::Plane, Vec<Value>)> = None;
            let mut coplanar = true;
            for pick in sels(d, 0) {
                let Sel::Face { body, index, point } = pick else { continue };
                let Some(source) = state.body(body) else { return Err("selected body disappeared".into()) };
                let Some(plane) = source.body.planar_face(*index) else {
                    coplanar = false;
                    break;
                };
                let tolerance = (source.body.size() * 1e-9).max(1e-8);
                if let Some((name, base, points)) = &mut group {
                    if name != body || base.normal().dot(plane.normal()) <= 1.0 - 1e-9 || base.height(plane.origin).abs() > tolerance {
                        coplanar = false;
                        break;
                    }
                    points.push(pt(*point));
                } else {
                    group = Some((body.clone(), plane, vec![pt(*point)]));
                }
            }
            if coplanar
                && let Some((body, _, points)) = group
                && points.len() > 1
            {
                out.push(("solid.extrude".to_string(), with(json!({"faces": points, "face_body": body}))));
                return Ok(out);
            }
            for pick in sels(d, 0) {
                if let Sel::Face { body, point, .. } = pick {
                    out.push(("solid.extrude".to_string(), with(json!({"face": pt(*point), "face_body": body}))));
                }
            }
            return Ok(out);
        }
        Kind::Revolve { angle, operation, axis: stored_axis } => {
            need(0, "profiles")?;
            let axis = match sels(d, 1).first() {
                Some(Sel::Axis { name }) => json!(name),
                Some(Sel::SketchCurve { id }) => json!(id),
                _ => stored_axis.clone().ok_or("select an axis")?,
            };
            let (sketch, idx) = profiles();
            let mut p =
                json!({"sketch": sketch, "profiles": idx, "axis": axis, "angle": angle, "operation": OPS.get(*operation).copied().unwrap_or("new")});
            if to_object_on(&d.inputs) {
                p["to"] = to_object_point(&d.inputs).ok_or("select the object to turn to")?;
            }
            ("solid.revolve", p)
        }
        Kind::Fillet { radius, chamfer, ctype, distance2, angle, flip, .. } => {
            need(0, "edges")?;
            // Faces stand for all their edges.
            let st = s.world_state();
            let mut pts: Vec<Vec3> = Vec::new();
            for x in sels(d, 0) {
                match x {
                    Sel::Edge { point, .. } => pts.push(*point),
                    Sel::Face { body, index, .. } => {
                        if let Some(b) = st.body(body) {
                            let m = b.mesh();
                            for e in m.face_edges(u32::try_from(*index).unwrap_or(u32::MAX)) {
                                if !m.seams.get(e).copied().unwrap_or(false)
                                    && let Some(p) = m.edges.get(e)
                                {
                                    pts.push(polyline_mid(p));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            let mut uniq: Vec<Vec3> = Vec::new();
            for p in pts {
                if !uniq.iter().any(|q| q.dist(p) < 1e-9) {
                    uniq.push(p);
                }
            }
            let edges: Vec<Value> = uniq.into_iter().map(pt).collect();
            if *chamfer {
                let mut p = json!({"edges": edges, "distance": radius});
                match *ctype {
                    1 => p["distance2"] = json!(distance2),
                    2 => p["angle"] = json!(angle),
                    _ => {}
                }
                if *ctype > 0 && *flip {
                    p["flip"] = json!(true);
                }
                ("solid.chamfer", p)
            } else {
                let mut p = json!({"edges": edges, "radius": radius});
                let style = d.extra.get("type").and_then(Value::as_str);
                if matches!(style, Some("variable" | "spline")) {
                    // The start radius goes at one end of the first edge (Swap Ends: the other).
                    let ends = sels(d, 0).iter().find_map(|x| match x {
                        Sel::Edge { body, index, .. } => {
                            st.body(body).and_then(|b| b.mesh().edges.get(*index).and_then(|e| Some((*e.first()?, *e.last()?))))
                        }
                        _ => None,
                    });
                    let swap = d.extra.get("swap_ends").and_then(Value::as_bool).unwrap_or(false);
                    if let Some((a, b)) = ends {
                        p["start"] = pt(if swap { b } else { a });
                    }
                    let mids = mid_points(&d.extra);
                    if style == Some("variable") && !mids.is_empty() {
                        p["mid"] = json!(mids.iter().map(|(t, r)| json!([t, r])).collect::<Vec<_>>());
                        if d.extra.get("smooth").and_then(Value::as_bool).unwrap_or(false) {
                            p["smooth"] = json!(true);
                        }
                    }
                }
                ("solid.fillet", p)
            }
        }
        Kind::Shell { thickness } => {
            need(0, "faces to remove")?;
            ("solid.shell", json!({"faces": face_points(0), "thickness": thickness}))
        }
        Kind::Draft { angle } => {
            need(0, "faces")?;
            need(1, "the neutral plane")?;
            let neutral = plane_value(s, sels(d, 1).first()).ok_or("the neutral plane must be a plane or a planar face")?;
            ("solid.draft", json!({"faces": face_points(0), "angle": angle, "neutral": neutral}))
        }
        Kind::Mirror => {
            need(0, "objects")?;
            need(1, "the mirror plane")?;
            let plane = plane_value(s, sels(d, 1).first()).ok_or("the mirror plane must be a plane or a planar face")?;
            let mut p = json!({"plane": plane});
            pattern_objects(s, sels(d, 0), &mut p);
            ("solid.mirror", p)
        }
        Kind::PatternRect { count, spacing, count2, spacing2 } => {
            need(0, "objects")?;
            let kept = d.extra.get("dir1").and_then(|v| serde_json::from_value::<[f64; 3]>(v.clone()).ok()).map(|a| Vec3::new(a[0], a[1], a[2]));
            let d1 = match (sels(d, 1).first(), kept) {
                (Some(x), _) => axis_of(app, x).ok_or("the direction must be an axis, a sketch line or a straight edge")?.1,
                (None, Some(k)) => k,
                (None, None) => return Err("select a direction (an axis, a sketch line or a straight edge) first".into()),
            };
            let mut p = json!({"dir1": pt(d1), "count1": count, "spacing1": spacing});
            pattern_objects(s, sels(d, 0), &mut p);
            let kept2 = d.extra.get("dir2").and_then(|v| serde_json::from_value::<[f64; 3]>(v.clone()).ok()).map(|a| Vec3::new(a[0], a[1], a[2]));
            let d2 = match sels(d, 2).first() {
                Some(x) => Some(axis_of(app, x).ok_or("the second direction must be an axis, a sketch line or a straight edge")?.1),
                None => kept2,
            };
            if let Some(d2) = d2 {
                p["dir2"] = pt(d2);
                p["count2"] = json!(count2);
                p["spacing2"] = json!(spacing2);
            }
            ("solid.pattern.rectangular", p)
        }
        Kind::PatternCirc { count, angle } => {
            need(0, "objects")?;
            let axis = match sels(d, 1).first() {
                None if d.extra.contains_key("axis") => d.extra.get("axis").cloned().unwrap_or(Value::Null),
                Some(Sel::Axis { name }) => json!(name),
                Some(x) => {
                    let (o, dir) = axis_of(app, x).ok_or("the axis must be an origin axis or a sketch line")?;
                    json!({"origin": pt(o), "dir": pt(dir)})
                }
                None => return Err("select an axis".into()),
            };
            let mut p = json!({"axis": axis, "count": count, "angle": angle});
            pattern_objects(s, sels(d, 0), &mut p);
            ("solid.pattern.circular", p)
        }
        Kind::Loft { operation } => {
            need(0, "profiles of two or more sketches")?;
            // One section per sketch, in the order the sketches were first picked.
            let mut sections: Vec<(u64, Vec<usize>)> = Vec::new();
            for x in sels(d, 0) {
                if let Sel::Profile { sketch, index } = x {
                    match sections.iter_mut().find(|(sk, _)| sk == sketch) {
                        Some((_, v)) => v.push(*index),
                        None => sections.push((*sketch, vec![*index])),
                    }
                }
            }
            if sections.len() < 2 {
                return Err("pick profiles in two or more sketches".into());
            }
            let sections: Vec<Value> = sections.into_iter().map(|(sk, v)| json!({"sketch": sk, "profiles": v})).collect();
            ("solid.loft", json!({"sections": sections, "operation": OPS.get(*operation).copied().unwrap_or("new")}))
        }
        Kind::Sweep { operation, opts } => {
            need(0, "a profile")?;
            need(1, "the path")?;
            let (sketch, idx) = profiles();
            let path: Vec<String> = sels(d, 1).iter().filter_map(|x| if let Sel::SketchCurve { id } = x { Some(id.clone()) } else { None }).collect();
            let path_sketch = curves_sketch(app, &path, sketch.as_u64()).ok_or("the path curves must be in one sketch")?;
            let mut p = json!({"sketch": sketch, "profiles": idx, "path_sketch": path_sketch, "path": path, "operation": OPS.get(*operation).copied().unwrap_or("new")});
            crate::dialogs_sweep::params(app, opts, sels(d, 2).first(), [sels(d, 3), sels(d, 4)], &mut p)?;
            ("solid.sweep", p)
        }
        Kind::OffsetPlane { offset } => {
            need(0, "a plane")?;
            let base = plane_arg(app, sels(d, 0).first())?;
            ("construct.plane.offset", json!({"base": base, "offset": offset}))
        }
        Kind::Midplane => {
            need(0, "the first plane")?;
            need(1, "the second plane")?;
            let a = plane_arg(app, sels(d, 0).first())?;
            let b = plane_arg(app, sels(d, 1).first())?;
            ("construct.plane.midplane", json!({"a": a, "b": b}))
        }
        Kind::PathPlane { t, sketch } => {
            need(0, "a path")?;
            let path = match sels(d, 0).first() {
                Some(Sel::SketchCurve { id }) => {
                    let st = app.session.world_state();
                    let matches: Vec<u64> = st
                        .sketches
                        .iter()
                        .filter(|ss| ss.sketch.curve_index(id).is_some() || ss.sketch.wire_index(id).is_some())
                        .map(|ss| ss.feature)
                        .collect();
                    let source = match sketch {
                        Some(source) if matches.contains(source) => *source,
                        None if matches.len() == 1 => *matches.first().ok_or("pick a path")?,
                        _ => return Err("pick the path in the viewport to identify its source sketch".into()),
                    };
                    json!({"sketch": source, "curve": id})
                }
                Some(Sel::Edge { point, .. }) => d.extra.get("path").cloned().unwrap_or_else(|| json!({"edge": pt(*point)})),
                _ => return Err("pick a sketch curve or model edge".into()),
            };
            ("construct.plane.along_path", json!({"path": path, "t": t}))
        }
        Kind::AnglePlane { angle } => {
            need(0, "a plane")?;
            let base = match sels(d, 0).first() {
                Some(Sel::Plane { name }) => name.clone(),
                _ => return Err("pick an origin or construction plane".into()),
            };
            let axis = match sels(d, 1).first() {
                Some(Sel::Axis { name }) => json!(name),
                Some(x) => {
                    let (o, dir) = axis_of(app, x).ok_or("the axis must be an origin axis or a sketch line")?;
                    json!({"origin": pt(o), "dir": pt(dir)})
                }
                None => d.extra.get("axis").cloned().ok_or("pick an axis")?,
            };
            ("construct.plane.at_angle", json!({"base": base, "axis": axis, "angle": angle}))
        }
        Kind::Rib { web, thickness, depth, flip, normal } => {
            need(0, "open sketch curves")?;
            let curves = curve_ids(d, 0);
            let sketch = curves_sketch(app, &curves, None).ok_or("the curves must be in one sketch")?;
            let mut p = json!({"sketch": sketch, "curves": curves, "thickness": thickness, "flip": flip});
            if *normal {
                p["direction"] = json!("normal");
            }
            if !depth.trim().is_empty() {
                p["depth"] = json!(depth);
            }
            (if *web { "solid.web" } else { "solid.rib" }, p)
        }
        Kind::Emboss { depth, deboss } => {
            need(0, "profiles")?;
            let (sketch, idx) = profiles();
            let mut p = json!({"sketch": sketch, "profiles": idx, "depth": depth, "mode": if *deboss { "deboss" } else { "emboss" }});
            if let Some(Sel::Face { body, point, .. }) = sels(d, 1).first() {
                p["face"] = pt(*point);
                p["targets"] = json!([body]);
            }
            ("solid.emboss", p)
        }
        Kind::ReplaceFace => {
            need(0, "faces to move")?;
            need(1, "the target")?;
            let mut p = json!({"faces": face_points(0)});
            match sels(d, 1).first() {
                Some(Sel::Plane { name }) => p["target"] = json!(name),
                Some(Sel::Face { point, .. }) => p["target_face"] = pt(*point),
                _ => return Err("pick a plane or planar face".into()),
            }
            ("solid.replace_face", p)
        }
        Kind::Align { flip } => {
            need(0, "bodies")?;
            need(1, "what to move from")?;
            need(2, "where to move it to")?;
            let mut p = json!({"bodies": body_names(0), "flip": flip});
            for (i, key) in [(1, "from"), (2, "to")] {
                let snap = d.extra.get(&format!("{key}_snap")).and_then(Value::as_str).unwrap_or("picked");
                match (sels(d, i).first(), snap) {
                    (Some(Sel::Face { point, .. }), "center") => p[key] = json!({"snap": "face_center", "at": pt(*point)}),
                    (Some(Sel::Face { point, .. } | Sel::Edge { point, .. }), "circle") => {
                        p[key] = json!({"snap": "circle_center", "at": pt(*point)})
                    }
                    (Some(Sel::Face { point, .. }), _) => p[format!("{key}_face")] = pt(*point),
                    (Some(Sel::Edge { point, .. }), _) => p[key] = json!({"snap": "edge_mid", "at": pt(*point)}),
                    (Some(Sel::Vertex { point, .. }), _) => p[key] = pt(*point),
                    _ => return Err("pick faces, edges or vertices".into()),
                }
            }
            ("solid.align", p)
        }
        Kind::Remove => {
            need(0, "bodies")?;
            ("solid.remove", json!({"bodies": body_names(0)}))
        }
        Kind::PathPattern { count, spacing } => {
            need(0, "objects")?;
            need(1, "the path")?;
            let path = curve_ids(d, 1);
            let sketch = curves_sketch(app, &path, None).ok_or("the path curves must be in one sketch")?;
            let mut p = json!({"path_sketch": sketch, "path": path, "count": count, "spacing": spacing});
            pattern_objects(s, sels(d, 0), &mut p);
            ("solid.pattern.path", p)
        }
        Kind::Pipe { diameter, wall } => {
            need(0, "the path")?;
            let path = curve_ids(d, 0);
            let sketch = curves_sketch(app, &path, None).ok_or("the path curves must be in one sketch")?;
            let mut p = json!({"path_sketch": sketch, "path": path, "diameter": diameter});
            if !wall.trim().is_empty() {
                p["wall"] = json!(wall);
            }
            ("solid.pipe", p)
        }
        Kind::Stock { margin } => {
            let mut p = json!({"margin": margin});
            if !sels(d, 0).is_empty() {
                p["bodies"] = json!(body_names(0));
            }
            ("solid.bounding_solid", p)
        }
        Kind::Split => {
            need(0, "the body to split")?;
            need(1, "the splitting plane")?;
            let body = body_names(0).into_iter().next().unwrap_or_default();
            // A plane, a planar face's plane, or any other face (its surface extended).
            let p = match sels(d, 1).first() {
                Some(Sel::Plane { name }) => json!({"body": body, "plane": name}),
                Some(Sel::Face { body: b, index, point }) => match planar_face(s, b, *index) {
                    Some((_, n)) => json!({"body": body, "plane": {"origin": pt(*point), "normal": pt(n)}}),
                    None => json!({"body": body, "tool": {"body": b, "point": pt(*point)}}),
                },
                // A straight sketch line, in the sketch it was picked in (#1552).
                Some(Sel::SketchCurve { id }) => {
                    let picked = d.extra.get("tool_sketch").and_then(Value::as_u64);
                    let st = s.world_state();
                    let holding: Vec<u64> = st.sketches.iter().filter(|ss| ss.sketch.curve_index(id).is_some()).map(|ss| ss.feature).collect();
                    let sketch = match picked {
                        Some(k) if holding.contains(&k) => k,
                        None if holding.len() == 1 => holding.first().copied().ok_or("pick a sketch line")?,
                        _ => return Err("pick the sketch line in the viewport".into()),
                    };
                    json!({"body": body, "sketch": sketch, "line": id})
                }
                _ => return Err("pick a plane, a face or a sketch line".into()),
            };
            ("solid.split_body", p)
        }
        Kind::Scale { factor } => {
            need(0, "bodies")?;
            ("solid.scale", json!({"bodies": body_names(0), "factor": factor}))
        }
        Kind::OffsetFaces { distance } => {
            need(0, "planar faces")?;
            ("solid.offset_face", json!({"faces": face_points(0), "distance": distance}))
        }
        Kind::Thread { designation, length, suggested, .. } => {
            need(0, "a cylindrical face")?;
            let mut p = json!({"face": face_points(0).into_iter().next().unwrap_or(Value::Null)});
            if !designation.trim().is_empty() && suggested.as_deref() != Some(designation.as_str()) {
                p["designation"] = json!(designation.trim());
            }
            if !length.trim().is_empty() {
                p["length"] = json!(length);
            }
            ("solid.thread", p)
        }
        Kind::Material { index } => {
            need(0, "bodies")?;
            let name = s.doc.material_names().get(*index).cloned().unwrap_or_else(|| "Default".into());
            ("material.assign", json!({"bodies": body_names(0), "material": name}))
        }
        Kind::Section { offset, flip } => {
            need(0, "a plane or planar face")?;
            let plane = match sels(d, 0).first() {
                Some(Sel::Plane { name }) if matches!(name.as_str(), "XY" | "XZ" | "YZ") => json!(name),
                Some(Sel::Face { body, index, point }) => {
                    let (_, n) = planar_face(s, body, *index).ok_or("the face must be planar")?;
                    json!({"origin": pt(*point), "normal": pt(n)})
                }
                Some(Sel::Plane { name }) => {
                    let pl = solidcraft_engine::view::construction_planes(s)
                        .into_iter()
                        .find(|(_, n, _)| n == name)
                        .map(|(_, _, pl)| pl)
                        .ok_or("unknown plane")?;
                    json!({"origin": pt(pl.origin), "normal": pt(pl.normal())})
                }
                _ => return Err("pick a plane or a planar face".into()),
            };
            ("inspect.section", json!({"plane": plane, "offset": offset, "flip": flip}))
        }
        Kind::Move { .. } => return Ok(vec![("solid.move".into(), crate::dialogs_move::params(app, d)?)]),
        Kind::Hole { diameter, depth, kind, cb_diameter, cb_depth, cs_diameter, cs_angle, opts } => {
            need(0, if opts.multiple { "sketch points" } else { "a face position" })?;
            let ty = match *kind {
                0 if opts.angled => "drilled",
                k => HOLE_TYPES.get(k).copied().unwrap_or("simple"),
            };
            // One hole per picked face position, or one Hole feature per sketch for sketch points.
            let mut places: Vec<Value> = Vec::new();
            if opts.multiple {
                let mut by_sketch: Vec<(u64, Vec<String>)> = Vec::new();
                for x in sels(d, 0) {
                    if let Sel::SketchPoint { id } = x
                        && let Some((sk, pid)) = id.split_once(':')
                        && let Ok(sk) = sk.parse::<u64>()
                    {
                        match by_sketch.iter_mut().find(|(s, _)| *s == sk) {
                            Some((_, v)) => v.push(pid.to_string()),
                            None => by_sketch.push((sk, vec![pid.to_string()])),
                        }
                    }
                }
                let all_points = d.extra.get("all_points").and_then(Value::as_bool).unwrap_or(false);
                let flip = d.extra.get("flip").and_then(Value::as_bool).unwrap_or(false);
                places.extend(
                    by_sketch
                        .into_iter()
                        .map(|(sk, ids)| if all_points { json!({ "sketch": sk }) } else { json!({"sketch": sk, "points": ids}) })
                        .map(|mut place| {
                            if flip {
                                place["flip"] = json!(true);
                            }
                            place
                        }),
                );
            } else {
                // One hole per pick: a curved surface split at its seam is picked whole (both
                // halves, the clicked one first), but the hole goes where it was clicked only.
                let mut taken: Vec<Sel> = Vec::new();
                for x in sels(d, 0) {
                    if !matches!(x, Sel::Face { .. }) || taken.iter().any(|t| same_face(t, x)) {
                        continue;
                    }
                    taken.extend(crate::viewport::grouped_selection(s, x));
                    let kept = d.extra.get("position_exprs").filter(|k| face_point(x).is_some_and(|p| p == k["point"])).map(|k| k["at"].clone());
                    places.extend(face_point(x).map(|p| json!({ "position": kept.unwrap_or(p) })));
                }
            }
            let mut out = Vec::new();
            for place in places {
                let mut params = place;
                params["diameter"] = json!(diameter);
                params["type"] = json!(ty);
                if to_object_on(&d.inputs) {
                    params["to"] = to_object_point(&d.inputs).ok_or("select the object to drill to")?;
                } else if !opts.all {
                    params["depth"] = json!(depth);
                }
                if ty == "drilled" {
                    params["tip_angle"] = json!(opts.tip_angle);
                }
                if opts.tapped && !opts.thread.trim().is_empty() {
                    params["thread"] = json!(opts.thread.trim());
                }
                match *kind {
                    1 => {
                        params["cb_diameter"] = json!(cb_diameter);
                        params["cb_depth"] = json!(cb_depth);
                    }
                    2 => {
                        params["cs_diameter"] = json!(cs_diameter);
                        params["cs_angle"] = json!(cs_angle);
                    }
                    _ => {}
                }
                out.push(("solid.hole".to_string(), params));
            }
            return Ok(out);
        }
        Kind::Primitive { cmd, fields, operation, at } => {
            let mut p = serde_json::Map::new();
            for (k, v) in fields {
                p.insert((*k).into(), json!(v));
            }
            if let Some((point, normal)) = *at {
                match *cmd {
                    "solid.box" => {
                        p.insert("corner".into(), box_corner(s, fields, point, normal));
                    }
                    "solid.cylinder" | "solid.coil" => {
                        p.insert("base".into(), pt(point));
                        if let Some(n) = normal {
                            p.insert("axis".into(), pt(n));
                        }
                    }
                    _ => {
                        p.insert("center".into(), pt(point));
                    }
                }
            }
            p.insert("operation".into(), json!(OPS.get(*operation).copied().unwrap_or("new")));
            (cmd, Value::Object(p))
        }
        Kind::BoundaryFill { operation, cells, .. } => {
            need(0, "the tool bodies")?;
            let picked: Vec<Value> = cells.iter().filter(|c| c.2).map(|c| pt(c.0)).collect();
            if picked.is_empty() {
                return Err("tick the cells to fill".into());
            }
            ("solid.boundary_fill", json!({"tools": body_names(0), "cells": picked, "operation": OPS.get(*operation).copied().unwrap_or("new")}))
        }
        Kind::Combine { operation, keep_tools } => {
            need(0, "the target body")?;
            need(1, "tool bodies")?;
            let target = body_names(0).into_iter().next().unwrap_or_default();
            let tools: Vec<String> = body_names(1).into_iter().filter(|n| *n != target).collect();
            (
                "solid.combine",
                json!({"target": target, "tools": tools, "operation": OPS.get(*operation).copied().unwrap_or("join"), "keep_tools": keep_tools}),
            )
        }
        Kind::EditParam { name, expr } => ("parameters.change", json!({"name": name, "expression": expr})),
        Kind::Rename { feature, name } => ("timeline.rename", json!({"feature": feature, "name": name})),
        Kind::ConfirmDelete { features, .. } => ("timeline.delete", json!({ "features": features })),
        Kind::Sketch | Kind::Params { .. } | Kind::Measure { .. } => return Ok(Vec::new()),
        Kind::Assembly(k) => return crate::dialogs_assembly::commands(app, k, &d.inputs),
        Kind::Sheet(k) => return crate::dialogs_sheet::commands(app, k, &d.inputs, &d.extra),
        Kind::Surface(k) => return crate::dialogs_surface::commands(app, k, &d.inputs, &d.extra),
        Kind::SketchModify(k) => return crate::sketch_modify_ui::commands(app, k, &d.inputs, &d.extra),
        Kind::SaveMesh(k) => return crate::dialogs_save_mesh::commands(k, &d.inputs),
        Kind::ViewDxf { .. } => return Ok(Vec::new()),
        Kind::Plastic(k) => return crate::dialogs_plastic::commands(app, k, &d.inputs, &d.extra),
        Kind::Appearance(k) => return Ok(crate::dialogs_appearance::commands(k, &d.inputs)),
        Kind::Motion(k) => return crate::dialogs_motion::commands(app, k, &d.inputs),
        Kind::Part(k) => return crate::dialogs_parts::commands(app, k, &d.inputs),
        Kind::Text(k) => return crate::sketch_text::commands(app, k),
        Kind::TextMove(k) => return crate::sketch_text_move::commands(app, k),
    };
    let mut params = params;
    if let Value::Object(m) = &mut params {
        for (k, v) in &d.extra {
            m.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Ok(vec![(cmd.to_string(), params)])
}

/// The corner of a box placed at `p`: standing on a face whose normal is along an axis (centred
/// on `p` across the face), otherwise centred on `p`. Sizes that are expressions stay
/// expressions, so the box keeps standing on the face when they change.
fn box_corner(s: &Session, fields: &[(&'static str, String)], p: Vec3, normal: Option<Vec3>) -> Value {
    let size = |k: &str| fields.iter().find(|f| f.0 == k).map(|f| s.doc.with_design_unit(&f.1, ValueKind::Length)).unwrap_or_default();
    let along = |axis: Vec3| normal.map(|n| n.dot(axis)).filter(|d| d.abs() > 1.0 - 1e-9);
    let coord = |c: f64, size: String, axis: Vec3| {
        // How much of the size lies behind `p`: none (or all) along the face normal, half across it.
        let back = match along(axis) {
            Some(d) if d > 0.0 => return json!(c),
            Some(_) => 1.0,
            None => 0.5,
        };
        // A plain length ("20 mm", "2 in"; a bare number has the design's unit by now) gives a
        // plain number (mm).
        let plain = solidcraft_engine::doc::expr::is_literal(&size)
            .then(|| solidcraft_engine::doc::Document::eval_in(&Default::default(), &size, ValueKind::Length).ok())
            .flatten();
        match plain.filter(|v| v.is_finite()) {
            Some(v) => json!(c - back * v),
            None if back < 1.0 => json!(format!("{c} mm - ({size}) / 2")),
            None => json!(format!("{c} mm - ({size})")),
        }
    };
    json!([coord(p.x, size("length"), Vec3::X), coord(p.y, size("width"), Vec3::Y), coord(p.z, size("height"), Vec3::Z)])
}

/// Switch an extrude's first input between profiles and faces, and (thin) profiles and curves;
/// picks the other kind doesn't take are dropped.
fn set_thin(inputs: &mut [SelInput], thin: bool) {
    if let Some(input) = inputs.first_mut() {
        input.accept = if thin { THIN_ACCEPT } else { EXTRUDE_ACCEPT };
        input.label = if thin { "Profiles/Curves" } else { "Profiles" };
        input.items.retain(|x| match x {
            Sel::Profile { .. } => true,
            Sel::SketchCurve { .. } => thin,
            Sel::Face { .. } => !thin,
            _ => false,
        });
    }
}

/// Sketch curve ids picked in an input.
fn curve_ids(d: &Dialog, i: usize) -> Vec<String> {
    sels(d, i).iter().filter_map(|x| if let Sel::SketchCurve { id } = x { Some(id.clone()) } else { None }).collect()
}

/// The features that made these bodies (what patterns and mirrors copy).
/// The features a pattern repeats: picked features as they are, picked bodies by the features
/// that made them.
/// What a pattern or mirror copies: picked features are replayed; picked bodies are copied as
/// they are now, with every feature that shaped them (a fillet after the extrude included).
fn pattern_objects(s: &Session, objects: &[Sel], p: &mut Value) {
    if objects.iter().any(|x| matches!(x, Sel::Feature { .. })) {
        p["features"] = json!(pattern_features(s, objects));
    } else {
        let bodies: Vec<&str> = objects.iter().filter_map(|x| if let Sel::Body { name } = x { Some(name.as_str()) } else { None }).collect();
        p["bodies"] = json!(bodies);
    }
}

fn pattern_features(s: &Session, objects: &[Sel]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bodies: Vec<String> = objects.iter().filter_map(|x| if let Sel::Body { name } = x { Some(name.clone()) } else { None }).collect();
    for x in objects {
        if let Sel::Feature { id } = x
            && let Some(f) = s.doc.feature(*id)
            && !out.contains(&f.name)
        {
            out.push(f.name.clone());
        }
    }
    for f in source_features(s, &bodies) {
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

fn source_features(s: &Session, bodies: &[String]) -> Vec<String> {
    let st = s.world_state();
    let mut features: Vec<String> = Vec::new();
    for n in bodies {
        if let Some(f) = st.body(n).and_then(|b| s.doc.feature(b.feature)).map(|f| f.name.clone())
            && !features.contains(&f)
        {
            features.push(f);
        }
    }
    features
}

/// The sketch holding all these curve ids (the active sketch first, never `not`).
pub(crate) fn curves_sketch(app: &SolveApp, ids: &[String], not: Option<u64>) -> Option<u64> {
    let st = app.session.world_state();
    let has =
        |sid: u64| st.sketch(sid).is_some_and(|ss| ids.iter().all(|id| ss.sketch.curve_index(id).is_some() || ss.sketch.wire_index(id).is_some()));
    if let Some(a) = app.session.active_sketch
        && Some(a) != not
        && has(a)
    {
        return Some(a);
    }
    st.sketches.iter().rev().map(|ss| ss.feature).find(|sid| Some(*sid) != not && has(*sid))
}

/// An axis selection as a line: a point on it and its unit direction (origin axes, or a straight
/// sketch line).
pub fn axis_of(app: &SolveApp, sel: &Sel) -> Option<(Vec3, Vec3)> {
    match sel {
        Sel::Axis { name } => match name.as_str() {
            "X" => Some((Vec3::ZERO, Vec3::X)),
            "Y" => Some((Vec3::ZERO, Vec3::Y)),
            "Z" => Some((Vec3::ZERO, Vec3::Z)),
            // A construction axis.
            n => solidcraft_engine::view::construction_axes(&app.session).into_iter().find(|(_, an, _, _)| an == n).map(|(_, _, o, d)| (o, d)),
        },
        Sel::SketchCurve { id } => {
            let st = app.session.world_state();
            let sid = curves_sketch(app, std::slice::from_ref(id), None)?;
            let ss = st.sketch(sid)?;
            let c = ss.sketch.curves.get(ss.sketch.curve_index(id)?)?;
            let solidcraft_engine::sketch::CurveKind::Line { a, b } = c.kind else { return None };
            let (pa, pb) = (ss.plane.to_world(ss.sketch.point(a)?), ss.plane.to_world(ss.sketch.point(b)?));
            Some((pa, (pb - pa).normalized()?))
        }
        // A straight model edge, along it.
        Sel::Edge { body, index, .. } => {
            let b = app.session.world_state().body(body).cloned()?;
            let m = b.mesh();
            let e = m.edges.get(*index)?;
            let (a, z) = (*e.first()?, *e.last()?);
            let dir = (z - a).normalized()?;
            let tol = 1e-6 * a.dist(z).max(1.0);
            if !e.iter().all(|p| (*p - a).cross(dir).len() < tol) {
                return None;
            }
            // An edge along an origin axis points the way the axis does (as picking the axis).
            let axis = [Vec3::X, Vec3::Y, Vec3::Z].into_iter().find(|x| dir.dot(*x).abs() > 1.0 - 1e-9);
            Some((a, axis.unwrap_or(dir)))
        }
        _ => None,
    }
}

/// Which profiles of a sketch a profile selection means.
pub(crate) fn profile_indices(ss: &solidcraft_engine::doc::SolvedSketch, sel: &ProfileSel) -> Vec<usize> {
    let ps = &ss.profiles;
    match sel {
        ProfileSel::All => (0..ps.len()).collect(),
        ProfileSel::Indices { indices } => indices.iter().copied().filter(|i| *i < ps.len()).collect(),
        ProfileSel::OffsetBoundaries { .. } | ProfileSel::WholeBoundaries { .. } => {
            solidcraft_engine::doc::profile_selection_indices(ss, sel).unwrap_or_default()
        }
        ProfileSel::Curves { loops } => loops
            .iter()
            .filter_map(|l| {
                solidcraft_engine::doc::profile_selection_indices(ss, &ProfileSel::Curves { loops: vec![l.clone()] }).ok()?.first().copied()
            })
            .collect(),
        ProfileSel::Points { points } => points.iter().filter_map(|q| solidcraft_engine::doc::sketch::profile_at_point(ps, *q)).collect(),
    }
}

thread_local! {
    /// A dialog is being filled from a feature: its points are in component frames.
    static REFILL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While filling a dialog from a feature (dropped at the end).
struct Refill;

impl Refill {
    fn start() -> Refill {
        REFILL.with(|r| r.set(true));
        Refill
    }
}

impl Drop for Refill {
    fn drop(&mut self) {
        REFILL.with(|r| r.set(false));
    }
}

/// A point stored in a feature, as it shows on world body `body`: features keep their picks in
/// the frame of the body they were on, so each body's placement takes the point to the world
/// (a moved component, a pasted instance). Other points are the world already.
fn stored_on(frames: &Option<Vec<(String, solidcraft_engine::doc::Mat)>>, body: &str, p: Vec3) -> Vec3 {
    frames.as_ref().and_then(|f| f.iter().find(|(n, _)| n == body)).map(|(_, m)| solidcraft_engine::doc::apply_point(m, p)).unwrap_or(p)
}

/// Body placements while filling a dialog from a feature (when anything is placed).
fn stored_frames(s: &Session) -> Option<Vec<(String, solidcraft_engine::doc::Mat)>> {
    if !REFILL.with(std::cell::Cell::get) || !solidcraft_engine::frames::placed(s) {
        return None;
    }
    let st = s.model.state();
    Some(s.doc.body_frames(&st).into_iter().map(|(w, _, _, m)| (w, m)).collect())
}

/// The edge of a visible body through (or nearest) a point.
pub(crate) fn edge_sel(s: &Session, p0: Vec3) -> Option<Sel> {
    let st = s.world_state();
    let frames = stored_frames(s);
    let mut best: Option<(f64, Sel)> = None;
    for b in &st.bodies {
        let p = stored_on(&frames, &b.name, p0);
        let m = b.mesh();
        for (i, e) in m.edges.iter().enumerate() {
            let d = e.windows(2).map(|w| p.dist_to_segment(w[0], w[1])).fold(f64::INFINITY, f64::min);
            if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, Sel::Edge { body: b.name.clone(), index: i, point: p }));
            }
        }
    }
    best.map(|x| x.1)
}

/// The body face containing a point (nearest triangle).
pub(crate) fn face_sel(s: &Session, p0: Vec3) -> Option<Sel> {
    let st = s.world_state();
    let frames = stored_frames(s);
    let mut best: Option<(f64, Sel)> = None;
    for b in &st.bodies {
        let p = stored_on(&frames, &b.name, p0);
        let m = b.mesh();
        for (t, f) in m.triangles.iter().zip(&m.tri_face) {
            let Some([a, bb, c]) = m.tri(t) else { continue };
            let d = crate::preview::point_triangle_dist(p, a, bb, c);
            if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, Sel::Face { body: b.name.clone(), index: *f as usize, point: p }));
            }
        }
    }
    best.filter(|(d, _)| *d < 1e-3 + 1e-6 * p0.len()).map(|x| x.1)
}

fn emboss_face_sel(s: &Session, component: u64, face: &solidcraft_engine::doc::EmbossFace, persistent: &str) -> Option<Sel> {
    let source = s.model.state();
    let body = source.body(&face.body)?;
    let index = solidcraft_engine::doc::naming::face_names(body).iter().position(|name| name == persistent)?;
    let owner = s.doc.model_body_component(body);
    let world_name = s.doc.world_body_name(&source, &face.body, owner, &face.path);
    let author = solidcraft_engine::doc::EmbossFace::component_frame(&s.doc, component, &face.author_path).ok()?;
    let point = solidcraft_engine::doc::apply_point(&author, face.at.0);
    s.world_state().body(&world_name)?;
    Some(Sel::Face { body: world_name, index, point })
}

/// A picked plane or planar face as `construct.plane.*` takes it.
fn plane_arg(app: &SolveApp, sel: Option<&Sel>) -> Result<Value, String> {
    match sel {
        Some(Sel::Plane { name }) => Ok(json!(name)),
        Some(face @ Sel::Face { .. }) => crate::sketch_attachment_ui::face_plane(app, face).ok_or("the picked face is no longer in the model".into()),
        _ => Err("pick an origin or construction plane, or a planar face".into()),
    }
}

/// A stored construction reference back into a dialog pick.
fn geo_sel(s: &Session, g: &solidcraft_engine::doc::construct::GeoRef) -> Option<Sel> {
    match g {
        solidcraft_engine::doc::construct::GeoRef::Plane { plane } => plane_sel(s, plane),
        solidcraft_engine::doc::construct::GeoRef::Face { at, .. } => face_sel(s, *at),
        _ => None,
    }
}

pub(crate) fn plane_sel(s: &Session, pl: &PlaneRef) -> Option<Sel> {
    match pl {
        PlaneRef::Origin { name } => Some(Sel::Plane { name: name.clone() }),
        PlaneRef::Construction { name } => Some(Sel::Plane { name: name.to_string() }),
        PlaneRef::Custom { plane } | PlaneRef::Face { plane, .. } => face_sel(s, plane.origin),
        PlaneRef::Flipped { base, .. } => plane_sel(s, base),
        _ => None,
    }
}

pub(crate) fn op_index(o: &Operation) -> usize {
    match o {
        Operation::NewBody => 0,
        Operation::Join => 1,
        Operation::Cut => 2,
        Operation::Intersect => 3,
    }
}

/// The origin axis a unit direction runs along (either way), if any.
fn world_axis(d: Vec3) -> Option<String> {
    [("X", Vec3::X), ("Y", Vec3::Y), ("Z", Vec3::Z)]
        .into_iter()
        .find(|(_, a)| d.cross(*a).len() < 1e-9 && d.dot(*a) > 0.0)
        .map(|(n, _)| n.to_string())
}

fn pt3(v: Vec3) -> Value {
    json!([v.x, v.y, v.z])
}

/// A point of expressions: numbers as numbers, expressions as strings.
fn pt_expr(p: &solidcraft_engine::doc::PointExpr) -> Value {
    Value::Array(p.iter().map(|e| e.parse::<f64>().ok().filter(|x| x.is_finite()).map_or_else(|| json!(e), |x| json!(x))).collect())
}

/// A dialog that edits an existing feature, filled from it. The timeline is rolled back to just
/// before the feature, so its references show on the geometry they refer to.
pub fn for_feature(app: &SolveApp, id: u64, marker: Option<usize>) -> Option<Dialog> {
    let _refill = Refill::start();
    let s = &app.session;
    let f = s.doc.feature(id)?.clone();
    let st = s.world_state();
    let start = |cmd: &str| Dialog::for_command(app, cmd);
    let mut d = match &f.kind {
        FeatureKind::Extrude { sketch, profiles, extent, operation, targets, thin } => {
            let mut d = start("solid.extrude")?;
            d.kind = Kind::Extrude {
                distance: extent.distance.to_string(),
                direction: match (extent.direction, &extent.distance2) {
                    (_, Some(_)) => TWO_SIDES,
                    (Direction::Positive, None) => 0,
                    (Direction::Negative, None) => 1,
                    (Direction::Symmetric, None) => 2,
                },
                operation: op_index(operation),
                auto_op: false,
                smart: crate::smart_extrude::Smart { manual_dir: true, ..Default::default() },
                distance2: extent.distance2.as_deref().map(str::to_string).unwrap_or_else(|| "10 mm".into()),
                taper: extent.taper.as_deref().map(str::to_string).unwrap_or_else(|| "0 deg".into()),
                taper_present: extent.taper.is_some(),
                to_offset: extent.to_offset.as_deref().map(str::to_string).unwrap_or_default(),
                start: extent.start_offset.as_deref().map(str::to_string).unwrap_or_default(),
                all: extent.through_all,
                thin: thin.is_some(),
                wall: thin.as_ref().map_or_else(|| "2 mm".into(), |t| t.thickness.to_string()),
                wall_side: thin.as_ref().map_or(0, |t| WALL_SIDES.iter().position(|w| ThinSide::parse(w) == Some(t.side)).unwrap_or(0)),
                wall2: thin.as_ref().and_then(|t| t.thickness2.as_deref().map(str::to_string)).unwrap_or_else(|| "1 mm".into()),
            };
            d.extra.insert("convention".into(), json!(extent.convention));
            // Extent references belong to the extrude's authoring frame, including free
            // points. They are not body-local picks (a reference may be on another body).
            let frame = feature_frame(s, f.component);
            for (label, point) in [(TO_OBJECT, extent.to), (FROM_OBJECT, extent.from)] {
                if let Some(point) = point {
                    set_object(&mut d.inputs, &mut d.active, label, true);
                    if let Some(input) = d.inputs.iter_mut().find(|i| i.label == label) {
                        input.items = vec![Sel::Vertex { body: String::new(), point: solidcraft_engine::doc::apply_point(&frame, point.0) }];
                    }
                }
            }
            if let Some(thin) = thin {
                set_thin(&mut d.inputs, true);
                if let Some(inp) = d.inputs.first_mut() {
                    inp.items = thin.curves.iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
                }
            } else {
                let items = st.sketch(sketch.0).map(|ss| profile_indices(ss, profiles)).unwrap_or_default();
                if let Some(inp) = d.inputs.first_mut() {
                    inp.items = items.into_iter().map(|index| Sel::Profile { sketch: sketch.0, index }).collect();
                }
            }
            if !targets.is_empty() {
                d.extra.insert("targets".into(), json!(targets));
            }
            d
        }
        FeatureKind::Revolve { sketch, profiles, axis, angle, operation, targets, angle2, symmetric, convention, to } => {
            let mut d = start("solid.revolve")?;
            let frame = feature_frame(s, f.component);
            let stored_axis = match axis {
                AxisRef::Line { origin, dir } => Some(json!({
                    "origin": pt(solidcraft_engine::doc::apply_point(&frame, origin.0)),
                    "dir": pt(solidcraft_engine::doc::apply_vector(&frame, dir.0))
                })),
                _ => None,
            };
            d.kind = Kind::Revolve { angle: angle.to_string(), operation: op_index(operation), axis: stored_axis };
            d.extra.insert("convention".into(), json!(convention));
            if let Some(a2) = angle2 {
                d.extra.insert("angle2".into(), json!(a2));
            }
            if *symmetric {
                d.extra.insert("direction".into(), json!("symmetric"));
            }
            if let Some(point) = to {
                set_to_object(&mut d.inputs, &mut d.active, true);
                if let Some(input) = d.inputs.iter_mut().find(|i| i.label == TO_OBJECT) {
                    input.items = vec![Sel::Vertex { body: String::new(), point: solidcraft_engine::doc::apply_point(&frame, point.0) }];
                }
            }
            let items = st.sketch(sketch.0).map(|ss| profile_indices(ss, profiles)).unwrap_or_default();
            if let Some(inp) = d.inputs.get_mut(0) {
                inp.items = items.into_iter().map(|index| Sel::Profile { sketch: sketch.0, index }).collect();
            }
            let ax = match axis {
                AxisRef::World { axis } => Some(Sel::Axis { name: axis.to_ascii_uppercase() }),
                AxisRef::SketchAxis { axis } => Some(Sel::Axis { name: axis.to_ascii_lowercase() }),
                AxisRef::SketchLine { curve } => Some(Sel::SketchCurve { id: curve.clone() }),
                _ => None,
            };
            if let Some(inp) = d.inputs.get_mut(1) {
                inp.items = ax.into_iter().collect();
            }
            if !targets.is_empty() {
                d.extra.insert("targets".into(), json!(targets));
            }
            d
        }
        FeatureKind::Fillet { edges, radius, .. } | FeatureKind::Chamfer { edges, distance: radius, .. } => {
            let chamfer = matches!(f.kind, FeatureKind::Chamfer { .. });
            let mut d = start(if chamfer { "solid.chamfer" } else { "solid.fillet" })?;
            d.kind = fillet_kind(radius, chamfer);
            if let Kind::Fillet { chain, ctype, distance2, angle, flip, .. } = &mut d.kind {
                *chain = false;
                // An unequal chamfer: two distances, or a distance and an angle.
                if let FeatureKind::Chamfer { distance2: d2, angle: a, flip: f, .. } = &f.kind {
                    if let Some(v) = d2 {
                        *ctype = 1;
                        *distance2 = v.to_string();
                    } else if let Some(v) = a {
                        *ctype = 2;
                        *angle = v.to_string();
                    }
                    *flip = *f;
                }
            }
            // A fillet that reduces or tapers a radius too large for its edges.
            if let FeatureKind::Fillet { fit, .. } = &f.kind {
                use solidcraft_engine::doc::FilletFit;
                let mode = match fit {
                    FilletFit::Exact => None,
                    FilletFit::Reduce => Some("reduce"),
                    FilletFit::Taper => Some("taper"),
                };
                if let Some(mode) = mode {
                    d.extra.insert("fit".into(), json!(mode));
                }
            }
            // A chord-length or variable fillet.
            if let FeatureKind::Fillet { style, .. } = &f.kind {
                match style {
                    FilletStyle::Constant => {}
                    FilletStyle::Curvature => {
                        d.extra.insert("type".into(), json!("curvature"));
                    }
                    FilletStyle::Chord => {
                        d.extra.insert("type".into(), json!("chord"));
                    }
                    FilletStyle::Variable { start, .. } | FilletStyle::Spline { start, .. } => {
                        match style {
                            FilletStyle::Variable { radius2, mid, smooth, .. } => {
                                d.extra.insert("type".into(), json!("variable"));
                                d.extra.insert("radius2".into(), json!(radius2));
                                d.extra.insert("mid".into(), json!(mid.iter().map(|(t, r)| json!([t, r])).collect::<Vec<_>>()));
                                d.extra.insert("smooth".into(), json!(smooth));
                            }
                            FilletStyle::Spline { knots, radii, .. } => {
                                d.extra.insert("type".into(), json!("spline"));
                                d.extra.insert("knots".into(), json!(knots));
                                d.extra.insert("radii".into(), json!(radii));
                            }
                            FilletStyle::Constant | FilletStyle::Chord | FilletStyle::Curvature => {}
                        }
                        // The start was the first edge's last point: ends swapped.
                        let first_end = edges.first().and_then(|p| edge_sel(s, p.0)).and_then(|x| match x {
                            Sel::Edge { body, index, .. } => {
                                s.world_state().body(&body).and_then(|b| b.mesh().edges.get(index).and_then(|e| e.last().copied()))
                            }
                            _ => None,
                        });
                        let swapped = first_end.is_some_and(|e| e.dist(start.0) < 1e-6);
                        d.extra.insert("swap_ends".into(), json!(swapped));
                    }
                }
            }
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = edges.iter().filter_map(|p| edge_sel(s, p.0)).collect();
            }
            d
        }
        FeatureKind::Shell { faces, thickness, direction, tangent_chain, .. } => {
            let mut d = start("solid.shell")?;
            d.kind = Kind::Shell { thickness: thickness.to_string() };
            d.extra.insert("tangent_chain".into(), json!(tangent_chain));
            if !direction.is_empty() && direction != "inside" {
                d.extra.insert("direction".into(), json!(direction));
            }
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = faces.iter().filter_map(|p| face_sel(s, p.0)).collect();
            }
            d
        }
        FeatureKind::Draft { faces, angle, neutral, pull, .. } => {
            let mut d = start("solid.draft")?;
            d.kind = Kind::Draft { angle: angle.to_string() };
            if let Some(inp) = d.inputs.get_mut(0) {
                inp.items = faces.iter().filter_map(|p| face_sel(s, p.0)).collect();
            }
            if let Some(inp) = d.inputs.get_mut(1) {
                inp.items = plane_sel(s, neutral).into_iter().collect();
            }
            d.extra.insert("pull".into(), pt3(pull.0));
            d
        }
        FeatureKind::Hole { position, at, direction, diameter, depth, hole, points, thread, to } => {
            let mut d = start("solid.hole")?;
            let mut k = hole_defaults();
            if let Kind::Hole { diameter: dia, depth: dep, kind, cb_diameter, cb_depth, cs_diameter, cs_angle, opts } = &mut k {
                *dia = diameter.to_string();
                opts.all = depth.is_none();
                opts.angled = matches!(hole, HoleKind::Drilled { .. });
                opts.multiple = points.is_some();
                if let Some(th) = thread {
                    opts.tapped = true;
                    opts.thread = th.clone();
                }
                if let Some(dd) = depth {
                    *dep = dd.to_string();
                }
                match hole {
                    HoleKind::Counterbore { cb_diameter: a, cb_depth: b } => {
                        *kind = 1;
                        *cb_diameter = a.to_string();
                        *cb_depth = b.to_string();
                    }
                    HoleKind::Countersink { cs_diameter: a, cs_angle: b } => {
                        *kind = 2;
                        *cs_diameter = a.to_string();
                        *cs_angle = b.to_string();
                    }
                    HoleKind::Drilled { tip_angle } => {
                        opts.tip_angle = tip_angle.to_string();
                    }
                    HoleKind::Simple => {}
                }
            }
            d.kind = k;
            match points {
                Some(sp) => {
                    d.inputs = vec![SelInput::new("Points", selection::POINTS, true)];
                    if let Some(inp) = d.inputs.first_mut() {
                        inp.items = sp.ids.iter().map(|id| Sel::SketchPoint { id: format!("{}:{id}", sp.sketch) }).collect();
                    }
                    if sp.flip {
                        d.extra.insert("flip".into(), json!(true));
                    }
                }
                None => {
                    if let Some(inp) = d.inputs.first_mut() {
                        inp.items = vec![face_sel(s, position.0).unwrap_or(Sel::Face { body: String::new(), index: 0, point: position.0 })];
                    }
                    d.extra.insert("direction".into(), pt3(direction.0));
                    // A position given as expressions (#1678) is sent again while the pick
                    // stays where the hole is; a new pick places it by the point picked.
                    if let Some(at) = at {
                        let exprs: Vec<&str> = at.iter().map(|e| e.as_str()).collect();
                        d.extra.insert("position_exprs".into(), json!({"at": exprs, "point": pt3(position.0)}));
                    }
                }
            }
            if to.is_some() {
                if let Kind::Hole { opts, .. } = &mut d.kind {
                    opts.all = false;
                }
                with_to_object(&mut d, s, to.map(Vec3::from));
            }
            // A hole at every point of a sketch.
            if points.as_ref().is_some_and(|sp| sp.ids.is_empty()) {
                d.extra.insert("all_points".into(), json!(true));
            }
            d
        }
        FeatureKind::Box { corner, length, width, height, operation } => {
            let mut d = start("solid.box")?;
            d.kind = Kind::Primitive {
                cmd: "solid.box",
                fields: vec![("length", stored_length(s, length)), ("width", stored_length(s, width)), ("height", stored_length(s, height))],
                operation: op_index(operation),
                at: None,
            };
            d.extra.insert("corner".into(), pt_expr(corner));
            d
        }
        FeatureKind::Cylinder { base, axis, radius, height, operation } => {
            let mut d = start("solid.cylinder")?;
            d.kind = Kind::Primitive {
                cmd: "solid.cylinder",
                fields: vec![("radius", stored_length(s, radius)), ("height", stored_length(s, height))],
                operation: op_index(operation),
                at: None,
            };
            d.extra.insert("base".into(), pt_expr(base));
            d.extra.insert("axis".into(), pt3(axis.0));
            d
        }
        FeatureKind::Sphere { center, radius, operation } => {
            let mut d = start("solid.sphere")?;
            d.kind =
                Kind::Primitive { cmd: "solid.sphere", fields: vec![("radius", stored_length(s, radius))], operation: op_index(operation), at: None };
            d.extra.insert("center".into(), pt_expr(center));
            d
        }
        FeatureKind::Torus { center, major, minor, operation } => {
            let mut d = start("solid.torus")?;
            d.kind = Kind::Primitive {
                cmd: "solid.torus",
                fields: vec![("major", stored_length(s, major)), ("minor", stored_length(s, minor))],
                operation: op_index(operation),
                at: None,
            };
            d.extra.insert("center".into(), pt_expr(center));
            d
        }
        FeatureKind::BoundaryFill { tools, cells, operation, remove_tools } => {
            let mut d = start("solid.boundary_fill")?;
            // The cells again, ticked where a stored cell point lies in the same cell.
            let before = s.model.state_before(f.id);
            let found = fill_cells(&before, &solidcraft_engine::doc::plain(tools));
            let filled: Vec<String> =
                cells.iter().filter_map(|c| found.iter().min_by(|a, b| a.0.dist(c.0).total_cmp(&b.0.dist(c.0))).map(|x| x.1.clone())).collect();
            let items: Vec<Sel> = tools.iter().map(|n| Sel::Body { name: n.to_string() }).collect();
            d.kind = Kind::BoundaryFill {
                operation: op_index(operation),
                cells: found.into_iter().map(|(p, l)| (p, l.clone(), filled.contains(&l))).collect(),
                of: items.clone(),
            };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = items;
            }
            if *remove_tools {
                d.extra.insert("remove_tools".into(), json!(true));
            }
            d
        }
        FeatureKind::Combine { target, tools, operation, keep_tools } => {
            let mut d = start("solid.combine")?;
            d.kind = Kind::Combine { operation: op_index(operation).max(1), keep_tools: *keep_tools };
            if let Some(inp) = d.inputs.get_mut(0) {
                inp.items = vec![Sel::Body { name: target.to_string() }];
            }
            if let Some(inp) = d.inputs.get_mut(1) {
                inp.items = tools.iter().map(|n| Sel::Body { name: n.to_string() }).collect();
            }
            d
        }
        FeatureKind::Mirror { features, plane, bodies, placements, .. } => {
            let mut d = start("solid.mirror")?;
            let ids: Vec<u64> = features.iter().filter_map(|n| s.doc.find_feature(n).map(|f| f.id)).collect();
            // Replay references stay features, including features that created a body.
            let as_features = !features.is_empty();
            if let Some(inp) = d.inputs.get_mut(0) {
                if !bodies.is_empty() {
                    // Each body as the placement it was mirrored at shows it (an instance by its world name).
                    let own = s.model.state();
                    let world = |k: usize, name: &str| match (placements.get(k), own.body(name)) {
                        (Some(path), Some(b)) => s.doc.world_body_name(&own, name, s.doc.model_body_component(b), path),
                        _ => name.to_string(),
                    };
                    inp.items = bodies.iter().enumerate().map(|(k, name)| Sel::Body { name: world(k, name) }).collect();
                } else if as_features {
                    inp.accept = selection::FEATURES;
                    inp.label = objects_label(inp.accept);
                    inp.items = ids.iter().map(|id| Sel::Feature { id: *id }).collect();
                } else {
                    inp.items = st.bodies.iter().filter(|b| ids.contains(&b.feature)).map(|b| Sel::Body { name: b.name.clone() }).collect();
                }
            }
            if let Some(inp) = d.inputs.get_mut(1) {
                inp.items = plane_sel(s, plane).into_iter().collect();
            }
            d
        }
        FeatureKind::Pattern { features, pattern, bodies: copied } => {
            let ids: Vec<u64> = features.iter().filter_map(|n| s.doc.find_feature(n).map(|f| f.id)).collect();
            let bodies: Vec<Sel> = if copied.is_empty() {
                st.bodies.iter().filter(|b| ids.contains(&b.feature)).map(|b| Sel::Body { name: b.name.clone() }).collect()
            } else {
                copied.iter().map(|name| Sel::Body { name: name.to_string() }).collect()
            };
            let mut d = match pattern {
                PatternKind::Rectangular { dir1, count1, spacing1, dir2, count2, spacing2, alignment, alignment2, reversed, reversed2 } => {
                    let mut d = start("solid.pattern.rectangular")?;
                    d.extra.insert("alignment".into(), json!(alignment));
                    d.extra.insert("alignment2".into(), json!(alignment2));
                    d.extra.insert("reversed".into(), json!(reversed));
                    d.extra.insert("reversed2".into(), json!(reversed2));
                    d.kind = Kind::PatternRect {
                        count: count1.to_string(),
                        spacing: spacing1.to_string(),
                        count2: count2.as_deref().map(str::to_string).unwrap_or_else(|| "1".into()),
                        spacing2: spacing2.as_deref().map(str::to_string).unwrap_or_else(|| "20 mm".into()),
                    };
                    // Directions along the origin axes show as those axes; others are kept as is.
                    for (i, (key, dir)) in [("dir1", Some(*dir1)), ("dir2", *dir2)].into_iter().enumerate() {
                        let Some(dir) = dir else { continue };
                        match world_axis(dir.0) {
                            Some(name) => {
                                if let Some(inp) = d.inputs.get_mut(i + 1) {
                                    inp.items = vec![Sel::Axis { name }];
                                }
                            }
                            None => {
                                d.extra.insert(key.into(), pt3(dir.0));
                            }
                        }
                    }
                    d
                }
                PatternKind::Circular { origin, axis, count, angle, alignment, reversed } => {
                    let mut d = start("solid.pattern.circular")?;
                    d.extra.insert("alignment".into(), json!(alignment));
                    d.extra.insert("reversed".into(), json!(reversed));
                    d.kind = Kind::PatternCirc { count: count.to_string(), angle: angle.to_string() };
                    match world_axis(axis.0).filter(|_| origin.len() < 1e-9) {
                        Some(name) => {
                            if let Some(inp) = d.inputs.get_mut(1) {
                                inp.items = vec![Sel::Axis { name }];
                            }
                        }
                        None => {
                            d.extra.insert("axis".into(), json!({"origin": pt3(origin.0), "dir": pt3(axis.0)}));
                        }
                    }
                    d
                }
                // No dialog for a path pattern yet: edit it with timeline.redefine.
                PatternKind::Path { .. } => return None,
            };
            // Replay references stay features, including features that created a body.
            let as_features = !features.is_empty();
            if let Some(inp) = d.inputs.first_mut() {
                if as_features {
                    inp.accept = selection::FEATURES;
                    inp.label = objects_label(inp.accept);
                    inp.items = ids.iter().map(|id| Sel::Feature { id: *id }).collect();
                } else {
                    inp.items = bodies;
                }
            }
            d
        }
        FeatureKind::Loft { sections, operation, targets } => {
            let mut d = start("solid.loft")?;
            d.kind = Kind::Loft { operation: op_index(operation) };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = sections
                    .iter()
                    .flat_map(|sec| {
                        let idx = st.sketch(sec.sketch.0).map(|ss| profile_indices(ss, &sec.profiles)).unwrap_or_default();
                        idx.into_iter().map(move |index| Sel::Profile { sketch: sec.sketch.0, index })
                    })
                    .collect();
            }
            if !targets.is_empty() {
                d.extra.insert("targets".into(), json!(targets));
            }
            d
        }
        FeatureKind::Sweep { sketch, profiles, path, operation, targets, orientation, twist, scale, rails, rail_fit, .. } => {
            let mut d = start("solid.sweep")?;
            let (opts, direction) = crate::dialogs_sweep::SweepOpts::of(orientation, twist, scale, *rail_fit, &feature_frame(s, f.component));
            d.kind = Kind::Sweep { operation: op_index(operation), opts };
            if let Some(inp) = d.inputs.get_mut(2) {
                inp.items = direction.into_iter().collect();
            }
            for (inp, items) in d.inputs.iter_mut().skip(3).zip(crate::dialogs_sweep::rail_sels(rails)) {
                inp.items = items;
            }
            let items = st.sketch(sketch.0).map(|ss| profile_indices(ss, profiles)).unwrap_or_default();
            if let Some(inp) = d.inputs.get_mut(0) {
                inp.items = items.into_iter().map(|index| Sel::Profile { sketch: sketch.0, index }).collect();
            }
            if let Some(inp) = d.inputs.get_mut(1) {
                inp.items = path.iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
            }
            if !targets.is_empty() {
                d.extra.insert("targets".into(), json!(targets));
            }
            d
        }
        FeatureKind::ConstructionPlane { plane: PlaneRef::Offset { base, distance } } => {
            let mut d = start("construct.plane.offset")?;
            d.kind = Kind::OffsetPlane { offset: distance.to_string() };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = plane_sel(s, base).into_iter().collect();
            }
            d
        }
        FeatureKind::ConstructionPlane { plane: PlaneRef::AlongPath { path, t } } => {
            let mut d = start("construct.plane.along_path")?;
            let (item, sketch) = match path.as_ref() {
                solidcraft_engine::doc::construct::GeoRef::SketchCurve { sketch, curve } => (Sel::SketchCurve { id: curve.clone() }, Some(*sketch)),
                solidcraft_engine::doc::construct::GeoRef::Edge { at, name } => {
                    // Preserve the saved named reference until a replacement is picked.
                    d.extra.insert("path".into(), json!({"type":"edge","at":pt3(*at),"name":name}));
                    (Sel::Edge { body: String::new(), index: 0, point: *at }, None)
                }
                _ => return None,
            };
            d.kind = Kind::PathPlane { t: t.to_string(), sketch };
            if let Some(input) = d.inputs.first_mut() {
                input.items = vec![item];
            }
            d
        }
        FeatureKind::ConstructionPlane { plane: PlaneRef::Midplane { a, b } } => {
            let mut d = start("construct.plane.midplane")?;
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = geo_sel(s, a).into_iter().collect();
            }
            if let Some(inp) = d.inputs.get_mut(1) {
                inp.items = geo_sel(s, b).into_iter().collect();
            }
            d
        }
        FeatureKind::ConstructionPlane { plane: PlaneRef::AtAngle { base, axis_origin, axis_dir, angle } } => {
            let mut d = start("construct.plane.at_angle")?;
            d.kind = Kind::AnglePlane { angle: angle.to_string() };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = plane_sel(s, base).into_iter().collect();
            }
            match world_axis(*axis_dir).filter(|_| axis_origin.len() < 1e-9) {
                Some(name) => {
                    if let Some(inp) = d.inputs.get_mut(1) {
                        inp.items = vec![Sel::Axis { name }];
                    }
                }
                None => {
                    d.extra.insert("axis".into(), json!({"origin": pt3(*axis_origin), "dir": pt3(*axis_dir)}));
                }
            }
            d
        }
        FeatureKind::Split { body, plane, tool, sketch, line } => {
            let mut d = start("solid.split_body")?;
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = vec![Sel::Body { name: body.to_string() }];
            }
            if let (Some(k), Some(id)) = (sketch, line) {
                d.extra.insert("tool_sketch".into(), json!(k.0));
                if let Some(inp) = d.inputs.get_mut(1) {
                    inp.items = vec![Sel::SketchCurve { id: id.clone() }];
                }
            } else if let Some(inp) = d.inputs.get_mut(1) {
                inp.items = match tool {
                    Some(t) => face_sel(s, t.point.0).into_iter().collect(),
                    None => plane_sel(s, plane).into_iter().collect(),
                };
            }
            d
        }
        FeatureKind::Scale { bodies, origin, factor, factors } => {
            let mut d = start("solid.scale")?;
            d.kind = Kind::Scale { factor: factor.to_string() };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = bodies.iter().map(|n| Sel::Body { name: n.to_string() }).collect();
            }
            d.extra.insert("origin".into(), pt3(origin.0));
            if let Some(f) = factors {
                d.extra.insert("factors".into(), json!(f));
            }
            d
        }
        FeatureKind::OffsetFace { faces, distance, body } => {
            let mut d = start("solid.offset_face")?;
            d.kind = Kind::OffsetFaces { distance: distance.to_string() };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = faces.iter().filter_map(|p| face_sel(s, p.0)).collect();
            }
            if let Some(b) = body {
                d.extra.insert("body".into(), json!(b));
            }
            d
        }
        FeatureKind::Thread { face, designation, length, modeled } => {
            let mut d = start("solid.thread")?;
            if *modeled {
                d.extra.insert("modeled".into(), json!(true));
            }
            d.kind = Kind::Thread {
                designation: designation.clone().unwrap_or_default(),
                length: length.as_deref().map(str::to_string).unwrap_or_default(),
                suggested: None,
                of: Vec::new(),
            };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = face_sel(s, face.0).into_iter().collect();
            }
            d.sync_thread(s);
            d
        }
        FeatureKind::BoundingSolid { bodies, margin } => {
            let mut d = start("solid.bounding_solid")?;
            d.kind = Kind::Stock { margin: margin.to_string() };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = bodies.iter().map(|n| Sel::Body { name: n.to_string() }).collect();
            }
            d
        }
        FeatureKind::Pipe { path, diameter, wall, operation, targets, .. } => {
            let mut d = start("solid.pipe")?;
            d.kind = Kind::Pipe { diameter: diameter.to_string(), wall: wall.as_deref().map(str::to_string).unwrap_or_default() };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = path.iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
            }
            d.extra.insert("operation".into(), json!(OPS.get(op_index(operation)).copied().unwrap_or("new")));
            if !targets.is_empty() {
                d.extra.insert("targets".into(), json!(targets));
            }
            d
        }
        FeatureKind::Emboss { sketch, profiles, depth, deboss, targets, face } => {
            let mut d = start("solid.emboss")?;
            d.kind = Kind::Emboss { depth: depth.to_string(), deboss: *deboss };
            let items = st.sketch(sketch.0).map(|ss| profile_indices(ss, profiles)).unwrap_or_default();
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = items.into_iter().map(|index| Sel::Profile { sketch: sketch.0, index }).collect();
            }
            if !targets.is_empty() {
                d.extra.insert("targets".into(), json!(targets));
            }
            if let Some(face) = face
                && let Some(input) = d.inputs.get_mut(1)
            {
                input.items = vec![emboss_face_sel(s, f.component, face, f.face_names.first()?)?];
            }
            d
        }
        FeatureKind::Rib { curves, thickness, depth, flip, web, normal, .. } => {
            let mut d = start(if *web { "solid.web" } else { "solid.rib" })?;
            d.kind = Kind::Rib {
                web: *web,
                thickness: thickness.to_string(),
                depth: depth.as_deref().map(str::to_string).unwrap_or_default(),
                flip: *flip,
                normal: *normal,
            };
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = curves.iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect();
            }
            d
        }
        FeatureKind::ReplaceFace { faces, target, body } => {
            let mut d = start("solid.replace_face")?;
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = faces.iter().filter_map(|p| face_sel(s, p.0)).collect();
            }
            if let Some(inp) = d.inputs.get_mut(1) {
                inp.items = plane_sel(s, target).into_iter().collect();
            }
            if let Some(b) = body {
                d.extra.insert("body".into(), json!(b));
            }
            d
        }
        FeatureKind::Remove { bodies } => {
            let mut d = start("solid.remove")?;
            if let Some(inp) = d.inputs.first_mut() {
                inp.items = bodies.iter().map(|n| Sel::Body { name: n.to_string() }).collect();
            }
            d
        }
        FeatureKind::Move { bodies, translate, rotate_axis, angle, copy } => {
            let mut d = start("solid.move")?;
            crate::dialogs_move::for_feature(
                &mut d,
                &solidcraft_engine::doc::plain(bodies),
                translate,
                rotate_axis.map(Vec3::from),
                angle.as_ref(),
                *copy,
            );
            d
        }
        other => {
            let (kind, inputs, extra) = crate::dialogs_sheet::for_feature(app, other)
                .or_else(|| crate::dialogs_plastic::for_feature(app, other))
                .or_else(|| crate::dialogs_surface::for_feature(app, other, &feature_frame(s, f.component)))?;
            let mut d = Dialog::new(kind, inputs);
            d.extra = extra;
            d
        }
    };
    d.editing = Some((id, marker));
    d.active = 0;
    d.error = None;
    Some(d)
}

#[cfg(test)]
mod param_refusal_tests {
    use super::*;
    use solidcraft_engine::Session;

    #[test]
    fn a_refused_parameter_change_shows_why() {
        // A triangle whose sides are parameters: a side longer than the other two together
        // has no solution, so the change is refused and the table shows the reason.
        let mut s = Session::default();
        s.execute("sketch.create", &json!({"plane": "XY", "name": "Tri"})).unwrap();
        s.execute("sketch.line", &json!({"points": [[0, 0], [40, 0], [20, 30]], "closed": true})).unwrap();
        for l in ["l1", "l2", "l3"] {
            s.execute("sketch.dimension", &json!({"entities": [l]})).unwrap();
        }
        s.execute("sketch.finish", &json!({})).unwrap();
        let name = s.doc.params.iter().find(|p| p.model).map(|p| p.name.clone()).unwrap();
        let before = s.doc.param(&name).map(|p| p.expr.clone());
        let mut app = SolveApp::new(s, Default::default());
        let why = change_param(&mut app, &name, "500 mm").unwrap_err();
        assert!(why.contains("Tri") && why.contains("conflict"), "{why}");
        assert_eq!(app.session.doc.param(&name).map(|p| p.expr.clone()), before);
        assert!(app.status.as_ref().is_some_and(|(m, _, error)| *error && m.contains("Tri")), "{:?}", app.status);
        assert!(change_param(&mut app, &name, "41 mm").is_ok());
    }
}

#[cfg(test)]
mod refused_ok_tests {
    //! A refused OK's error lasts only as long as the inputs it was about (#641).

    use super::*;
    use crate::scenario::Harness;

    fn steps(h: &mut Harness, steps: &[Value]) {
        for s in steps {
            h.step(s).unwrap();
        }
    }

    /// A box, and a hole dialog with a position picked on its top.
    fn hole_dialog() -> Harness {
        let mut h = Harness::new();
        steps(
            &mut h,
            &[
                json!({"call": "engine.execute", "params": {"command": "solid.box", "params": {"length": 50, "width": 30, "height": 20}}}),
                json!({"call": "ui.view", "params": {"view": "iso"}}),
                json!({"start": "solid.hole"}),
                json!({"click": {"world": [25, 15, 20]}}),
            ],
        );
        h
    }

    fn set_depth(h: &mut Harness, depth: &str) {
        steps(h, &[json!({"click": {"handle": "dialog:Depth"}}), json!({"key": "A", "cmd": true}), json!({"text": depth})]);
    }

    /// What the dialog shows in red once the live preview has finished: its own error, else the
    /// preview's.
    fn shown(h: &mut Harness) -> Option<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while h.app.preview.busy {
            assert!(std::time::Instant::now() < deadline, "the preview did not finish");
            h.frames(1);
        }
        h.app.dialog.as_ref().and_then(|d| d.error.clone()).or_else(|| h.app.preview.error.clone())
    }

    fn refused(h: &mut Harness) {
        set_depth(h, "-5");
        steps(h, &[json!({"key": "Enter"})]);
        assert_eq!(h.app.dialog.as_ref().and_then(|d| d.error.as_deref()), Some("hole depth must be positive"));
    }

    #[test]
    fn the_error_stays_while_the_inputs_do() {
        let mut h = hole_dialog();
        refused(&mut h);
        h.frames(10);
        assert_eq!(shown(&mut h).as_deref(), Some("hole depth must be positive"));
    }

    #[test]
    fn a_value_fixed_in_the_dialog_clears_it() {
        let mut h = hole_dialog();
        refused(&mut h);
        set_depth(&mut h, "5");
        h.frames(2);
        assert_eq!(shown(&mut h), None);
    }

    /// The canvas value box and the arrows change the dialog before `show` runs in a frame.
    #[test]
    fn a_value_fixed_outside_the_dialog_clears_it() {
        let mut h = hole_dialog();
        refused(&mut h);
        if let Some(Kind::Hole { depth, .. }) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            *depth = "5 mm".into();
        }
        h.frames(2);
        assert_eq!(shown(&mut h), None);
    }

    #[test]
    fn a_refusal_for_a_missing_pick_clears_once_it_is_picked() {
        let mut h = Harness::new();
        steps(
            &mut h,
            &[
                json!({"call": "engine.execute", "params": {"command": "solid.box", "params": {"length": 50, "width": 30, "height": 20}}}),
                json!({"call": "ui.view", "params": {"view": "iso"}}),
                json!({"start": "solid.hole"}),
                json!({"key": "Enter"}),
            ],
        );
        let error = h.app.dialog.as_ref().and_then(|d| d.error.clone());
        assert!(error.as_deref().is_some_and(|e| e.contains("first")), "{error:?}");
        steps(&mut h, &[json!({"click": {"world": [25, 15, 20]}})]);
        h.frames(2);
        assert_eq!(shown(&mut h), None);
    }

    /// A parameter the depth names changes the design, not the dialog's commands.
    #[test]
    fn a_parameter_the_inputs_use_clears_it() {
        let mut h = hole_dialog();
        let add = json!({"command": "parameters.add", "params": {"name": "dep", "expression": "-5 mm"}});
        steps(&mut h, &[json!({"call": "engine.execute", "params": add})]);
        set_depth(&mut h, "dep");
        steps(&mut h, &[json!({"key": "Enter"})]);
        assert!(h.app.dialog.as_ref().and_then(|d| d.error.as_deref()).is_some_and(|e| e.contains("depth")));
        let change = json!({"command": "parameters.change", "params": {"name": "dep", "expression": "5 mm"}});
        steps(&mut h, &[json!({"call": "engine.execute", "params": change})]);
        h.frames(2);
        assert_eq!(shown(&mut h), None);
    }

    /// Picks changed some other way than a click (a box selection, a cleared chip) are seen too,
    /// and the dialog then says what the inputs lack now.
    #[test]
    fn a_pick_changed_outside_the_dialog_shows_what_is_missing_now() {
        let mut h = hole_dialog();
        refused(&mut h);
        if let Some(input) = h.app.dialog.as_mut().and_then(|d| d.inputs.first_mut()) {
            input.items.clear();
        }
        h.frames(2);
        let error = h.app.dialog.as_ref().and_then(|d| d.error.clone());
        assert!(error.as_deref().is_some_and(|e| e.contains("first")), "{error:?}");
    }

    /// Editing an existing hole: the refusal clears, and OK then edits that hole.
    #[test]
    fn editing_a_hole_clears_it_and_keeps_the_feature() {
        let mut h = Harness::new();
        let hole = json!({"command": "solid.hole", "params": {"position": [25, 15, 20], "diameter": "5 mm", "depth": "10 mm", "name": "Bore"}});
        steps(
            &mut h,
            &[
                json!({"call": "engine.execute", "params": {"command": "solid.box", "params": {"length": 50, "width": 30, "height": 20}}}),
                json!({"call": "engine.execute", "params": hole}),
                json!({"call": "ui.view", "params": {"view": "iso"}}),
                json!({"call": "ui.editFeature", "params": {"feature": "Bore"}}),
            ],
        );
        let features = h.app.session.doc.features.len();
        refused(&mut h);
        assert!(h.app.dialog.as_ref().is_some_and(|d| d.editing.is_some()));
        set_depth(&mut h, "5");
        h.frames(2);
        assert_eq!(shown(&mut h), None);
        steps(&mut h, &[json!({"key": "Enter"})]);
        assert!(h.app.dialog.is_none());
        assert_eq!(h.app.session.doc.features.len(), features);
        let bore = h.app.session.execute("timeline.inspect", &json!({"feature": "Bore"})).unwrap();
        assert!(bore.to_string().contains("\"depth\":\"5\""), "{bore}");
    }
}

/// Whether two picks are the same face of the same body (wherever on it they were made).
fn same_face(a: &Sel, b: &Sel) -> bool {
    matches!((a, b), (Sel::Face { body: ba, index: ia, .. }, Sel::Face { body: bb, index: ib, .. }) if ba == bb && ia == ib)
}

/// Where a face was picked.
fn face_point(x: &Sel) -> Option<Value> {
    if let Sel::Face { point, .. } = x { Some(pt(*point)) } else { None }
}
