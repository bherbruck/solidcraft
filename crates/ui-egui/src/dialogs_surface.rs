//! Dialogs for the surface tools and face splitting (#19): Surface Extrude, Revolve, Sweep and
//! Loft, Offset and Ruled Surface, Patch, Stitch, Thicken, Extend, Trim, Untrim and Split Face. Each has a live preview and Edit Feature.

use serde_json::{Map, Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::doc::expr::Kind as ValueKind;
use solidcraft_engine::doc::{AxisRef, Direction, FaceAt, FeatureKind, Mat, PlaneRef, ProfileSel, RuledCorners, RuledMode};
use solidcraft_engine::geom::Vec3;

use crate::SolveApp;
use crate::dialogs::{Kind, OP_LABELS, OPS, combo, field, op_index, row_label};
use crate::selection::{AXES, BODIES, CURVES, EDGES, FACES, PLANES, PROFILES, SelInput, VERTICES};

/// A surface tool dialog's values.
#[derive(Clone, Debug, PartialEq)]
pub enum Surf {
    /// `direction`: an index into [`EXTRUDE_SIDES`].
    Extrude {
        distance: String,
        direction: usize,
    },
    /// `axis`: a stored {origin, dir} axis, used while no axis is picked.
    Revolve {
        angle: String,
        symmetric: bool,
        axis: Option<Value>,
    },
    Sweep(crate::dialogs_sweep::SweepOpts),
    Loft,
    /// `continuity`: an index into [`PATCH_CONTINUITY`] (how a patch on edges meets the faces
    /// beside its loop).
    Patch {
        continuity: usize,
    },
    Unstitch,
    ReverseNormal,
    Offset {
        distance: String,
        chain: bool,
    },
    /// `mode`: an index into [`RULED_MODES`].
    Ruled {
        distance: String,
        mode: usize,
        angle: String,
        flip: bool,
        round: bool,
    },
    Stitch {
        tolerance: String,
    },
    Thicken {
        thickness: String,
        symmetric: bool,
        operation: usize,
    },
    Extend {
        distance: String,
    },
    Trim,
    /// `holes`: an index into [`UNTRIM_HOLES`].
    Untrim {
        holes: usize,
        outer: bool,
        in_place: bool,
    },
    SplitFace,
}

impl Surf {
    pub fn title(&self) -> &'static str {
        match self {
            Surf::Extrude { .. } => "SURFACE EXTRUDE",
            Surf::Revolve { .. } => "SURFACE REVOLVE",
            Surf::Sweep(_) => "SURFACE SWEEP",
            Surf::Loft => "SURFACE LOFT",
            Surf::Patch { .. } => "PATCH",
            Surf::Unstitch => "UNSTITCH",
            Surf::ReverseNormal => "REVERSE NORMAL",
            Surf::Offset { .. } => "OFFSET SURFACE",
            Surf::Ruled { .. } => "RULED SURFACE",
            Surf::Stitch { .. } => "STITCH",
            Surf::Thicken { .. } => "THICKEN",
            Surf::Extend { .. } => "EXTEND",
            Surf::Trim => "TRIM",
            Surf::Untrim { .. } => "UNTRIM",
            Surf::SplitFace => "SPLIT FACE",
        }
    }

    /// The value on the canvas.
    pub fn primary(&mut self) -> Option<(&'static str, ValueKind, &mut String)> {
        Some(match self {
            Surf::Thicken { thickness, .. } => ("Thickness", ValueKind::Length, thickness),
            Surf::Extend { distance } | Surf::Extrude { distance, .. } | Surf::Offset { distance, .. } | Surf::Ruled { distance, .. } => {
                ("Distance", ValueKind::Length, distance)
            }
            Surf::Revolve { angle, .. } => ("Angle", ValueKind::Angle, angle),
            _ => return None,
        })
    }
}

/// How a patch on edges meets the faces beside its loop (labels, and the `continuity` parameter).
const PATCH_CONTINUITY: [(&str, &str); 3] = [("Position (G0)", "g0"), ("Tangent (G1)", "g1"), ("Curvature (G2)", "g2")];

/// Which holes Untrim takes away: every one, the picked ones, or none (the outer boundary only).
const UNTRIM_HOLES: [&str; 3] = ["All", "Picked", "None"];

/// The sides a surface extrude goes to (labels, and the `direction` parameter).
const EXTRUDE_SIDES: [(&str, &str); 3] = [("One Side", "positive"), ("Symmetric", "symmetric"), ("Flipped", "negative")];

/// The ways a ruled surface leaves its edges (labels, and the `mode` parameter).
const RULED_MODES: [(&str, &str); 4] = [("Normal", "normal"), ("Tangent", "tangent"), ("Angle", "angle"), ("Direction", "direction")];

