//! Undo/redo, the timeline, deletion and selection.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use solidcraft_doc::{Feature, FeatureKind};

use super::CommandSpec;
use crate::params::{bad, bool_, num, str_, string_list};
use crate::{EngineError, Result, Sel, Session, Snapshot};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("edit.undo", "Undo", undo).icon("undo").key("Ctrl+Z").noundo().doc_only(),
    CommandSpec::new("edit.redo", "Redo", redo).icon("redo").key("Ctrl+Y").noundo().doc_only(),
    CommandSpec::new("edit.checkpoint", "Checkpoint", checkpoint).icon("checkpoint")
        .noundo()
        .doc_only()
        .params("→ {checkpoint: id}: remember the design, active sketch and undo history (restore with edit.restore_checkpoint)"),
    CommandSpec::new("edit.restore_checkpoint", "Restore Checkpoint", restore_checkpoint).icon("restore")
        .noundo()
        .doc_only()
        .params("checkpoint: id; forget?: bool (true = drop it without going back). Goes back to it, undo history included"),
    CommandSpec::new("timeline.compute_all", "Compute All", compute_all).at("SOLID", "MODIFY").icon("compute").noundo(),
    CommandSpec::new("timeline.delete", "Delete", delete)
        .at("SOLID", "MODIFY")
        .icon("delete")
        .key("Delete")
        .params("features: [id or name] (a sketch takes its dependent features with it) → deleted: [ids]; failing?: [{feature, error}] (later features that worked and now fail)"),
    CommandSpec::new("timeline.rename", "Rename", rename).icon("rename").params("feature: id|name, name (references to it follow)"),
    CommandSpec::new("timeline.group", "New Feature Folder", group)
        .icon("folder")
        .doc_only()
        .params("name?; features?: [id|name]; parent?: folder id (features leave any other timeline folder)"),
    CommandSpec::new("timeline.ungroup", "Remove Feature Folders", ungroup).icon("folder")
        .doc_only()
        .params("group: id | groups: [ids] (features remain in the timeline)"),
    CommandSpec::new("timeline.rename_group", "Rename Feature Folder", rename_group).icon("rename").doc_only().params("group: id, name"),
    CommandSpec::new("timeline.move_to_group", "Move Features to Folder", move_to_group).icon("folder")
        .doc_only()
        .params("features?: [id|name], groups?: [ids], group?: id (omit or null to move to the timeline root)"),
    CommandSpec::new("timeline.move_many", "Move Timeline Items", move_many).icon("reorder").params(
        "features?: [id|name], groups?: [folder ids], group?: folder id, position: insertion slot; atomically changes folder membership and moves all selected contents as one block",
    ),
    CommandSpec::new("timeline.rollback", "Move Timeline Marker", rollback).icon("rollback").params("position: number of features to keep active (omit = end)"),
    CommandSpec::new("timeline.roll_to", "Roll History Marker", rollback).icon("rollback")
        .params("position: number of features to keep active (omit = end) | feature: id|name (the marker goes right after it)"),
    CommandSpec::new("timeline.reorder", "Reorder Feature", reorder).icon("reorder")
        .params("feature: id|name, position: its new index; refused when a feature that worked would fail"),
    CommandSpec::new("timeline.reorder_many", "Reorder Features", reorder_many).icon("reorder").params(
        "features: [id|name], position: insertion slot in the current history; selected features keep their relative order; refused when a feature that worked would fail",
    ),
    CommandSpec::new("timeline.redefine", "Edit Feature", redefine).icon("edit")
        .params("feature: id|name, command: the feature's command id, params: its parameters (the feature is rebuilt in place)"),
    CommandSpec::new("timeline.dependents", "Feature Dependents", dependents).icon("graph")
        .noundo()
        .params("feature: id|name → features deleted with it, and features that would fail without it"),
    CommandSpec::new("timeline.suppress", "Suppress Feature", suppress).icon("suppress").params("feature: id|name, suppressed?: bool (default toggles)"),
    CommandSpec::new("timeline.edit", "Edit Feature", edit_feature).icon("edit").params(
        "feature: id|name, set: {fields to change, merged into the feature's definition as timeline.inspect returns it, \
         e.g. {\"extent\": {\"distance\": \"30\"}} (extrude), {\"angle\": \"90 deg\"} (revolve); \
         expressions are text} → recomputed: features rebuilt, restored: features whose earlier result for the same inputs was reused",
    ),
    CommandSpec::new("timeline.inspect", "Inspect Feature", inspect_feature).icon("inspect").noundo().params(
        "feature: id|name → type, definition (the fields timeline.edit's set changes, with their current values), \
         inputs: [{label, param, expression, unit}] (named inputs parameters.change can also set), error, warning",
    ),
    CommandSpec::new("select.set", "Select", select_set).icon("select")
        .noundo()
        .params("items: [{type: body|edge|face|feature|sketch_curve|sketch_point|profile, …}], add?: bool"),
    CommandSpec::new("select.clear", "Clear Selection", select_clear).icon("select").noundo(),
];

fn undo(s: &mut Session, _p: &Value) -> Result<Value> {
    let Some(prev) = s.undo.pop() else { return Err(EngineError::Other("nothing to undo".into())) };
    let now = go_to(s, prev);
    let label = now.label.clone();
    s.redo.push(now);
    Ok(json!({"undone": label}))
}

fn redo(s: &mut Session, _p: &Value) -> Result<Value> {
    let Some(next) = s.redo.pop() else { return Err(EngineError::Other("nothing to redo".into())) };
    let now = go_to(s, next);
    let label = now.label.clone();
    s.undo.push(now);
    Ok(json!({"redone": label}))
}

/// Put the session at `snap`; returns where it was, under the same label.
fn go_to(s: &mut Session, snap: Snapshot) -> Snapshot {
    let was = Snapshot { label: snap.label, doc: s.doc.clone(), active_sketch: s.active_sketch, pending_moves: s.pending_moves.clone() };
    let same = std::sync::Arc::ptr_eq(&s.doc, &snap.doc);
    s.doc = snap.doc;
    s.active_sketch = snap.active_sketch;
    s.pending_moves = snap.pending_moves;
    s.selection.clear();
    // A new document is evaluated by `execute`.
    if same {
        s.refresh();
    }
    was
}

fn checkpoint(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({"checkpoint": s.checkpoint()}))
}

