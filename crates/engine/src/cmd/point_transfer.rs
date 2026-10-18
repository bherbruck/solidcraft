//! Anchor-based placement, shared by the interactive dialogs and JSON commands. Sketch
//! anchors are in sketch coordinates; body and occurrence anchors are world coordinates.
//! Object copies retain references to bodies / exact occurrence paths in this document.

use serde_json::{Value, json};
use solidcraft_doc::{BodyName, FeatureKind, apply_vector, mat_inverse};
use solidcraft_geom::{Vec2, Vec3};

use super::CommandSpec;
use crate::params::{bad, bool_, finite, req_vec2, req_vec3, string_list};
use crate::{Result, Sel, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("objects.copy", "Copy Bodies or Component", copy).icon("copy").noundo()
        .enabled(super::not_in_sketch)
        .params("objects?: [{type: body, name}…] or [{type: occurrence, path: [ids]}] (default: selection). References must still exist on paste; component copies are linked instances"),
    CommandSpec::new("objects.move.point_to_point", "Point to Point Move/Copy", move_objects).icon("move")
        .enabled(super::not_in_sketch)
        .params("objects?: bodies or one exact occurrence as in objects.copy (default selection); from: [x,y,z], to: [x,y,z] in world coordinates; copy?: bool (default false; component copies are linked instances)"),
    CommandSpec::new("edit.paste.point_to_point", "Point to Point Paste", paste).icon("paste")
        .key("Ctrl+Shift+V")
        .enabled(|s| {
            if s.active_sketch.is_some() {
                s.sketch_clipboard.as_ref().map(|_| ()).ok_or_else(|| "Copy sketch geometry first".into())
            } else if s.object_clipboard.is_empty() && s.clipboard.is_empty() {
                Err("Copy bodies, a component or timeline features first".into())
            } else { Ok(()) }
        })
        .params("from, to: source anchor and destination ([x,y] in the active sketch, [x,y,z] in world coordinates otherwise). Pastes sketch geometry with its internal constraints/dimensions, bodies, a linked component instance, or timeline features, in one undo step"),
];

/// Validate paired anchors before any mutation. Old numeric placement remains unchanged.
pub(super) fn sketch_translation(p: &Value, cmd: &str) -> Result<Option<Vec2>> {
    if p.get("from").is_none() && p.get("to").is_none() {
        return Ok(None);
    }
    if p.get("translate").is_some() || p.get("at").is_some() {
        return Err(bad(cmd, "from/to cannot be combined with at or translate"));
    }
    let delta = req_vec2(cmd, p, "to")? - req_vec2(cmd, p, "from")?;
    if finite(delta.x).is_none() || finite(delta.y).is_none() {
        return Err(bad(cmd, "anchor translation exceeds supported numeric range"));
    }
    Ok(Some(delta))
}

fn world_translation(p: &Value, cmd: &str) -> Result<Vec3> {
    if p.get("translate").is_some() || p.get("at").is_some() {
        return Err(bad(cmd, "from/to cannot be combined with at or translate"));
    }
    let delta = req_vec3(cmd, p, "to")? - req_vec3(cmd, p, "from")?;
    if [delta.x, delta.y, delta.z].into_iter().any(|v| finite(v).is_none()) {
        return Err(bad(cmd, "anchor translation exceeds supported numeric range"));
    }
    Ok(delta)
}

fn chosen(s: &Session, p: &Value, cmd: &str) -> Result<Vec<Sel>> {
    let objects: Vec<Sel> = match p.get("objects") {
        Some(v) => {
            if v.as_array().is_none_or(|a| a.is_empty() || a.len() > 1000) {
                return Err(bad(cmd, "objects must list 1…1000 bodies or one occurrence"));
            }
            if v.as_array().is_some_and(|a| a.iter().any(|o| o.get("path").and_then(Value::as_array).is_some_and(|p| p.is_empty() || p.len() > 64))) {
                return Err(bad(cmd, "occurrence path must contain 1…64 ids"));
            }
            serde_json::from_value(v.clone()).map_err(|_| bad(cmd, "invalid objects"))?
        }
        None => s
            .selection
            .iter()
            .filter_map(|x| match x {
                Sel::Body { .. } | Sel::Occurrence { .. } => Some(x.clone()),
                Sel::Face { body, .. } | Sel::Edge { body, .. } | Sel::Vertex { body, .. } if !body.is_empty() => {
                    Some(Sel::Body { name: body.clone() })
                }
                _ => None,
            })
            .collect(),
    };
    if objects.is_empty() || objects.len() > 1000 {
        return Err(bad(cmd, "select bodies or one component placement"));
    }
    if objects.iter().any(|x| matches!(x, Sel::Occurrence { .. })) && objects.len() != 1 {
        return Err(bad(cmd, "select one component placement, or bodies without components"));
    }
    let state = s.world_state();
    for object in &objects {
        match object {
            Sel::Body { name } if state.body(name).is_some() => {}
            Sel::Occurrence { path } => {
                s.occurrence_placement(path).map_err(|e| bad(cmd, e.to_string()))?;
            }
            _ => return Err(bad(cmd, "a copied body or component placement no longer exists")),
        }
    }
    let mut unique = Vec::new();
    for object in objects {
        if !unique.contains(&object) {
            unique.push(object);
        }
    }
    Ok(unique)
}