fn inputs(k: &Surf) -> Vec<SelInput> {
    match k {
        Surf::Extrude { .. } => vec![SelInput::new("Profiles or curves", PROFILES | CURVES, true)],
        Surf::Revolve { .. } => vec![SelInput::new("Profiles or curves", PROFILES | CURVES, true), SelInput::new("Axis", AXES, false)],
        Surf::Sweep(_) => {
            let [rail, rail2] = crate::dialogs_sweep::rail_inputs();
            vec![
                SelInput::new("Profiles or curves", PROFILES | CURVES, true),
                SelInput::new("Path", CURVES, true),
                crate::dialogs_sweep::direction_input(),
                rail,
                rail2,
            ]
        }
        Surf::Loft => vec![SelInput::new("Sections (one sketch each)", PROFILES | CURVES, true)],
        Surf::Patch { .. } => vec![SelInput::new("Boundary", PROFILES | EDGES, true), SelInput::new("Pass through", VERTICES | FACES, true)],
        Surf::Unstitch => vec![SelInput::new("Body, or faces to take out", BODIES | FACES, true)],
        Surf::ReverseNormal => vec![SelInput::new("Surface, or faces to turn over", BODIES | FACES, true)],
        Surf::Offset { .. } => vec![SelInput::new("Faces", FACES, true)],
        Surf::Ruled { .. } => {
            vec![SelInput::new("Edges", EDGES, true), SelInput::new("Reference face", FACES, false), SelInput::new("Direction", AXES | EDGES, false)]
        }
        Surf::Stitch { .. } => vec![SelInput::new("Surfaces", BODIES, true)],
        Surf::Thicken { .. } => vec![SelInput::new("Surfaces", BODIES, true)],
        Surf::Extend { .. } => vec![SelInput::new("Edges", EDGES, true)],
        Surf::Trim => vec![
            SelInput::new("Surface", BODIES, false),
            SelInput::new("Trimming tool", PLANES | FACES, false),
            SelInput::new("Side to keep", FACES, false),
        ],
        Surf::Untrim { .. } => vec![SelInput::new("Face", FACES, false), SelInput::new("Holes (edges on them)", EDGES, true)],
        Surf::SplitFace => vec![SelInput::new("Faces to split", FACES, true), SelInput::new("Splitting tool", PLANES | FACES, false)],
    }
}

/// The dialog for a surface command.
pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let k = match id {
        "surface.extrude" => Surf::Extrude { distance: "10 mm".into(), direction: 0 },
        "surface.revolve" => Surf::Revolve { angle: "360 deg".into(), symmetric: false, axis: None },
        "surface.sweep" => Surf::Sweep(crate::dialogs_sweep::SweepOpts::default()),
        "surface.loft" => Surf::Loft,
        "surface.patch" => Surf::Patch { continuity: 0 },
        "surface.unstitch" => Surf::Unstitch,
        "surface.reverse_normal" => Surf::ReverseNormal,
        "surface.offset" => Surf::Offset { distance: "2 mm".into(), chain: false },
        "surface.ruled" => Surf::Ruled { distance: "10 mm".into(), mode: 0, angle: "5 deg".into(), flip: false, round: false },
        "surface.stitch" => Surf::Stitch { tolerance: String::new() },
        "surface.thicken" => Surf::Thicken { thickness: "2 mm".into(), symmetric: false, operation: 0 },
        "surface.extend" => Surf::Extend { distance: "10 mm".into() },
        "surface.trim" => Surf::Trim,
        "surface.untrim" => Surf::Untrim { holes: 0, outer: false, in_place: false },
        "solid.split_face" => Surf::SplitFace,
        _ => return None,
    };
    let i = inputs(&k);
    Some((Kind::Surface(k), i))
}