fn restore_checkpoint(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "edit.restore_checkpoint";
    let id = p.get("checkpoint").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`checkpoint` must be an id from edit.checkpoint"))?;
    if bool_(p, "forget") == Some(true) {
        s.drop_checkpoint(id);
        return Ok(json!({"checkpoint": id, "restored": false}));
    }
    s.restore_checkpoint(id)?;
    Ok(json!({"checkpoint": id, "restored": true}))
}

fn compute_all(s: &mut Session, _p: &Value) -> Result<Value> {
    s.model = solidcraft_doc::Model::new();
    s.refresh();
    let errors = s.model.results.iter().filter(|r| r.error.is_some()).count();
    Ok(json!({"features": s.model.results.len(), "errors": errors}))
}

fn feature_id(s: &Session, v: Option<&Value>, cmd: &str) -> Result<u64> {
    let key = match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, "`feature` must be an id or name")),
    };
    s.doc.find_feature(&key).map(|f| f.id).ok_or_else(|| bad(cmd, format!("no feature `{key}`")))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.delete";
    let mut keys = string_list(p, "features");
    if let Some(Value::Array(a)) = p.get("features") {
        keys.extend(a.iter().filter_map(Value::as_u64).map(|n| n.to_string()));
    }
    if let Some(f) = p.get("feature") {
        keys.push(match f {
            Value::Number(n) => n.to_string(),
            Value::String(x) => x.clone(),
            _ => String::new(),
        });
    }
    if keys.is_empty() {
        return Err(bad(cmd, "`features` must list features"));
    }
    // Features that work now; any of them failing after the delete is reported (a later
    // feature that names a deleted body, #267).
    let worked: std::collections::BTreeSet<u64> = s.model.results.iter().filter(|r| r.error.is_none() && !r.skipped).map(|r| r.id).collect();
    let mut gone = Vec::new();
    for k in keys {
        let Some(id) = s.doc.find_feature(&k).map(|f| f.id) else {
            if gone.is_empty() {
                return Err(bad(cmd, format!("no feature `{k}`")));
            }
            continue;
        };
        gone.extend(s.doc_mut().delete_feature(id)?);
    }
    if s.active_sketch.is_some_and(|a| gone.contains(&a)) {
        s.active_sketch = None;
    }
    s.selection.clear();
    s.refresh();
    let failing: Vec<Value> = s
        .model
        .results
        .iter()
        .filter(|r| worked.contains(&r.id))
        .filter_map(|r| r.error.as_ref().map(|e| json!({"feature": r.name, "error": e})))
        .collect();
    if failing.is_empty() {
        return Ok(json!({"deleted": gone}));
    }
    Ok(json!({"deleted": gone, "failing": failing}))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.rename";
    let id = feature_id(s, p.get("feature"), cmd)?;
    let name =
        str_(p, "name").map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128).ok_or_else(|| bad(cmd, "`name` must be 1…128 characters"))?;
    let old = s.doc.feature(id).map(|f| f.name.clone()).ok_or_else(|| bad(cmd, "feature"))?;
    if s.doc.features.iter().any(|f| f.name == name && f.id != id) {
        return Err(bad(cmd, format!("another feature is called `{name}`")));
    }
    if s.doc.feature(id).is_some_and(|f| matches!(f.kind, FeatureKind::ConstructionPlane { .. })) {
        super::features::check_plane_name(name, cmd)?;
    }
    if name == old {
        return Ok(json!({"feature": id, "name": name}));
    }
    let names = BTreeMap::from([(old.clone(), name.to_string())]);
    let sketches = BTreeMap::new();
    let doc = s.doc_mut();
    if let Some(f) = doc.feature_mut(id) {
        f.name = name.to_string();
    }
    for f in doc.features.iter_mut() {
        f.kind.remap_refs(&sketches, &names);
    }
    Ok(json!({"feature": id, "name": name}))
}

fn timeline_group_id(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    timeline_group_id_field(s, p, "group", cmd)
}

fn timeline_group_id_field(s: &Session, p: &Value, field: &str, cmd: &str) -> Result<u64> {
    let id = p.get(field).and_then(Value::as_u64).ok_or_else(|| bad(cmd, format!("`{field}` must be a timeline folder id")))?;
    if s.doc.timeline_groups.iter().any(|group| group.id == id) { Ok(id) } else { Err(bad(cmd, format!("no timeline folder {id}"))) }
}