fn copy(s: &mut Session, p: &Value) -> Result<Value> {
    let objects = chosen(s, p, "objects.copy")?;
    let out = json!({"copied": objects});
    s.object_clipboard = objects;
    s.clipboard.clear();
    Ok(out)
}

fn transfer(s: &mut Session, objects: Vec<Sel>, delta: Vec3, copy: bool, cmd: &str) -> Result<Value> {
    if let Some(Sel::Occurrence { path }) = objects.first() {
        let placement = s.occurrence_placement(path).map_err(|e| bad(cmd, e.to_string()))?;
        let mut params = json!({"occurrence": placement.occurrence, "path": path, "space": "world", "translate": [delta.x,delta.y,delta.z]});
        if copy {
            // occurrence.copy's translation is in the parent's frame, including pending moves.
            let inverse = mat_inverse(&placement.parent_world).ok_or_else(|| bad(cmd, "invalid parent placement"))?;
            let local = apply_vector(&inverse, delta);
            params["translate"] = json!([local.x, local.y, local.z]);
            return super::component::copy_occurrence(s, &params);
        }
        return super::component::move_occurrence(s, &params);
    }
    let bodies: Vec<_> = objects.iter().filter_map(|o| if let Sel::Body { name } = o { Some(name) } else { None }).collect();
    let params = json!({"bodies": bodies});
    // Reuse solid.move's body-name mapping, without a nested execute (and second undo).
    // The vector uses the selected placement, which may differ from the active instance.
    let local = crate::frames::to_local(s, "solid.move", &params).unwrap_or(params);
    let names = string_list(&local, "bodies");
    let state = s.model.state();
    if names.iter().any(|name| state.body(name).is_none_or(|body| s.doc.model_body_component(body) != s.active_component)) {
        return Err(bad(
            cmd,
            "activate the bodies' component before moving or pasting bodies; use a component placement to move an assembly instance",
        ));
    }
    let placed = s.placed_document().body_frames_with_source(&state);
    let frames: Vec<_> = objects
        .iter()
        .filter_map(|o| match o {
            Sel::Body { name } => placed.iter().find(|(world, _, _, _, _)| world == name).map(|(_, _, _, _, frame)| frame),
            _ => None,
        })
        .collect();
    let frame = frames.first().copied().ok_or_else(|| bad(cmd, "body placement no longer exists"))?;
    if frames.len() != objects.len()
        || frames.iter().any(|other| *other != frame)
        || names.iter().collect::<std::collections::BTreeSet<_>>().len() != names.len()
    {
        return Err(bad(cmd, "select bodies from one component placement"));
    }
    let inverse = mat_inverse(frame).ok_or_else(|| bad(cmd, "invalid body placement"))?;
    let t = apply_vector(&inverse, delta);
    if [t.x, t.y, t.z].into_iter().any(|v| finite(v).is_none()) {
        return Err(bad(cmd, "local translation exceeds supported numeric range"));
    }
    let kind = FeatureKind::Move {
        bodies: names.into_iter().map(BodyName).collect(),
        translate: [format!("{} mm", t.x).into(), format!("{} mm", t.y).into(), format!("{} mm", t.z).into()],
        rotate_axis: None,
        angle: None,
        copy,
    };
    super::features::add_feature_in(s, &json!({}), kind, false)
}

fn move_objects(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "objects.move.point_to_point";
    let delta = world_translation(p, cmd)?;
    let objects = chosen(s, p, cmd)?;
    transfer(s, objects, delta, bool_(p, "copy").unwrap_or(false), cmd)
}

fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "edit.paste.point_to_point";
    if s.active_sketch.is_some() {
        sketch_translation(p, cmd)?.ok_or_else(|| bad(cmd, "pick from and to anchors"))?;
        return super::sketch_clipboard::paste(s, p);
    }
    let delta = world_translation(p, cmd)?;
    if !s.object_clipboard.is_empty() {
        let objects = chosen(s, &json!({"objects": s.object_clipboard}), cmd)?;
        return transfer(s, objects, delta, true, cmd);
    }
    let inverse = mat_inverse(&s.active_component_frame()).ok_or_else(|| bad(cmd, "invalid active component placement"))?;
    let local = apply_vector(&inverse, delta);
    super::clipboard::paste(s, &json!({"translate": [local.x,local.y,local.z]}))
}

#[cfg(test)]
#[path = "point_transfer_tests.rs"]
mod tests;