/// The rows after the inputs; true when Enter was pressed in a field.
pub fn rows(ui: &mut egui::Ui, k: &mut Surf) -> bool {
    let mut enter = false;
    match k {
        Surf::Stitch { tolerance } => {
            row_label(ui, "Tolerance");
            let r = ui.add(egui::TextEdit::singleline(tolerance).hint_text("tight"));
            crate::params_dialog::complete(ui, &r, tolerance);
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.end_row();
        }
        Surf::Thicken { thickness, symmetric, operation } => {
            row_label(ui, "Thickness");
            enter |= field(ui, thickness);
            ui.end_row();
            row_label(ui, "Direction");
            let mut dir = usize::from(*symmetric);
            combo(ui, "th_dir", &["One Side", "Symmetric"], &mut dir);
            *symmetric = dir == 1;
            ui.end_row();
            row_label(ui, "Operation");
            combo(ui, "th_op", &OP_LABELS, operation);
            ui.end_row();
        }
        Surf::Extend { distance } => {
            row_label(ui, "Distance");
            enter |= field(ui, distance);
            ui.end_row();
        }
        Surf::Revolve { angle, symmetric, .. } => {
            row_label(ui, "Angle");
            enter |= field(ui, angle);
            ui.end_row();
            row_label(ui, "Direction");
            let mut dir = usize::from(*symmetric);
            combo(ui, "sr_dir", &["One Side", "Symmetric"], &mut dir);
            *symmetric = dir == 1;
            ui.end_row();
        }
        Surf::Offset { distance, chain } => {
            row_label(ui, "Distance");
            enter |= field(ui, distance);
            ui.end_row();
            row_label(ui, "Tangent chain");
            ui.checkbox(chain, "");
            ui.end_row();
        }
        Surf::Ruled { distance, mode, angle, flip, round } => {
            row_label(ui, "Distance");
            enter |= field(ui, distance);
            ui.end_row();
            row_label(ui, "Type");
            combo(ui, "ru_mode", &RULED_MODES.map(|(label, _)| label), mode);
            ui.end_row();
            if RULED_MODES.get(*mode).is_some_and(|(_, m)| *m == "angle") {
                row_label(ui, "Angle");
                enter |= field(ui, angle);
                ui.end_row();
            }
            row_label(ui, "Corners");
            let mut corner = usize::from(*round);
            combo(ui, "ru_corners", &["Mitre", "Round"], &mut corner);
            *round = corner == 1;
            ui.end_row();
            row_label(ui, "Flip");
            ui.checkbox(flip, "");
            ui.end_row();
        }
        Surf::Extrude { distance, direction } => {
            row_label(ui, "Distance");
            enter |= field(ui, distance);
            ui.end_row();
            row_label(ui, "Direction");
            combo(ui, "sx_dir", &EXTRUDE_SIDES.map(|(label, _)| label), direction);
            ui.end_row();
        }
        Surf::Untrim { holes, outer, in_place } => {
            row_label(ui, "Holes");
            combo(ui, "ut_holes", &UNTRIM_HOLES, holes);
            ui.end_row();
            row_label(ui, "Restore outer boundary");
            ui.checkbox(outer, "");
            ui.end_row();
            row_label(ui, "In place (surfaces)");
            ui.checkbox(in_place, "");
            ui.end_row();
        }
        Surf::Patch { continuity } => {
            row_label(ui, "Continuity");
            combo(ui, "pa_cont", &PATCH_CONTINUITY.map(|(label, _)| label), continuity);
            ui.end_row();
        }
        Surf::Sweep(opts) => enter |= crate::dialogs_sweep::rows(ui, opts, "ss"),
        Surf::Trim | Surf::SplitFace | Surf::Unstitch | Surf::ReverseNormal | Surf::Loft => {}
    }
    enter
}

fn items(inputs: &[SelInput], i: usize) -> &[Sel] {
    inputs.get(i).map(|x| x.items.as_slice()).unwrap_or(&[])
}

fn pt(v: Vec3) -> Value {
    json!([v.x, v.y, v.z])
}

fn bodies(inputs: &[SelInput], i: usize) -> Vec<String> {
    items(inputs, i).iter().filter_map(|x| if let Sel::Body { name } = x { Some(name.clone()) } else { None }).collect()
}

/// One body and points on the picked faces of it (none: a whole body picked).
fn body_and_faces(picks: &[Sel]) -> Result<(String, Vec<Value>), String> {
    let body_of = |x: &Sel| match x {
        Sel::Body { name } | Sel::Face { body: name, .. } => Some(name.clone()),
        _ => None,
    };
    let body = picks.iter().find_map(body_of).ok_or("pick a body or some of its faces")?;
    if picks.iter().any(|x| body_of(x).as_ref() != Some(&body)) {
        return Err("pick faces of one body".into());
    }
    let faces = picks.iter().filter_map(|x| if let Sel::Face { point, .. } = x { Some(pt(*point)) } else { None }).collect();
    Ok((body, faces))
}

/// The sketch, profiles and curves of a surface builder's picks (all in one sketch).
fn sheet_picks(app: &SolveApp, picks: &[Sel]) -> Result<Value, String> {
    let profiles: Vec<(u64, usize)> =
        picks.iter().filter_map(|x| if let Sel::Profile { sketch, index } = x { Some((*sketch, *index)) } else { None }).collect();
    let curves: Vec<String> = picks.iter().filter_map(|x| if let Sel::SketchCurve { id } = x { Some(id.clone()) } else { None }).collect();
    let sketch = match profiles.first() {
        Some((sk, _)) => *sk,
        None => crate::dialogs::curves_sketch(app, &curves, None).ok_or("the curves must be in one sketch")?,
    };
    if profiles.iter().any(|(sk, _)| *sk != sketch) {
        return Err("the profiles and curves must be in one sketch".into());
    }
    let mut p = json!({ "sketch": sketch });
    if !curves.is_empty() {
        p["curves"] = json!(curves);
    }
    if !profiles.is_empty() {
        p["profiles"] = json!(profiles.iter().map(|(_, i)| *i).collect::<Vec<_>>());
    }
    Ok(p)
}

