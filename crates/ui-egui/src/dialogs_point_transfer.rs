//! Two explicit anchor picks; opening/cancelling never changes the document.

use serde_json::{Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::geom::Vec3;

use crate::SolveApp;
use crate::dialogs::{Dialog, Kind, check, row_label};
use crate::selection::{BODIES, EDGES, FACES, POINTS, SelInput, VERTICES};

#[derive(Clone, Debug, PartialEq)]
pub struct Transfer {
    pub paste: bool,
    pub copy: bool,
}

pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let paste = match id {
        "edit.paste.point_to_point" => true,
        "objects.move.point_to_point" => false,
        _ => return None,
    };
    let points = if app.session.active_sketch.is_some() { POINTS } else { VERTICES | EDGES | FACES | POINTS };
    let mut inputs = Vec::new();
    if !paste {
        inputs.push(SelInput::new("Objects", BODIES, true));
    }
    inputs.push(SelInput::new("From", points, false));
    inputs.push(SelInput::new("To", points, false));
    Some((Kind::PointTransfer(Transfer { paste, copy: false }), inputs))
}

pub fn rows(ui: &mut egui::Ui, t: &mut Transfer) {
    if !t.paste {
        row_label(ui, "Create Copy");
        check(ui, &mut t.copy);
        ui.end_row();
    }
}

/// World point of a vertex, face, edge or sketch point (in any visible sketch).
pub(crate) fn anchor(app: &SolveApp, inputs: &[SelInput], i: usize) -> Result<Vec3, String> {
    let sel = inputs.get(i).and_then(|input| input.items.first()).ok_or("Pick the From and To points")?;
    let point = match sel {
        Sel::Vertex { point, .. } | Sel::Face { point, .. } | Sel::Edge { point, .. } => *point,
        Sel::SketchPoint { id } => {
            let state = app.session.world_state();
            let active = app.session.active_sketch;
            let local_id = active.and_then(|sid| state.sketch(sid)).is_some_and(|s| s.sketch.resolve_point(id).is_some());
            let (sketch, id) = if local_id {
                (active.ok_or("Edit a sketch to pick its points")?, id.as_str())
            } else if let Some((owner, point)) = id.split_once(':').and_then(|(owner, point)| owner.parse::<u64>().ok().map(|owner| (owner, point))) {
                (owner, point)
            } else {
                (active.ok_or("Edit a sketch to pick its points")?, id.as_str())
            };
            let sketch = state.sketch(sketch).ok_or("Sketch no longer exists")?;
            let at = if id == "origin" {
                solidcraft_engine::geom::Vec2::ZERO
            } else {
                sketch.sketch.resolve_point(id).and_then(|p| sketch.sketch.point(p)).ok_or("Sketch point no longer exists")?
            };
            sketch.plane.to_world(at)
        }
        _ => return Err("Pick a point".into()),
    };
    if !point.is_finite() {
        return Err("Point must be finite".into());
    }
    Ok(point)
}

pub fn commands(app: &SolveApp, d: &Dialog, t: &Transfer) -> Result<Vec<(String, Value)>, String> {
    let first = usize::from(!t.paste);
    let from = anchor(app, &d.inputs, first)?;
    let to = anchor(app, &d.inputs, first + 1)?;
    let mut p = if let Some(sketch) = app.session.active_sketch {
        let state = app.session.world_state();
        let plane = state.sketch(sketch).ok_or("Sketch no longer exists")?.plane;
        // A source point picked on the original sketch names the clipboard's own coordinates,
        // even when the destination sketch uses a different plane.
        let source_plane = if t.paste {
            d.inputs
                .first()
                .and_then(|i| i.items.first())
                .and_then(|sel| match sel {
                    Sel::SketchPoint { id } if state.sketch(sketch).is_none_or(|s| s.sketch.resolve_point(id).is_none()) => {
                        id.split_once(':').and_then(|(owner, _)| owner.parse::<u64>().ok())
                    }
                    _ => None,
                })
                .filter(|sid| app.session.sketch_clipboard.as_ref().is_some_and(|clip| clip.sketch == *sid))
                .and_then(|sid| state.sketch(sid))
                .map(|s| s.plane)
                .unwrap_or(plane)
        } else {
            plane
        };
        let (a, b) = (source_plane.to_local(from), plane.to_local(to));
        json!({"from": [a.x,a.y], "to": [b.x,b.y]})
    } else {
        json!({"from": [from.x,from.y,from.z], "to": [to.x,to.y,to.z]})
    };
    let cmd = if t.paste {
        "edit.paste.point_to_point"
    } else {
        let objects = d.inputs.first().map(|i| i.items.as_slice()).unwrap_or(&[]);
        if objects.is_empty() {
            return Err("Select bodies or one component placement".into());
        }
        p["objects"] = json!(objects);
        p["copy"] = json!(t.copy);
        "objects.move.point_to_point"
    };
    Ok(vec![(cmd.into(), p)])
}

/// Empty-space sketch picks use the active sketch plane, without adding geometry.
pub fn pick_position(d: &mut Dialog, session: &solidcraft_engine::Session, point: Vec3) -> bool {
    if !matches!(d.kind, Kind::PointTransfer(_)) {
        return false;
    }
    if !d.active_input().is_some_and(|input| input.label == "From" || input.label == "To") {
        return false;
    }
    d.pick(session, Sel::Vertex { body: String::new(), point });
    true
}

#[cfg(test)]
#[path = "dialogs_point_transfer_tests.rs"]
mod tests;