fn group_features(s: &Session, p: &Value, required: bool, cmd: &str) -> Result<Vec<u64>> {
    let values = match p.get("features") {
        Some(Value::Array(values)) => values,
        None if !required => return Ok(Vec::new()),
        _ => return Err(bad(cmd, "`features` must be a list of feature ids or names")),
    };
    if values.is_empty() && required {
        return Err(bad(cmd, "`features` must list at least one feature"));
    }
    if values.len() > 10_000 {
        return Err(bad(cmd, "too many features"));
    }
    let mut ids = Vec::new();
    for value in values {
        let id = feature_id(s, Some(value), cmd)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn group_ids(s: &Session, p: &Value, required: bool, cmd: &str) -> Result<Vec<u64>> {
    let values = match p.get("groups") {
        Some(Value::Array(values)) => values,
        None if !required => return Ok(Vec::new()),
        None => return Err(bad(cmd, "`groups` must be a list of timeline folder ids")),
        _ => return Err(bad(cmd, "`groups` must be a list of timeline folder ids")),
    };
    if values.is_empty() && required {
        return Err(bad(cmd, "`groups` must list at least one folder"));
    }
    if values.len() > 10_000 {
        return Err(bad(cmd, "too many folders"));
    }
    let mut ids = Vec::new();
    for value in values {
        let id = value.as_u64().ok_or_else(|| bad(cmd, "`groups` must contain folder ids"))?;
        if !s.doc.timeline_groups.iter().any(|group| group.id == id) {
            return Err(bad(cmd, format!("no timeline folder {id}")));
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn group_move_would_cycle(s: &Session, target: u64, sources: &[u64]) -> bool {
    let sources: std::collections::BTreeSet<u64> = sources.iter().copied().collect();
    let parents: std::collections::BTreeMap<u64, Option<u64>> = s.doc.timeline_groups.iter().map(|group| (group.id, group.parent)).collect();
    let mut current = Some(target);
    let mut steps = 0usize;
    while let Some(id) = current {
        if sources.contains(&id) {
            return true;
        }
        steps += 1;
        if steps > s.doc.timeline_groups.len() {
            return true;
        }
        current = parents.get(&id).copied().flatten();
    }
    false
}

fn group_feature_ids(doc: &solidcraft_doc::Document, roots: &[u64]) -> std::collections::BTreeSet<u64> {
    let by_id: std::collections::BTreeMap<u64, &solidcraft_doc::TimelineGroup> = doc.timeline_groups.iter().map(|group| (group.id, group)).collect();
    let mut children = std::collections::BTreeMap::<u64, Vec<u64>>::new();
    for group in &doc.timeline_groups {
        if let Some(parent) = group.parent {
            children.entry(parent).or_default().push(group.id);
        }
    }
    let mut pending = roots.to_vec();
    let mut visited = std::collections::BTreeSet::new();
    let mut features = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        if let Some(group) = by_id.get(&id) {
            features.extend(group.features.iter().copied());
            pending.extend(children.get(&id).into_iter().flatten().copied());
        }
    }
    features
}

fn sort_group_features(doc: &mut solidcraft_doc::Document) {
    let positions: std::collections::BTreeMap<u64, usize> = doc.features.iter().enumerate().map(|(index, feature)| (feature.id, index)).collect();
    for group in &mut doc.timeline_groups {
        group.features.sort_by_key(|feature| positions.get(feature).copied().unwrap_or(usize::MAX));
    }
}

fn require_contiguous_groups(doc: &solidcraft_doc::Document, cmd: &str) -> Result<()> {
    let Some(id) = solidcraft_doc::non_contiguous_timeline_group(&doc.features, &doc.timeline_groups) else { return Ok(()) };
    let name = doc.timeline_groups.iter().find(|group| group.id == id).map(|group| group.name.as_str()).unwrap_or("unknown");
    Err(bad(cmd, format!("timeline folder `{name}` must contain one contiguous run of features")))
}

fn group(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.group";
    let mut features = group_features(s, p, false, cmd)?;
    let groups = group_ids(s, p, false, cmd)?;
    let name = match str_(p, "name").map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) if name.len() <= 128 => name.to_string(),
        Some(_) => return Err(bad(cmd, "`name` must be 1…128 characters")),
        None => format!("Folder{}", s.doc.timeline_groups.len() + 1),
    };
    let id = s
        .doc
        .timeline_groups
        .iter()
        .map(|group| group.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| bad(cmd, "no more timeline folder ids are available"))?;
    let parent = match p.get("parent") {
        Some(Value::Null) | None => None,
        Some(_) => Some(timeline_group_id_field(s, p, "parent", cmd)?),
    };
    if let Some(parent) = parent
        && group_move_would_cycle(s, parent, &groups)
    {
        return Err(bad(cmd, "a new folder cannot contain an ancestor of its parent"));
    }
    let nested = group_feature_ids(&s.doc, &groups);
    features.retain(|feature| !nested.contains(feature));
    let mut next = (*s.doc).clone();
    for group in &mut next.timeline_groups {
        group.features.retain(|feature| !features.contains(feature));
        if groups.contains(&group.id) {
            group.parent = Some(id);
        }
    }
    next.timeline_groups.push(solidcraft_doc::TimelineGroup { id, name: name.clone(), parent, features });
    sort_group_features(&mut next);
    require_contiguous_groups(&next, cmd)?;
    *s.doc_mut() = next;
    Ok(json!({"group": id, "name": name, "parent": parent, "groups": groups}))
}

fn ungroup(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.ungroup";
    let ids = match p.get("groups") {
        Some(Value::Array(values)) if !values.is_empty() && values.len() <= 10_000 => {
            let mut ids = Vec::new();
            for value in values {
                let id = value.as_u64().ok_or_else(|| bad(cmd, "`groups` must contain timeline folder ids"))?;
                if !s.doc.timeline_groups.iter().any(|group| group.id == id) {
                    return Err(bad(cmd, format!("no timeline folder {id}")));
                }
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            ids
        }
        Some(Value::Array(_)) => return Err(bad(cmd, "`groups` must list 1…10000 timeline folder ids")),
        Some(_) => return Err(bad(cmd, "`groups` must be a list of timeline folder ids")),
        None => vec![timeline_group_id(s, p, cmd)?],
    };
    let parents: std::collections::BTreeMap<u64, Option<u64>> =
        s.doc.timeline_groups.iter().filter(|group| ids.contains(&group.id)).map(|group| (group.id, group.parent)).collect();
    let surviving_parent = |mut parent: Option<u64>| {
        let mut steps = 0usize;
        while let Some(id) = parent.filter(|id| ids.contains(id)) {
            parent = parents.get(&id).copied().flatten();
            steps += 1;
            if steps > parents.len() {
                return None;
            }
        }
        parent
    };
    let mut next = (*s.doc).clone();
    let moved_features: Vec<(Option<u64>, Vec<u64>)> = next
        .timeline_groups
        .iter()
        .filter(|group| ids.contains(&group.id))
        .map(|group| (surviving_parent(group.parent), group.features.clone()))
        .collect();
    for group in &mut next.timeline_groups {
        if ids.contains(&group.id) {
            continue;
        }
        group.parent = surviving_parent(group.parent);
        for (_, features) in moved_features.iter().filter(|(parent, _)| *parent == Some(group.id)) {
            group.features.extend(features.iter().copied());
        }
    }
    next.timeline_groups.retain(|group| !ids.contains(&group.id));
    sort_group_features(&mut next);
    require_contiguous_groups(&next, cmd)?;
    *s.doc_mut() = next;
    Ok(json!({"groups": ids}))
}

fn rename_group(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.rename_group";
    let id = timeline_group_id(s, p, cmd)?;
    let name = str_(p, "name")
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 128)
        .ok_or_else(|| bad(cmd, "`name` must be 1…128 characters"))?;
    if let Some(group) = s.doc_mut().timeline_groups.iter_mut().find(|group| group.id == id) {
        group.name = name.to_string();
    }
    Ok(json!({"group": id, "name": name}))
}

fn move_to_group(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.move_to_group";
    let features = group_features(s, p, false, cmd)?;
    let groups = group_ids(s, p, false, cmd)?;
    if features.is_empty() && groups.is_empty() {
        return Err(bad(cmd, "list at least one feature or folder"));
    }
    let target = match p.get("group") {
        Some(Value::Null) | None => None,
        Some(_) => Some(timeline_group_id(s, p, cmd)?),
    };
    if let Some(target) = target
        && group_move_would_cycle(s, target, &groups)
    {
        return Err(bad(cmd, "a folder cannot be moved into itself or one of its descendants"));
    }
    let mut next = (*s.doc).clone();
    for group in &mut next.timeline_groups {
        group.features.retain(|feature| !features.contains(feature));
        if Some(group.id) == target {
            group.features.extend(features.iter().copied());
        }
    }
    for group in &mut next.timeline_groups {
        if groups.contains(&group.id) {
            group.parent = target;
        }
    }
    sort_group_features(&mut next);
    require_contiguous_groups(&next, cmd)?;
    *s.doc_mut() = next;
    Ok(json!({"features": features, "groups": groups, "group": target}))
}

fn move_many(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.move_many";
    let mut features = group_features(s, p, false, cmd)?;
    let groups = group_ids(s, p, false, cmd)?;
    if features.is_empty() && groups.is_empty() {
        return Err(bad(cmd, "list at least one feature or folder"));
    }
    let target = match p.get("group") {
        Some(Value::Null) | None => None,
        Some(_) => Some(timeline_group_id(s, p, cmd)?),
    };
    if let Some(target) = target
        && group_move_would_cycle(s, target, &groups)
    {
        return Err(bad(cmd, "a folder cannot be moved into itself or one of its descendants"));
    }
    let nested = group_feature_ids(&s.doc, &groups);
    features.retain(|feature| !nested.contains(feature));
    let moving: Vec<u64> =
        s.doc.features.iter().filter(|feature| nested.contains(&feature.id) || features.contains(&feature.id)).map(|feature| feature.id).collect();
    let mut next = (*s.doc).clone();
    for group in &mut next.timeline_groups {
        group.features.retain(|feature| !features.contains(feature));
        if Some(group.id) == target {
            group.features.extend(features.iter().copied());
        }
        if groups.contains(&group.id) {
            group.parent = target;
        }
    }
    sort_group_features(&mut next);
    let slot = num(p, "position").map(|x| x.max(0.0) as usize).ok_or_else(|| bad(cmd, "`position` must be a number"))?;
    let (next, ordered) = reorder_features(s, next, &moving, slot, cmd)?;
    if next != *s.doc {
        *s.doc_mut() = next;
    }
    Ok(json!({"features": ordered, "groups": groups, "group": target, "position": slot}))
}

fn rollback(s: &mut Session, p: &Value) -> Result<Value> {
    let n = s.doc.features.len();
    let pos = match p.get("feature") {
        Some(f) => {
            let id = feature_id(s, Some(f), "timeline.roll_to")?;
            s.doc.feature_index(id).map(|i| i + 1)
        }
        None => num(p, "position").map(|x| x.max(0.0) as usize),
    };
    s.doc_mut().marker = match pos {
        Some(k) if k < n => Some(k),
        _ => None,
    };
    s.selection.clear();
    Ok(json!({"marker": s.doc.marker.unwrap_or(n), "features": n}))
}

fn suppress(s: &mut Session, p: &Value) -> Result<Value> {
    let id = feature_id(s, p.get("feature"), "timeline.suppress")?;
    let want = bool_(p, "suppressed");
    let f = s.doc_mut().feature_mut(id).ok_or_else(|| EngineError::Other("feature".into()))?;
    f.suppressed = want.unwrap_or(!f.suppressed);
    Ok(json!({"feature": id, "suppressed": f.suppressed}))
}

fn merge(dst: &mut Value, src: &Value, depth: usize) {
    if depth > 16 {
        return;
    }
    match (dst, src) {
        (Value::Object(d), Value::Object(s)) => {
            for (k, v) in s {
                match d.get_mut(k) {
                    Some(slot) if slot.is_object() && v.is_object() => merge(slot, v, depth + 1),
                    _ => {
                        d.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (d, s) => *d = s.clone(),
    }
}

/// One step of a path into a feature's JSON definition.
#[derive(Clone)]
enum Step {
    Key(String),
    Index(usize),
}

fn path_text(path: &[Step]) -> String {
    let mut t = String::new();
    for s in path {
        match s {
            Step::Key(k) if t.is_empty() => t.push_str(k),
            Step::Key(k) => t.push_str(&format!(".{k}")),
            Step::Index(i) => t.push_str(&format!("[{i}]")),
        }
    }
    t
}

fn at_path<'a>(v: &'a Value, path: &[Step]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, s| match s {
        Step::Key(k) => v.get(k),
        Step::Index(i) => v.get(i),
    })
}

/// Does the stored feature read the field at `path` of `merged` (a feature's JSON with the edit
/// merged in)? serde skips a field it doesn't know whatever its value, but no field type takes a
/// flag, a text and nested lists alike.
fn reads_field(merged: &Value, path: &[Step]) -> bool {
    let Some((last, parent)) = path.split_last() else { return true };
    [json!(true), json!("\u{1}"), json!([[[]]])].into_iter().any(|probe| {
        let mut v = merged.clone();
        let slot = parent.iter().try_fold(&mut v, |v, s| match s {
            Step::Key(k) => v.get_mut(k),
            Step::Index(i) => v.get_mut(*i),
        });
        match (slot, last) {
            (Some(Value::Object(o)), Step::Key(k)) => {
                o.insert(k.clone(), probe);
            }
            (Some(Value::Array(a)), Step::Index(i)) => match a.get_mut(*i) {
                Some(x) => *x = probe,
                None => return false,
            },
            _ => return false,
        }
        serde_json::from_value::<Feature>(v).is_err()
    })
}

/// The first field of `set` that the feature's definition doesn't have. A field the definition
/// shows before or after the edit is known; one it leaves out (an unset option, a value at its
/// default) is known when the stored feature reads it. Setting an absent field to `null`
/// changes nothing either way and passes.
fn unknown_field(
    set: &Value,
    before: Option<&Value>,
    after: Option<&Value>,
    merged: &Value,
    path: &mut Vec<Step>,
    depth: usize,
) -> Option<Vec<Step>> {
    if depth > 16 {
        return None;
    }
    let check = |step: Step, v: &Value, path: &mut Vec<Step>| {
        let one = [step];
        let (b, a) = (before.and_then(|x| at_path(x, &one)), after.and_then(|x| at_path(x, &one)));
        let [step] = one;
        path.push(step);
        let found = if b.is_none() && a.is_none() && !v.is_null() && !reads_field(merged, path) {
            Some(path.clone())
        } else {
            unknown_field(v, b, a, merged, path, depth + 1)
        };
        path.pop();
        found
    };
    match set {
        Value::Object(o) => o.iter().find_map(|(k, v)| check(Step::Key(k.clone()), v, path)),
        Value::Array(items) => items.iter().enumerate().take(10_000).find_map(|(i, v)| check(Step::Index(i), v, path)),
        _ => None,
    }
}

/// "unknown field `offset`; ConstructionPlane has `plane.base.name`, `plane.distance`…": the
/// editable fields where it was given, as paths.
fn unknown_field_message(field: &[Step], def: &Value, type_name: &str) -> String {
    fn leaves(v: &Value, prefix: &str, top: bool, depth: usize, out: &mut Vec<String>) {
        match v {
            Value::Object(o) if depth < 4 && !o.is_empty() => {
                for (k, x) in o {
                    if top && NOT_EDITABLE.contains(&k.as_str()) {
                        continue;
                    }
                    let at = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                    leaves(x, &at, false, depth + 1, out);
                }
            }
            _ if !prefix.is_empty() => out.push(format!("`{prefix}`")),
            _ => {}
        }
    }
    let parent = field.split_last().map(|(_, p)| p).unwrap_or_default();
    let mut known = Vec::new();
    if let Some(at) = at_path(def, parent) {
        leaves(at, &path_text(parent), parent.is_empty(), 0, &mut known);
    }
    const SHOWN: usize = 40;
    let more = known.len().saturating_sub(SHOWN);
    known.truncate(SHOWN);
    let mut list = known.join(", ");
    if more > 0 {
        list.push_str(&format!(" and {more} more"));
    }
    let field = path_text(field);
    if list.is_empty() {
        return format!("unknown field `{field}`; timeline.inspect lists {type_name}'s fields");
    }
    format!("unknown field `{field}`; {type_name} has {list} (timeline.inspect shows them with their values)")
}

/// `profiles` (also a loft section's) given as the feature commands take it (`"all"`,
/// `[index]`, `[{curves}]`, `[{point}]`...) becomes the stored form; the stored form passes.
fn command_profiles(set: &Value, cmd: &str) -> Result<Value> {
    let conv = |v: &mut Value| -> Result<()> {
        if let Some(pv) = v.get("profiles").filter(|x| x.get("type").is_none()) {
            let sel = super::features::profiles(&json!({"profiles": pv}), cmd)?;
            let sv = serde_json::to_value(sel).map_err(|e| EngineError::Other(e.to_string()))?;
            if let Some(o) = v.as_object_mut() {
                o.insert("profiles".into(), sv);
            }
        }
        Ok(())
    };
    let mut set = set.clone();
    conv(&mut set)?;
    if let Some(secs) = set.get_mut("sections").and_then(Value::as_array_mut) {
        for sec in secs.iter_mut().take(10_000) {
            conv(sec)?;
        }
    }
    Ok(set)
}

/// Fields of a stored feature left out of its definition: bookkeeping timeline.edit recomputes, and
/// the name and suppression, which timeline.rename and timeline.suppress change.
const NOT_EDITABLE: &[&str] = &["id", "type", "name", "suppressed", "param_names", "edge_refs", "edge_names", "face_names"];

fn inspect_feature(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.inspect";
    let id = feature_id(s, p.get("feature"), cmd)?;
    let f = s.doc.feature(id).ok_or_else(|| bad(cmd, "feature"))?;
    let mut def = serde_json::to_value(f).map_err(|e| EngineError::Other(e.to_string()))?;
    let mut omitted = Vec::new();
    if let Some(o) = def.as_object_mut() {
        for k in NOT_EDITABLE {
            o.remove(*k);
        }
        // Large payloads that timeline.edit cannot change.
        let heavy: &[(&str, &str)] = match &f.kind {
            FeatureKind::Sketch { .. } => &[("sketch", "the sketch geometry (sketch.inspect; edit it with the sketch commands)")],
            FeatureKind::Import { .. } => &[("step", "the imported STEP text"), ("components", "the imported product tree")],
            FeatureKind::MeshImport { .. } => &[("meshes", "the imported meshes")],
            _ => &[],
        };
        for (k, why) in heavy {
            if o.remove(*k).is_some() {
                omitted.push(json!({"field": k, "what": why}));
            }
        }
    }
    let inputs: Vec<Value> = f
        .kind
        .inputs()
        .into_iter()
        .enumerate()
        .map(|(i, (label, e, k))| {
            json!({"label": label, "param": f.param_names.get(i).filter(|n| !n.is_empty()), "expression": e, "unit": k.default_unit()})
        })
        .collect();
    let marker = s.doc.marker.unwrap_or(usize::MAX);
    let rolled_back = s.doc.features.iter().position(|x| x.id == id).is_some_and(|i| i >= marker);
    let r = s.model.result(id);
    Ok(json!({
        "id": f.id, "name": f.name, "type": f.kind.type_name(), "suppressed": f.suppressed, "rolled_back": rolled_back,
        "definition": def, "omitted": omitted, "inputs": inputs,
        "error": r.and_then(|r| r.error.clone()), "warning": r.and_then(|r| r.warning.clone()),
        "boolean_diagnostics": r.map(|r| &r.boolean_diagnostics),
    }))
}

fn edit_feature(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.edit";
    let id = feature_id(s, p.get("feature"), cmd)?;
    let set = p.get("set").filter(|v| v.is_object()).ok_or_else(|| bad(cmd, "`set` must be an object of fields"))?;
    let f = s.doc.feature(id).cloned().ok_or_else(|| bad(cmd, "feature"))?;
    let mut v = serde_json::to_value(&f).map_err(|e| EngineError::Other(e.to_string()))?;
    let set = &command_profiles(set, cmd)?;
    merge(&mut v, set, 0);
    let mut nf: Feature = serde_json::from_value(v.clone()).map_err(|e| bad(cmd, format!("invalid feature: {e}")))?;
    nf.id = f.id;
    if let (FeatureKind::Extrude { extent: old, .. }, FeatureKind::Extrude { extent: new, .. }) = (&f.kind, &mut nf.kind)
        && old.direction != solidcraft_doc::Direction::Symmetric
        && new.direction == solidcraft_doc::Direction::Symmetric
        && set.get("extent").and_then(|e| e.get("convention")).is_none()
    {
        new.convention = solidcraft_doc::ExtrudeConvention::TotalDistance;
    }
    if std::mem::discriminant(&nf.kind) != std::mem::discriminant(&f.kind) {
        return Err(bad(cmd, "the feature type cannot change"));
    }
    // serde drops fields the definition doesn't have without a word: refuse them, or a typo
    // would report success and change nothing.
    let after = serde_json::to_value(&nf).map_err(|e| EngineError::Other(e.to_string()))?;
    let before = serde_json::to_value(&f).map_err(|e| EngineError::Other(e.to_string()))?;
    if let Some(field) = unknown_field(set, Some(&before), Some(&after), &v, &mut Vec::new(), 0) {
        return Err(bad(cmd, unknown_field_message(&field, &before, f.kind.type_name())));
    }
    if let FeatureKind::BodyTransfer { bodies, from, to, transform } = &nf.kind {
        solidcraft_doc::validate_body_transfer(&s.doc, &solidcraft_doc::plain(bodies), *from, *to, transform)?;
        nf.component = *to;
    }
    if let (FeatureKind::Sketch { sketch: a, .. }, FeatureKind::Sketch { sketch: b, .. }) = (&nf.kind, &f.kind)
        && a != b
    {
        return Err(bad(cmd, "edit sketch geometry with the sketch commands"));
    }
    // New edges for a blend: remember where they sit now (in the model before the feature).
    if let (
        FeatureKind::Fillet { edges: a, body, .. } | FeatureKind::Chamfer { edges: a, body, .. },
        FeatureKind::Fillet { edges: b, .. } | FeatureKind::Chamfer { edges: b, .. },
    ) = (&nf.kind, &f.kind)
        && a != b
    {
        let before = s.model.state_before(id);
        nf.edge_refs = solidcraft_doc::edge_refs(&before, body, &solidcraft_doc::plain(a));
        nf.edge_names = solidcraft_doc::edge_names_for(&before, body, &solidcraft_doc::plain(a));
    }
    // New or reordered flange edges: their names now, so each edge's own angle and height stay
    // with the edge they were given for.
    if let (FeatureKind::SheetFlange { edges: a, .. }, FeatureKind::SheetFlange { edges: b, .. }) = (&nf.kind, &f.kind) {
        let points = |e: &[solidcraft_doc::FlangeEdge]| e.iter().map(|x| x.point).collect::<Vec<_>>();
        if points(a) != points(b) {
            nf.edge_names = solidcraft_doc::sheet::sheet_edge_names(&s.model.state_before(id), &solidcraft_doc::plain(&points(a)));
        }
    }
    // New bend edges for a conversion: the names of the body's edges there now.
    if let (FeatureKind::SheetConvert { bends: a, body, .. }, FeatureKind::SheetConvert { bends: b, body: was, .. }) = (&nf.kind, &f.kind)
        && (a != b || body != was)
    {
        nf.edge_names = solidcraft_doc::sheet::bend_edge_names(&s.model.state_before(id), body, &solidcraft_doc::plain(a));
    }
    // Profiles picked by index or point are kept by their curves, as when the feature was made.
    if nf.kind.clone().profile_sels_mut() != f.kind.clone().profile_sels_mut() {
        super::features::stable_profiles(s, &mut nf.kind)?;
    }
    // New face points: their faces' names now.
    if nf.kind.face_points() != f.kind.face_points() {
        nf.face_names = solidcraft_doc::face_point_names(&s.doc, &s.model.state_before(id), &nf.kind, nf.component);
    }
    if let Some(slot) = s.doc_mut().feature_mut(id) {
        *slot = nf;
    }
    s.refresh();
    if let Some(e) = s.model.result(id).and_then(|r| r.error.clone()) {
        return Err(EngineError::Other(e));
    }
    Ok(json!({"feature": id, "recomputed": s.model.last_recomputed, "restored": s.model.last_restored}))
}

fn select_set(s: &mut Session, p: &Value) -> Result<Value> {
    let items = p.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    if items.len() > 10_000 {
        return Err(bad("select.set", "too many items"));
    }
    let mut sel: Vec<Sel> = Vec::new();
    for it in items {
        let one: Sel = serde_json::from_value(it).map_err(|e| bad("select.set", e.to_string()))?;
        if let Sel::Occurrence { path } = &one {
            s.occurrence_bodies(path, true).map_err(|e| bad("select.set", e.to_string()))?;
        }
        sel.push(one);
    }
    if !bool_(p, "add").unwrap_or(false) {
        s.selection.clear();
    }
    for x in sel {
        if !s.selection.contains(&x) {
            s.selection.push(x);
        }
    }
    s.revision += 1;
    Ok(json!({"selected": s.selection.len()}))
}

fn select_clear(s: &mut Session, _p: &Value) -> Result<Value> {
    s.selection.clear();
    s.revision += 1;
    Ok(json!({"selected": 0}))
}

/// Features that worked in `before` and fail in `after`: (name, error).
fn newly_broken(before: &solidcraft_doc::Model, after: &solidcraft_doc::Model) -> Vec<(String, String)> {
    after
        .results
        .iter()
        .filter_map(|r| {
            let e = r.error.clone()?;
            let was_ok = before.result(r.id).is_some_and(|b| b.error.is_none() && !b.skipped);
            was_ok.then(|| (r.name.clone(), e))
        })
        .collect()
}

fn reorder(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.reorder";
    let id = feature_id(s, p.get("feature"), cmd)?;
    let n = s.doc.features.len();
    let to = num(p, "position").map(|x| x.max(0.0) as usize).ok_or_else(|| bad(cmd, "`position` must be a number"))?.min(n.saturating_sub(1));
    let from = s.doc.feature_index(id).ok_or_else(|| bad(cmd, "feature"))?;
    if from == to {
        return Ok(json!({"feature": id, "position": to}));
    }
    let mut next = (*s.doc).clone();
    let f = next.features.remove(from);
    let name = f.name.clone();
    next.features.insert(to, f);
    sort_group_features(&mut next);
    require_contiguous_groups(&next, cmd)?;
    // Everything that evaluated before must still evaluate.
    let mut after = solidcraft_doc::Model::new();
    after.evaluate(&next);
    let broken = newly_broken(&s.model, &after);
    if !broken.is_empty() {
        let list: Vec<String> = broken.iter().map(|(n, e)| format!("{n} ({e})")).collect();
        return Err(bad(cmd, format!("moving {name} there breaks {}", list.join("; "))));
    }
    *s.doc_mut() = next;
    Ok(json!({"feature": id, "position": to}))
}

fn reorder_many(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.reorder_many";
    let ids = group_features(s, p, true, cmd)?;
    let slot = num(p, "position").map(|x| x.max(0.0) as usize).ok_or_else(|| bad(cmd, "`position` must be a number"))?;
    let (next, ordered) = reorder_features(s, (*s.doc).clone(), &ids, slot, cmd)?;
    if next != *s.doc {
        *s.doc_mut() = next;
    }
    Ok(json!({"features": ordered, "position": slot.min(s.doc.features.len())}))
}

fn reorder_features(
    s: &Session,
    mut next: solidcraft_doc::Document,
    ids: &[u64],
    slot: usize,
    cmd: &str,
) -> Result<(solidcraft_doc::Document, Vec<u64>)> {
    let n = next.features.len();
    let slot = slot.min(n);
    let selected: std::collections::BTreeSet<u64> = ids.iter().copied().collect();
    let ordered: Vec<u64> = next.features.iter().filter(|feature| selected.contains(&feature.id)).map(|feature| feature.id).collect();
    let before_slot = next.features.iter().take(slot).filter(|feature| selected.contains(&feature.id)).count();
    let insert_at = slot.saturating_sub(before_slot).min(n.saturating_sub(ordered.len()));

    let mut moving = Vec::new();
    next.features.retain(|feature| {
        if selected.contains(&feature.id) {
            moving.push(feature.clone());
            false
        } else {
            true
        }
    });
    let unchanged = s.doc.features.iter().map(|feature| feature.id).eq(next.features[..insert_at]
        .iter()
        .chain(moving.iter())
        .chain(next.features[insert_at..].iter())
        .map(|feature| feature.id));
    next.features.splice(insert_at..insert_at, moving);
    sort_group_features(&mut next);
    require_contiguous_groups(&next, cmd)?;
    if !unchanged {
        let mut after = solidcraft_doc::Model::new();
        after.evaluate(&next);
        let broken = newly_broken(&s.model, &after);
        if !broken.is_empty() {
            let list: Vec<String> = broken.iter().map(|(name, error)| format!("{name} ({error})")).collect();
            return Err(bad(cmd, format!("moving the selected features there breaks {}", list.join("; "))));
        }
    }
    Ok((next, ordered))
}

fn input_keys(kind: &FeatureKind) -> Vec<(String, solidcraft_doc::expr::Kind)> {
    let mut keys: Vec<_> = kind.inputs().into_iter().map(|(label, _, unit)| (label.to_string(), unit)).collect();
    if let FeatureKind::Fillet { style, .. } = kind {
        if let Some((key, _)) = keys.first_mut() {
            *key = "fillet.radius".into();
        }
        match style {
            solidcraft_doc::FilletStyle::Variable { mid, .. } => {
                for ((key, _), (position, _)) in keys.iter_mut().skip(2).zip(mid) {
                    *key = format!("fillet.mid.{}", position.to_bits());
                }
            }
            solidcraft_doc::FilletStyle::Spline { .. } => {
                for (i, (key, _)) in keys.iter_mut().enumerate().skip(1) {
                    *key = format!("fillet.control.{i}");
                }
            }
            solidcraft_doc::FilletStyle::Constant | solidcraft_doc::FilletStyle::Chord | solidcraft_doc::FilletStyle::Curvature => {}
        }
    }
    keys
}

fn redefine(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.redefine";
    let id = feature_id(s, p.get("feature"), cmd)?;
    let cid = str_(p, "command").ok_or_else(|| bad(cmd, "`command` must name the feature's command"))?;
    let spec = super::find_command(cid).ok_or_else(|| bad(cmd, format!("no command `{cid}`")))?;
    // Only modeling constructors may run here. In particular, a scratch session does not
    // isolate filesystem writes performed by export commands.
    let constructor = super::features::COMMANDS
        .iter()
        .chain(super::features_more::COMMANDS)
        .chain(super::construct::COMMANDS)
        .chain(super::surface::COMMANDS)
        .any(|candidate| candidate.id == spec.id)
        || matches!(
            spec.id,
            "sheet.flange"
                | "sheet.hem"
                | "sheet.perforate"
                | "sheet.fold"
                | "sheet.unfold"
                | "sheet.refold"
                | "sheet.convert"
                | "plastic.boss"
                | "plastic.lip"
                | "plastic.snap_fit"
                | "plastic.rest"
        );
    if !constructor {
        return Err(bad(cmd, format!("`{cid}` does not make a feature")));
    }
    let mut params = p.get("params").cloned().unwrap_or_else(|| json!({}));
    let idx = s.doc.feature_index(id).ok_or_else(|| bad(cmd, "feature"))?;
    let old = s.doc.feature(id).cloned().ok_or_else(|| bad(cmd, "feature"))?;
    if let FeatureKind::Extrude { extent, .. } | FeatureKind::SurfaceExtrude { extent, .. } = &old.kind
        && matches!(cid, "solid.extrude" | "surface.extrude")
        && let Some(parameters) = params.as_object_mut()
    {
        let convention = if cid == "surface.extrude" || extent.direction == solidcraft_doc::Direction::Symmetric {
            extent.convention
        } else {
            solidcraft_doc::ExtrudeConvention::TotalDistance
        };
        parameters.entry("convention").or_insert_with(|| json!(convention));
    }
    if let FeatureKind::Revolve { convention, .. } = &old.kind
        && cid == "solid.revolve"
        && let Some(parameters) = params.as_object_mut()
    {
        parameters.entry("convention").or_insert_with(|| json!(convention));
    }
    if !old.participants.is_empty()
        && let Some(parameters) = params.as_object_mut()
    {
        parameters.entry("participants").or_insert_with(|| json!(old.participants));
    }
    // Run the constructor on a scratch session: failed edits must not disturb sketch mode,
    // selections, component authoring state or the original feature's definition.
    let mut trial = s.scratch();
    trial.active_component = old.component;
    if let FeatureKind::Emboss { face: Some(face), .. } = &old.kind {
        trial.active_occurrence_path = (!face.author_path.is_empty()).then(|| face.author_path.clone());
    }
    trial.active_sketch = None;
    trial.selection = s.selection.clone();
    // Keep the original identity as a suppressed placeholder. The constructor and
    // any ownership boundary it adds go AFTER this slot, so discarding its temporary
    // feature leaves auxiliary history after the rewritten original.
    if let Some(slot) = trial.doc_mut().feature_mut(id) {
        slot.suppressed = true;
    }
    trial.doc_mut().marker = Some(idx + 1);
    trial.refresh();
    let r = spec.run_nested(&mut trial, &params)?;
    let made = r.get("feature").and_then(Value::as_u64).ok_or_else(|| bad(cmd, format!("`{cid}` did not make a feature")))?;
    let mut new = trial.doc.feature(made).cloned().ok_or_else(|| bad(cmd, format!("`{cid}` did not make a feature")))?;
    if let (FeatureKind::Emboss { face: Some(before), .. }, FeatureKind::Emboss { face: Some(after), .. }) = (&old.kind, &mut new.kind)
        && before.body == after.body
        && before.path == after.path
        && before.author_path == after.author_path
        && before.at.dist(after.at.0) < 1e-8
    {
        *after = before.clone();
    }
    if std::mem::discriminant(&new.kind) != std::mem::discriminant(&old.kind) {
        return Err(bad(cmd, format!("{} is not a {} feature", old.name, cid)));
    }
    // Optional inputs can move other inputs' positions (e.g. Distance 2 goes before Taper).
    // Keep each existing model parameter attached to its semantic input, rather than its
    // old index. Constructor-assigned names for new inputs cannot reuse the old dimensions.
    let mut old_inputs: Vec<_> = input_keys(&old.kind).into_iter().zip(&old.param_names).collect();
    let param_names: Vec<String> = input_keys(&new.kind)
        .iter()
        .enumerate()
        .map(|(i, (label, kind))| {
            if let Some(k) = old_inputs.iter().position(|((old_label, old_kind), _)| old_label == label && old_kind == kind) {
                old_inputs.remove(k).1.clone()
            } else {
                new.param_names.get(i).cloned().unwrap_or_default()
            }
        })
        .collect();
    trial.doc_mut().features.retain(|f| f.id != made);
    let added = trial.doc.features.len().saturating_sub(s.doc.features.len());
    trial.doc_mut().marker = s.doc.marker.map(|marker| if marker > idx { marker.saturating_add(added) } else { marker });
    if made == s.doc.next_id && trial.doc.next_id == made.saturating_add(1) {
        trial.doc_mut().next_id = s.doc.next_id;
    }
    if let Some(slot) = trial.doc_mut().feature_mut(id) {
        slot.suppressed = old.suppressed;
        slot.kind = new.kind;
        slot.edge_refs = new.edge_refs;
        slot.edge_names = new.edge_names;
        slot.face_names = new.face_names;
        slot.param_names = param_names;
        if params.get("participants").is_some() {
            slot.participants = new.participants;
        }
    }
    // New optional inputs need model parameter names, while existing dimensions keep theirs.
    trial.doc_mut().name_feature_inputs(id);
    trial.refresh();
    if let Some(e) = trial.model.result(id).and_then(|r| r.error.clone()) {
        return Err(EngineError::Other(e));
    }
    let original_ids: std::collections::BTreeSet<u64> = s.doc.features.iter().map(|feature| feature.id).collect();
    let added_ids: std::collections::BTreeSet<u64> =
        trial.doc.features.iter().filter(|feature| !original_ids.contains(&feature.id)).map(|feature| feature.id).collect();
    let adds_transfer =
        trial.doc.features.iter().any(|feature| added_ids.contains(&feature.id) && matches!(feature.kind, FeatureKind::BodyTransfer { .. }));
    let baseline = if adds_transfer && s.doc.marker.is_some() {
        let mut baseline = s.scratch();
        baseline.doc_mut().marker = None;
        baseline.refresh();
        Some(baseline)
    } else {
        None
    };
    let proof = if (adds_transfer && trial.doc.marker.is_some()) || added_ids.iter().any(|id| trial.model.result(*id).is_none()) {
        let mut proof = trial.scratch();
        proof.doc_mut().marker = None;
        proof.refresh();
        Some(proof)
    } else {
        None
    };
    let validated = proof.as_ref().map(|proof| &proof.model).unwrap_or(&trial.model);
    let original_model = baseline.as_ref().map(|baseline| &baseline.model).unwrap_or(&s.model);
    for feature in &trial.doc.features {
        let auxiliary = added_ids.contains(&feature.id);
        let protected_transfer = matches!(feature.kind, FeatureKind::BodyTransfer { .. })
            && original_model.result(feature.id).is_some_and(|result| !result.skipped && result.error.is_none());
        if (auxiliary || protected_transfer)
            && let Some(error) = validated.result(feature.id).and_then(|result| result.error.as_ref())
        {
            return Err(bad(cmd, format!("{}: {error}", feature.name)));
        }
    }
    s.pending_moves = trial.pending_moves;
    s.doc = trial.doc;
    s.refresh();
    if let Some(e) = s.model.result(id).and_then(|r| r.error.clone()) {
        return Err(EngineError::Other(e));
    }
    Ok(json!({"feature": id, "recomputed": s.model.last_recomputed, "restored": s.model.last_restored}))
}

fn dependents(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.dependents";
    let id = feature_id(s, p.get("feature"), cmd)?;
    let mut next = (*s.doc).clone();
    let gone = next.delete_feature(id)?;
    let mut after = solidcraft_doc::Model::new();
    after.evaluate(&next);
    let names = |ids: &[u64]| -> Vec<String> { ids.iter().filter(|x| **x != id).filter_map(|x| s.doc.feature(*x).map(|f| f.name.clone())).collect() };
    let broken: Vec<String> = newly_broken(&s.model, &after).into_iter().map(|(n, _)| n).collect();
    Ok(json!({"deleted_with_it": names(&gone), "would_fail": broken}))
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