/// A surface builder's stored picks as selections again: its profiles, then its curves.
fn sheet_sels(app: &SolveApp, sketch: u64, profiles: Option<&ProfileSel>, curves: &[String]) -> Vec<Sel> {
    let st = app.session.world_state();
    let mut sels: Vec<Sel> = match (profiles, st.sketch(sketch)) {
        (Some(sel), Some(ss)) => crate::dialogs::profile_indices(ss, sel).into_iter().map(|index| Sel::Profile { sketch, index }).collect(),
        _ => Vec::new(),
    };
    sels.extend(curves.iter().map(|id| Sel::SketchCurve { id: id.clone() }));
    sels
}

/// A splitting or trimming tool: a plane by name, a planar face as its plane, or any other
/// face as {body, point} (its surface extended).
fn tool(app: &SolveApp, sel: Option<&Sel>) -> Result<(&'static str, Value), String> {
    match sel {
        Some(Sel::Plane { name }) => Ok(("plane", json!(name))),
        Some(Sel::Face { body, index, point }) => match crate::dialogs::planar_face(&app.session, body, *index) {
            Some((_, n)) => Ok(("plane", json!({"origin": pt(*point), "normal": pt(n)}))),
            None => Ok(("tool", json!({"body": body, "point": pt(*point)}))),
        },
        _ => Err("pick a plane or a face as the tool".into()),
    }
}

/// The command a surface dialog makes.
pub fn commands(app: &SolveApp, k: &Surf, inputs: &[SelInput], extra: &Map<String, Value>) -> Result<Vec<(String, Value)>, String> {
    let need = |i: usize, what: &str| if items(inputs, i).is_empty() { Err(format!("select {what} first")) } else { Ok(()) };
    let (cmd, mut p) = match k {
        Surf::Extrude { distance, direction } => {
            need(0, "profiles or curves to extrude")?;
            let mut p = sheet_picks(app, items(inputs, 0))?;
            p["distance"] = json!(distance);
            p["direction"] = json!(EXTRUDE_SIDES.get(*direction).map_or("positive", |(_, d)| *d));
            ("surface.extrude", p)
        }
        Surf::Revolve { angle, symmetric, axis } => {
            need(0, "profiles or curves to revolve")?;
            let mut p = sheet_picks(app, items(inputs, 0))?;
            p["axis"] = match items(inputs, 1).first() {
                Some(Sel::Axis { name }) => json!(name),
                Some(Sel::SketchCurve { id }) => json!(id),
                _ => axis.clone().ok_or("select an axis")?,
            };
            p["angle"] = json!(angle);
            if *symmetric {
                p["symmetric"] = json!(true);
            }
            ("surface.revolve", p)
        }
        Surf::Sweep(opts) => {
            need(0, "profiles or curves to sweep")?;
            need(1, "the path")?;
            let mut p = sheet_picks(app, items(inputs, 0))?;
            let path: Vec<String> =
                items(inputs, 1).iter().filter_map(|x| if let Sel::SketchCurve { id } = x { Some(id.clone()) } else { None }).collect();
            p["path_sketch"] =
                json!(crate::dialogs::curves_sketch(app, &path, p["sketch"].as_u64()).ok_or("the path curves must be in one other sketch")?);
            p["path"] = json!(path);
            crate::dialogs_sweep::params(app, opts, items(inputs, 2).first(), [items(inputs, 3), items(inputs, 4)], &mut p)?;
            ("surface.sweep", p)
        }
        Surf::Loft => {
            need(0, "the sections, one sketch each")?;
            // One section per sketch, in the order the sketches were first picked.
            let st = app.session.world_state();
            let sketch_of = |x: &Sel| match x {
                Sel::Profile { sketch, .. } => Some(*sketch),
                Sel::SketchCurve { id } => st.sketches.iter().rev().find(|ss| ss.sketch.curve_index(id).is_some()).map(|ss| ss.feature),
                _ => None,
            };
            let mut order: Vec<u64> = Vec::new();
            for x in items(inputs, 0) {
                let sk = sketch_of(x).ok_or("a picked curve is no longer in a sketch")?;
                if !order.contains(&sk) {
                    order.push(sk);
                }
            }
            if order.len() < 2 {
                return Err("pick sections in two or more sketches".into());
            }
            let sections: Vec<Value> = order
                .iter()
                .map(|sk| {
                    let picks: Vec<Sel> = items(inputs, 0).iter().filter(|x| sketch_of(x) == Some(*sk)).cloned().collect();
                    sheet_picks(app, &picks)
                })
                .collect::<Result<_, String>>()?;
            ("surface.loft", json!({ "sections": sections }))
        }
        Surf::Patch { continuity } => {
            need(0, "a boundary (sketch profiles or a loop of edges)")?;
            let profiles: Vec<(u64, usize)> =
                items(inputs, 0).iter().filter_map(|x| if let Sel::Profile { sketch, index } = x { Some((*sketch, *index)) } else { None }).collect();
            if let Some((sk, _)) = profiles.first() {
                let idx: Vec<usize> = profiles.iter().filter(|(s, _)| s == sk).map(|(_, i)| *i).collect();
                ("surface.patch", json!({"sketch": sk, "profiles": idx}))
            } else {
                let edges: Vec<Value> =
                    items(inputs, 0).iter().filter_map(|x| if let Sel::Edge { point, .. } = x { Some(pt(*point)) } else { None }).collect();
                let body = items(inputs, 0).iter().find_map(|x| if let Sel::Edge { body, .. } = x { Some(body.clone()) } else { None });
                let mut p = json!({"edges": edges, "body": body});
                if let Some((_, word)) = PATCH_CONTINUITY.get(*continuity).filter(|_| *continuity > 0) {
                    p["continuity"] = json!(word);
                }
                let points: Vec<Value> = items(inputs, 1)
                    .iter()
                    .filter_map(|x| match x {
                        Sel::Vertex { point, .. } | Sel::Face { point, .. } => Some(pt(*point)),
                        _ => None,
                    })
                    .collect();
                if !points.is_empty() {
                    p["points"] = json!(points);
                }
                ("surface.patch", p)
            }
        }
        Surf::Offset { distance, chain } => {
            need(0, "the faces to offset")?;
            let (body, faces) = body_and_faces(items(inputs, 0))?;
            if faces.is_empty() {
                return Err("pick faces to offset".into());
            }
            let mut p = json!({"body": body, "faces": faces, "distance": distance});
            if *chain {
                p["chain"] = json!(true);
            }
            ("surface.offset", p)
        }
        Surf::Ruled { distance, mode, angle, flip, round } => {
            need(0, "the edges to rule from")?;
            let edge_body = |x: &Sel| if let Sel::Edge { body, .. } = x { Some(body.clone()) } else { None };
            let body = items(inputs, 0).iter().find_map(edge_body).ok_or("pick edges of a body")?;
            if items(inputs, 0).iter().any(|x| edge_body(x).as_ref() != Some(&body)) {
                return Err("pick edges of one body".into());
            }
            let edges: Vec<Value> =
                items(inputs, 0).iter().filter_map(|x| if let Sel::Edge { point, .. } = x { Some(pt(*point)) } else { None }).collect();
            let mode = RULED_MODES.get(*mode).map_or("normal", |(_, m)| *m);
            let mut p = json!({"body": body, "edges": edges, "distance": distance, "mode": mode});
            if mode == "angle" {
                p["angle"] = json!(angle);
            }
            if let Some(Sel::Face { body: fb, point, .. }) = items(inputs, 1).first() {
                if *fb != body {
                    return Err("the reference face must be on the edges' body".into());
                }
                p["face"] = pt(*point);
            }
            // A picked direction, else the one the feature was made with (Edit Feature).
            let kept = extra.get("direction").and_then(|v| serde_json::from_value::<[f64; 3]>(v.clone()).ok()).map(|a| Vec3::new(a[0], a[1], a[2]));
            let dir = match items(inputs, 2).first() {
                Some(x) => Some(crate::dialogs::axis_of(app, x).ok_or("the direction must be an axis, a sketch line or a straight edge")?.1),
                None => kept,
            };
            match (mode, dir) {
                ("angle" | "direction", Some(d)) => p["direction"] = pt(d),
                ("direction", None) => return Err("select a direction (an axis, a sketch line or a straight edge) first".into()),
                _ => {}
            }
            if *flip {
                p["flip"] = json!(true);
            }
            if *round {
                p["corners"] = json!("round");
            }
            ("surface.ruled", p)
        }
        Surf::Unstitch | Surf::ReverseNormal => {
            need(0, "a body or some of its faces")?;
            let (body, faces) = body_and_faces(items(inputs, 0))?;
            let cmd = if matches!(k, Surf::Unstitch) { "surface.unstitch" } else { "surface.reverse_normal" };
            (cmd, json!({"body": body, "faces": faces}))
        }
        Surf::Stitch { tolerance } => {
            need(0, "the surfaces to stitch")?;
            let mut p = json!({ "bodies": bodies(inputs, 0) });
            if !tolerance.trim().is_empty() {
                p["tolerance"] = json!(tolerance);
            }
            ("surface.stitch", p)
        }
        Surf::Thicken { thickness, symmetric, operation } => {
            need(0, "the surfaces to thicken")?;
            (
                "surface.thicken",
                json!({"bodies": bodies(inputs, 0), "thickness": thickness, "symmetric": symmetric, "operation": OPS.get(*operation).copied().unwrap_or("new")}),
            )
        }
        Surf::Extend { distance } => {
            need(0, "the edges to extend")?;
            let body =
                items(inputs, 0).iter().find_map(|x| if let Sel::Edge { body, .. } = x { Some(body.clone()) } else { None }).unwrap_or_default();
            let edges: Vec<Value> =
                items(inputs, 0).iter().filter_map(|x| if let Sel::Edge { point, .. } = x { Some(pt(*point)) } else { None }).collect();
            ("surface.extend", json!({"body": body, "edges": edges, "distance": distance}))
        }
        Surf::Trim => {
            need(0, "the surface to trim")?;
            need(1, "the trimming tool")?;
            need(2, "the side to keep (a point on it)")?;
            let body = bodies(inputs, 0).into_iter().next().unwrap_or_default();
            let (key, t) = tool(app, items(inputs, 1).first())?;
            let keep = items(inputs, 2)
                .first()
                .and_then(|x| if let Sel::Face { point, .. } = x { Some(pt(*point)) } else { None })
                .ok_or("pick the side to keep")?;
            let mut p = json!({"body": body, "keep": keep});
            p[key] = t;
            ("surface.trim", p)
        }
        Surf::Untrim { holes, outer, in_place } => {
            need(0, "the face to untrim")?;
            let (body, point) = items(inputs, 0)
                .first()
                .and_then(|x| if let Sel::Face { body, point, .. } = x { Some((body.clone(), pt(*point))) } else { None })
                .ok_or("pick the face to untrim")?;
            let mut p = json!({"face": {"body": body, "point": point}});
            match holes {
                0 => p["holes"] = json!("all"),
                1 => {
                    let picked: Vec<Value> =
                        items(inputs, 1).iter().filter_map(|x| if let Sel::Edge { point, .. } = x { Some(pt(*point)) } else { None }).collect();
                    if picked.is_empty() {
                        return Err("pick edges of the holes to remove".into());
                    }
                    p["holes"] = json!(picked);
                }
                _ => p["holes"] = json!([]),
            }
            if *outer {
                p["outer"] = json!(true);
            }
            if *in_place {
                p["in_place"] = json!(true);
            }
            ("surface.untrim", p)
        }
        Surf::SplitFace => {
            need(0, "the faces to split")?;
            need(1, "the splitting tool")?;
            let faces: Vec<Value> =
                items(inputs, 0).iter().filter_map(|x| if let Sel::Face { point, .. } = x { Some(pt(*point)) } else { None }).collect();
            let body = items(inputs, 0).iter().find_map(|x| if let Sel::Face { body, .. } = x { Some(body.clone()) } else { None });
            let (key, t) = tool(app, items(inputs, 1).first())?;
            let mut p = json!({"faces": faces, "body": body});
            p[key] = t;
            ("solid.split_face", p)
        }
    };
    if let Value::Object(m) = &mut p {
        // A ruled surface's kept direction is used above, or dropped with its mode.
        for (k, v) in extra.iter().filter(|(k, _)| !(cmd == "surface.ruled" && k.as_str() == "direction")) {
            m.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Ok(vec![(cmd.to_string(), p)])
}

/// A tool as a selection again: a plane, else the face at the stored point.
fn tool_sel(app: &SolveApp, plane: &PlaneRef, tool: Option<&FaceAt>) -> Option<Sel> {
    match tool {
        Some(t) => crate::dialogs::face_sel(&app.session, t.point.0),
        None => crate::dialogs::plane_sel(&app.session, plane),
    }
}

/// The dialog for editing a surface feature.
/// `frame` maps the feature's component frame to the world, for an axis stored as a line.
pub fn for_feature(app: &SolveApp, kind: &FeatureKind, frame: &Mat) -> Option<(Kind, Vec<SelInput>, Map<String, Value>)> {
    let s = &app.session;
    let edge = |p: &solidcraft_engine::doc::Position| crate::dialogs::edge_sel(s, p.0);
    let (k, fill): (Surf, Vec<Vec<Sel>>) = match kind {
        FeatureKind::SurfaceRevolve { sketch, profiles, curves, axis, angle, angle2, symmetric } => {
            let (pick, stored) = match axis {
                AxisRef::World { axis } => (Some(Sel::Axis { name: axis.to_ascii_uppercase() }), None),
                AxisRef::SketchAxis { axis } => (Some(Sel::Axis { name: axis.to_ascii_lowercase() }), None),
                AxisRef::SketchLine { curve } => (Some(Sel::SketchCurve { id: curve.clone() }), None),
                AxisRef::Line { origin, dir } => (
                    None,
                    Some(json!({
                        "origin": pt(solidcraft_engine::doc::apply_point(frame, origin.0)),
                        "dir": pt(solidcraft_engine::doc::apply_vector(frame, dir.0))
                    })),
                ),
            };
            let mut extra = Map::new();
            if let Some(a2) = angle2 {
                extra.insert("angle2".into(), json!(a2));
            }
            let k = Surf::Revolve { angle: angle.to_string(), symmetric: *symmetric, axis: stored };
            let mut ins = inputs(&k);
            for (inp, items) in ins.iter_mut().zip([sheet_sels(app, sketch.0, profiles.as_ref(), curves), pick.into_iter().collect()]) {
                inp.items = items;
            }
            return Some((Kind::Surface(k), ins, extra));
        }
        FeatureKind::SurfaceSweep { sketch, profiles, curves, path, orientation, twist, scale, rails, rail_fit, .. } => {
            let (opts, direction) = crate::dialogs_sweep::SweepOpts::of(orientation, twist, scale, *rail_fit, frame);
            let [rail, rail2] = crate::dialogs_sweep::rail_sels(rails);
            (
                Surf::Sweep(opts),
                vec![
                    sheet_sels(app, sketch.0, profiles.as_ref(), curves),
                    path.iter().map(|id| Sel::SketchCurve { id: id.clone() }).collect(),
                    direction.into_iter().collect(),
                    rail,
                    rail2,
                ],
            )
        }
        FeatureKind::SurfaceLoft { sections, .. } => {
            (Surf::Loft, vec![sections.iter().flat_map(|sec| sheet_sels(app, sec.sketch.0, sec.profiles.as_ref(), &sec.curves)).collect()])
        }
        FeatureKind::SurfaceExtrude { sketch, profiles, curves, extent } => {
            let sels = sheet_sels(app, sketch.0, profiles.as_ref(), curves);
            let direction = match extent.direction {
                Direction::Positive => 0,
                Direction::Symmetric => 1,
                Direction::Negative => 2,
            };
            let k = Surf::Extrude { distance: extent.distance.to_string(), direction };
            let mut ins = inputs(&k);
            if let Some(input) = ins.first_mut() {
                input.items = sels;
            }
            let mut extra = Map::new();
            extra.insert("convention".into(), json!(extent.convention));
            return Some((Kind::Surface(k), ins, extra));
        }
        FeatureKind::Patch { sketch, edges, crown, continuity, points, .. } => {
            let mut sels: Vec<Sel> = edges.iter().filter_map(edge).collect();
            if let Some(sk) = sketch {
                let st = s.world_state();
                if let Some(ss) = st.sketch(sk.0) {
                    sels.extend((0..ss.profiles.len()).map(|index| Sel::Profile { sketch: sk.0, index }));
                }
            }
            let index = PATCH_CONTINUITY.iter().position(|(_, w)| *w == continuity.all.as_str()).unwrap_or(0);
            // What the dialog does not show is kept as it was: the crown, edges with a
            // continuity of their own, and points that are not on a face any more.
            let through: Vec<Sel> = points.iter().filter_map(|p| crate::dialogs::face_sel(s, p.0)).collect();
            let mut extra = Map::new();
            if *crown != 0.0 {
                extra.insert("crown".into(), json!(crown));
            }
            if !continuity.edges.is_empty() {
                extra.insert("continuity".into(), serde_json::to_value(continuity).unwrap_or(Value::Null));
            }
            if through.len() != points.len() {
                extra.insert("points".into(), json!(points.iter().map(|p| pt(p.0)).collect::<Vec<_>>()));
            }
            let k = Surf::Patch { continuity: index };
            let mut ins = inputs(&k);
            for (input, picks) in ins.iter_mut().zip([sels, through]) {
                input.items = picks;
            }
            return Some((Kind::Surface(k), ins, extra));
        }
        FeatureKind::SurfaceOffset { faces, distance, chain, .. } => (
            Surf::Offset { distance: distance.to_string(), chain: *chain },
            vec![faces.iter().filter_map(|p| crate::dialogs::face_sel(s, p.0)).collect()],
        ),
        FeatureKind::SurfaceRuled { edges, distance, mode, angle, direction, flip, corners, face, .. } => {
            let k = Surf::Ruled {
                distance: distance.to_string(),
                mode: match mode {
                    RuledMode::Normal => 0,
                    RuledMode::Tangent => 1,
                    RuledMode::Angle => 2,
                    RuledMode::Direction => 3,
                },
                angle: angle.as_deref().map(str::to_string).unwrap_or_else(|| "5 deg".into()),
                flip: *flip,
                round: *corners == RuledCorners::Round,
            };
            let mut ins = inputs(&k);
            let fill = [edges.iter().filter_map(edge).collect(), face.and_then(|p| crate::dialogs::face_sel(s, p.0)).into_iter().collect()];
            for (inp, items) in ins.iter_mut().zip(fill) {
                inp.items = items;
            }
            let mut extra = Map::new();
            if let Some(d) = direction {
                extra.insert("direction".into(), pt(d.0));
            }
            return Some((Kind::Surface(k), ins, extra));
        }
        FeatureKind::Unstitch { body, faces } | FeatureKind::ReverseNormal { body, faces } => {
            let sels: Vec<Sel> = if faces.is_empty() {
                vec![Sel::Body { name: body.to_string() }]
            } else {
                faces.iter().filter_map(|p| crate::dialogs::face_sel(s, p.0)).collect()
            };
            (if matches!(kind, FeatureKind::Unstitch { .. }) { Surf::Unstitch } else { Surf::ReverseNormal }, vec![sels])
        }
        FeatureKind::Stitch { bodies, tolerance } => (
            Surf::Stitch { tolerance: if tolerance.trim() == "0" { String::new() } else { tolerance.to_string() } },
            vec![bodies.iter().map(|n| Sel::Body { name: n.to_string() }).collect()],
        ),
        FeatureKind::Thicken { bodies, thickness, symmetric, operation, .. } => (
            Surf::Thicken { thickness: thickness.to_string(), symmetric: *symmetric, operation: op_index(operation) },
            vec![bodies.iter().map(|n| Sel::Body { name: n.to_string() }).collect()],
        ),
        FeatureKind::SurfaceExtend { edges, distance, mode, up_to, neighbours, .. } => {
            // The dialog edits the edges and the distance; the rest is kept as it was.
            let mut extra = Map::new();
            if !mode.is_natural() {
                extra.insert("mode".into(), json!(mode));
            }
            if !*neighbours {
                extra.insert("neighbours".into(), json!(false));
            }
            match up_to {
                Some(solidcraft_engine::doc::ExtendTo::Bounds) => {
                    extra.insert("up_to".into(), json!("bounds"));
                }
                Some(solidcraft_engine::doc::ExtendTo::Face(t)) => {
                    extra.insert("up_to".into(), json!({"body": t.body, "point": pt(t.point.0)}));
                }
                Some(solidcraft_engine::doc::ExtendTo::Plane(PlaneRef::Origin { name })) => {
                    extra.insert("up_to".into(), json!(name));
                }
                Some(solidcraft_engine::doc::ExtendTo::Plane(PlaneRef::Construction { name })) => {
                    extra.insert("up_to".into(), json!(name));
                }
                _ => {}
            }
            let k = Surf::Extend { distance: distance.to_string() };
            let mut ins = inputs(&k);
            if let Some(inp) = ins.first_mut() {
                inp.items = edges.iter().filter_map(edge).collect();
            }
            return Some((Kind::Surface(k), ins, extra));
        }
        FeatureKind::SurfaceTrim { body, plane, tool, keep } => (
            Surf::Trim,
            vec![
                vec![Sel::Body { name: body.to_string() }],
                tool_sel(app, plane, tool.as_ref()).into_iter().collect(),
                crate::dialogs::face_sel(s, keep.0).into_iter().collect(),
            ],
        ),
        FeatureKind::SurfaceUntrim { face, holes, outer, in_place } => {
            let (index, picked) = match holes {
                None => (0, Vec::new()),
                Some(points) if points.is_empty() => (2, Vec::new()),
                Some(points) => (1, points.iter().filter_map(edge).collect()),
            };
            (
                Surf::Untrim { holes: index, outer: *outer, in_place: *in_place },
                vec![crate::dialogs::face_sel(s, face.point.0).into_iter().collect(), picked],
            )
        }
        FeatureKind::SplitFace { faces, plane, tool, .. } => (
            Surf::SplitFace,
            vec![faces.iter().filter_map(|p| crate::dialogs::face_sel(s, p.0)).collect(), tool_sel(app, plane, tool.as_ref()).into_iter().collect()],
        ),
        _ => return None,
    };
    let mut ins = inputs(&k);
    for (inp, items) in ins.iter_mut().zip(fill) {
        inp.items = items;
    }
    Some((Kind::Surface(k), ins, Map::new()))
}
