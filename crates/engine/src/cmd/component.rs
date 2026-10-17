//! Components and occurrences: the design as a tree (see `solidcraft_doc` assembly docs).
//! New sketches and features go into the active component, authored in its frame; bodies
//! belong to their feature's component unless moved; occurrences place components (instances
//! share contents, "paste new" copies them) and can be grounded and moved.

use serde_json::{Value, json};
use solidcraft_doc::{FeatureKind, Mat, Occurrence};
use solidcraft_geom::Vec3;

use super::CommandSpec;
use crate::params::{bad, bool_, str_, string_list, vec3};
use crate::{Result, Session};

#[path = "component_properties.rs"]
pub(super) mod properties;
#[path = "component_transfer.rs"]
mod transfer;

#[cfg(test)]
#[path = "occurrence_tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod occurrence_tests;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("component.create", "New Component", new_component)
        .at("SOLID", "ASSEMBLE")
        .icon("component")
        .params("name?, parent?: component id or name (default: the active one); parent_path?: exact parent placement; activate?: bool (default true)"),
    CommandSpec::new("component.from_bodies", "Create Components from Bodies", from_bodies)
        .icon("component")
        .params("bodies: [names] → one new component per body, named after it"),
    CommandSpec::new("component.activate", "Activate Component", activate).icon("activate").noundo().params("component: id or name (0 or \"root\" for the design); occurrence?, path?: exact placement used to author shared geometry"),
    CommandSpec::new("component.list", "List Components", list).icon("list").noundo().params(
        "compact?: bool (counts of bodies, features, sketches and construction planes instead of their names; occurrences leave out \
         identity transforms; children as occurrence ids); bodies?: bool (with compact: body names, not their count)",
    ),
    CommandSpec::new("component.properties", "Component Physical Properties", properties::component)
        .icon("measure").noundo()
        .params("component?: id or name (default active); descendants?: bool (default true). Measures the first placement, or all placements for root, including pending moves; material-aware mass and center of mass in world coordinates"),
    CommandSpec::new("occurrence.properties", "Occurrence Physical Properties", properties::occurrence)
        .icon("measure").noundo()
        .params("occurrence: id or name; path?: [occurrence ids] (required if nested under multiple instances); descendants?: bool (default true). Physical properties of this placement in world coordinates, including pending moves"),
    CommandSpec::new("component.rename", "Rename Component", rename).icon("rename").params("component, name"),
    CommandSpec::new("component.delete", "Delete Component", delete).icon("delete").params("component: deletes it, its subcomponents, occurrences and features"),
    CommandSpec::new("component.unlock", "Unlock Contents", unlock).icon("rigid_group").params(
        "component: removes the rigid joints that hold its contents together (an imported assembly's rigid group), so its parts \
         move and take joints one by one → joints (the ids removed)",
    ),
    CommandSpec::new("component.move_bodies", "Move Bodies to Component", move_bodies).icon("move").params("bodies: [names], component"),
    CommandSpec::new("component.move_sketches", "Move Sketches to Component", move_sketches).icon("move").params("sketches: [ids or names], component"),
    CommandSpec::new("occurrence.copy", "Paste (Instance)", copy_occurrence).icon("copy")
        .params("component (or occurrence): what to place again; path?: validates selected occurrence; parent?: component (default: the same parent); translate?: [x,y,z]. Copies the selected effective transform, including pending moves"),
    CommandSpec::new("component.paste_new", "Paste New", paste_new).icon("paste").params("component: independently copy its nested definition graph and history, materials and styles; occurrence?, path?: selected source placement; translate?: [x,y,z]. Reassigned external bodies become base geometry; retained external links and omitted boundary joints are reported"),
    CommandSpec::new("occurrence.move", "Move Occurrence", move_occurrence).icon("move")
        .params("occurrence; path?: exact placement; space?: local|world (default local); translate?: [x,y,z]; axis?: [x,y,z], angle?: radians or \"30 deg\", origin?: [x,y,z]; capture?: bool (default true; false keeps it pending until Capture Position). Nested occurrence definitions remain shared"),
    CommandSpec::new("occurrence.ground", "Ground", ground).icon("ground").params("occurrence; grounded?: bool (default toggles)"),
    CommandSpec::new("component.capture_position", "Capture Position", capture).at("SOLID", "POSITION").icon("capture").params("keeps pending occurrence moves"),
    CommandSpec::new("component.revert_position", "Revert Position", revert).at("SOLID", "POSITION").icon("revert").noundo().params("occurrences?: [ids] (default all); drops selected pending occurrence moves"),
];

fn key(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn component(s: &Session, p: &Value, k: &str, cmd: &str) -> Result<u64> {
    let kk = key(p.get(k)).ok_or_else(|| bad(cmd, format!("`{k}` must be a component id or name")))?;
    s.doc.find_component(&kk).ok_or_else(|| bad(cmd, format!("no component `{kk}`")))
}

fn occurrence(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    let kk = key(p.get("occurrence")).ok_or_else(|| bad(cmd, "`occurrence` must be an id or name"))?;
    s.doc.occurrences.iter().find(|o| o.id.to_string() == kk || o.name == kk).map(|o| o.id).ok_or_else(|| bad(cmd, format!("no occurrence `{kk}`")))
}

fn translation(t: Vec3) -> Mat {
    solidcraft_doc::rigid(t, Vec3::ZERO, Vec3::Z, 0.0)
}

fn move_vector(p: &Value, key: &str, default: Vec3, coordinate: bool, cmd: &str) -> Result<Vec3> {
    let v = match p.get(key) {
        None => default,
        Some(value) => vec3(value).ok_or_else(|| bad(cmd, format!("{key} must contain three finite numbers")))?,
    };
    if !v.is_finite() || (coordinate && [v.x, v.y, v.z].iter().any(|x| x.abs() > 1e9)) {
        return Err(bad(cmd, format!("{key} exceeds supported numeric range")));
    }
    Ok(v)
}

pub(super) fn valid_position(m: &Mat) -> bool {
    let column = |i: usize| Vec3::new(m[i][0], m[i][1], m[i][2]);
    let (x, y, z) = (column(0), column(1), column(2));
    m.iter().flatten().all(|x| x.is_finite())
        && m[3][..3].iter().all(|x| x.abs() <= 1e9)
        && [x, y, z].iter().all(|axis| (axis.dot(*axis) - 1.0).abs() < 1e-6)
        && x.dot(y).abs() < 1e-6
        && x.dot(z).abs() < 1e-6
        && y.dot(z).abs() < 1e-6
        && x.cross(y).dot(z) > 0.0
        && [m[0][3], m[1][3], m[2][3]].iter().all(|x| x.abs() < 1e-9)
        && (m[3][3] - 1.0).abs() < 1e-9
}

fn new_component(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "component.create";
    let parent = if p.get("parent").is_some() { component(s, p, "parent", cmd)? } else { s.active_component };
    let parent_path = if let Some(value) = p.get("parent_path") {
        let path = value
            .as_array()
            .filter(|a| a.len() < 64)
            .and_then(|a| a.iter().map(Value::as_u64).collect::<Option<Vec<_>>>())
            .ok_or_else(|| bad(cmd, "parent_path must list up to 63 occurrence ids"))?;
        let component = if path.is_empty() { 0 } else { s.occurrence_placement(&path).map_err(|e| bad(cmd, e.to_string()))?.component };
        if component != parent {
            return Err(bad(cmd, "parent_path belongs to another component"));
        }
        path
    } else if parent == s.active_component {
        s.active_occurrence_path.clone().unwrap_or_else(|| s.doc.first_path(parent))
    } else {
        s.doc.first_path(parent)
    };
    if parent_path.len() >= 64 {
        return Err(bad(cmd, "new component exceeds occurrence depth limit"));
    }
    let id = s.doc_mut().add_component(str_(p, "name"), parent)?;
    if p.get("activate").and_then(Value::as_bool).unwrap_or(true) {
        let mut path = parent_path;
        if let Some(o) = s.doc.occurrence_of(id) {
            path.push(o.id);
        }
        s.active_component = id;
        s.active_occurrence_path = Some(path);
    }
    let name = s.doc.components.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_default();
    let occ = s.doc.occurrence_of(id).map(|o| o.id);
    Ok(json!({"component": id, "name": name, "parent": parent, "occurrence": occ}))
}

fn from_bodies(s: &mut Session, p: &Value) -> Result<Value> {
    transfer::from_bodies(s, p)
}

fn activate(s: &mut Session, p: &Value) -> Result<Value> {
    let id = component(s, p, "component", "component.activate")?;
    let path = if p.get("path").is_some() || p.get("occurrence").is_some() {
        let occurrence = occurrence(s, p, "component.activate")?;
        let path = occurrence_path(s, p, occurrence, "component.activate")?;
        if s.occurrence_placement(&path)?.component != id {
            return Err(bad("component.activate", "path belongs to another component"));
        }
        Some(path)
    } else {
        None
    };
    s.active_component = id;
    s.active_occurrence_path = path;
    s.revision += 1;
    Ok(json!({ "active": id }))
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let st = s.model.state();
    let doc = &s.doc;
    // Compact: what a large assembly needs to be found around in, without every name and matrix.
    let compact = bool_(p, "compact").unwrap_or(false);
    let names = |v: Vec<&str>| if compact { json!(v.len()) } else { json!(v) };
    let body_names = !compact || bool_(p, "bodies").unwrap_or(false);
    let placement = |o: &Occurrence| {
        if !compact {
            return json!(o);
        }
        let mut v = json!({"id": o.id, "name": o.name, "parent": o.parent, "grounded": o.grounded});
        if o.transform.iter().flatten().zip(solidcraft_doc::IDENTITY.iter().flatten()).any(|(a, b)| (a - b).abs() > 1e-12) {
            v["transform"] = json!(o.transform);
        }
        v
    };
    let node = |id: u64, name: &str, parent: Option<u64>| {
        let bodies: Vec<&str> = st.bodies.iter().filter(|b| doc.model_body_component(b) == id).map(|b| b.name.as_str()).collect();
        let features: Vec<&str> = doc
            .features
            .iter()
            .filter(|f| f.component == id && !matches!(f.kind, FeatureKind::Sketch { .. } | FeatureKind::ConstructionPlane { .. }))
            .map(|f| f.name.as_str())
            .collect();
        let sketches: Vec<&str> =
            doc.features.iter().filter(|f| f.component == id && matches!(f.kind, FeatureKind::Sketch { .. })).map(|f| f.name.as_str()).collect();
        let construction: Vec<&str> = doc
            .features
            .iter()
            .filter(|f| f.component == id && matches!(f.kind, FeatureKind::ConstructionPlane { .. }))
            .map(|f| f.name.as_str())
            .collect();
        let occurrences: Vec<Value> = doc.occurrences.iter().filter(|o| o.component == id).map(placement).collect();
        let children: Vec<Value> = doc.occurrences.iter().filter(|o| o.parent == id).map(|o| if compact { json!(o.id) } else { json!(o) }).collect();
        let bodies = if body_names { json!(bodies) } else { json!(bodies.len()) };
        json!({"id": id, "name": name, "parent": parent, "bodies": bodies, "features": names(features), "sketches": names(sketches),
            "construction": names(construction), "occurrences": occurrences, "children": children})
    };
    let mut out = vec![node(0, &doc.name, None)];
    for c in &doc.components {
        out.push(node(c.id, &c.name, Some(c.parent)));
    }
    Ok(json!({"active": s.active_component, "components": out, "pending_moves": s.pending_moves.keys().collect::<Vec<_>>()}))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "component.rename";
    let id = component(s, p, "component", cmd)?;
    let name =
        str_(p, "name").map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128).ok_or_else(|| bad(cmd, "`name` must be 1…128 characters"))?;
    let d = s.doc_mut();
    if id == 0 {
        d.name = name.to_string();
    } else if let Some(c) = d.components.iter_mut().find(|c| c.id == id) {
        let old = std::mem::replace(&mut c.name, name.to_string());
        // Placements still named after the component ("Old:2") follow it (#1665); others keep theirs.
        for o in d.occurrences.iter_mut().filter(|o| o.component == id) {
            let k = o.name.strip_prefix(old.as_str()).and_then(|r| r.strip_prefix(':'));
            if let Some(k) = k.filter(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit())) {
                o.name = format!("{name}:{k}");
            }
        }
    }
    Ok(json!({"component": id, "name": name}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "component.delete";
    let id = component(s, p, "component", cmd)?;
    if id == 0 {
        return Err(bad(cmd, "the design itself can't be deleted"));
    }
    let gone: Vec<u64> = s.doc.components.iter().filter(|c| s.doc.component_within(c.id, id)).map(|c| c.id).collect();
    if s.doc.features.iter().any(|feature| {
        !gone.contains(&feature.component)
            && matches!(&feature.kind, FeatureKind::BodyTransfer { from, to, .. } if gone.contains(from) || gone.contains(to))
    }) {
        return Err(bad(
            cmd,
            "component supplies a recorded body transfer outside this subtree; remove that transfer history before deleting its source",
        ));
    }
    let feats: Vec<u64> = s.doc.features.iter().filter(|f| gone.contains(&f.component)).map(|f| f.id).collect();
    let d = s.doc_mut();
    for f in &feats {
        let _ = d.delete_feature(*f);
    }
    d.components.retain(|c| !gone.contains(&c.id));
    d.remove_configuration_components(&gone);
    d.occurrences.retain(|o| !gone.contains(&o.component) && !gone.contains(&o.parent));
    let moved_out: Vec<String> = d.body_components.iter().filter(|(_, c)| gone.contains(c)).map(|(b, _)| b.clone()).collect();
    d.body_components.retain(|_, c| !gone.contains(c));
    d.body_offsets.retain(|b, _| !moved_out.contains(b));
    if gone.contains(&s.active_component) {
        s.active_component = 0;
    }
    Ok(json!({"deleted": gone, "features": feats}))
}

fn unlock(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "component.unlock";
    let id = component(s, p, "component", cmd)?;
    if id == 0 {
        return Err(bad(cmd, "pick a component, not the design"));
    }
    let gone = s.contents_joints(id);
    if gone.is_empty() {
        return Err(bad(cmd, "nothing holds this component's contents together"));
    }
    let doc = s.doc_mut();
    doc.assembly.joints.retain(|j| !gone.contains(&j.id));
    doc.assembly.links.retain(|l| !gone.contains(&l.a) && !gone.contains(&l.b));
    s.visibility.hidden_joints.retain(|h| !gone.contains(h));
    Ok(json!({"component": id, "joints": gone}))
}

fn move_bodies(s: &mut Session, p: &Value) -> Result<Value> {
    transfer::move_bodies(s, p)
}

pub(crate) fn transfer_constructor_result(s: &mut Session, p: &Value) -> Result<Value> {
    transfer::move_result(s, p)
}

fn move_sketches(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "component.move_sketches";
    let to = component(s, p, "component", cmd)?;
    let keys = string_list(p, "sketches");
    let mut ids = Vec::new();
    for k in &keys {
        match s.doc.find_feature(k) {
            Some(f) if matches!(f.kind, FeatureKind::Sketch { .. }) => ids.push(f.id),
            _ => return Err(bad(cmd, format!("no sketch `{k}`"))),
        }
    }
    if ids.is_empty() {
        return Err(bad(cmd, "`sketches` must list sketches"));
    }
    for id in &ids {
        if let Some(f) = s.doc_mut().feature_mut(*id) {
            f.component = to;
        }
    }
    Ok(json!({"sketches": ids, "component": to}))
}

fn copy_occurrence(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "occurrence.copy";
    let selected = if p.get("occurrence").is_some() {
        let o = occurrence(s, p, cmd)?;
        Some(o)
    } else {
        None
    };
    let comp = if let Some(id) = selected {
        s.doc.occurrences.iter().find(|o| o.id == id).map(|o| o.component).unwrap_or(0)
    } else {
        component(s, p, "component", cmd)?
    };
    let src = if let Some(id) = selected { s.doc.occurrences.iter().find(|o| o.id == id) } else { s.doc.occurrence_of(comp) }
        .cloned()
        .ok_or_else(|| bad(cmd, "that component has no occurrence"))?;
    if p.get("path").is_some() {
        occurrence_path(s, p, src.id, cmd)?;
    }
    let parent = if p.get("parent").is_some() { component(s, p, "parent", cmd)? } else { src.parent };
    let t = move_vector(p, "translate", Vec3::ZERO, true, cmd)?;
    let base = s.pending_moves.get(&src.id).unwrap_or(&src.transform);
    let m = solidcraft_doc::mat_mul(&translation(t), base);
    if !valid_position(&m) {
        return Err(bad(cmd, "copy transform exceeds supported numeric range"));
    }
    let id = s.doc_mut().add_occurrence(comp, parent, m)?;
    Ok(json!({"occurrence": id, "component": comp}))
}

fn paste_new(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "component.paste_new";
    let comp = component(s, p, "component", cmd)?;
    if comp == 0 {
        return Err(bad(cmd, "pick a component, not the design"));
    }
    let src = s.doc.components.iter().find(|c| c.id == comp).cloned().ok_or_else(|| bad(cmd, "component"))?;
    let occ = if p.get("occurrence").is_some() {
        let id = occurrence(s, p, cmd)?;
        if p.get("path").is_some() {
            occurrence_path(s, p, id, cmd)?;
        }
        let occurrence = s.doc.occurrences.iter().find(|o| o.id == id).cloned().ok_or_else(|| bad(cmd, "no occurrence"))?;
        if occurrence.component != comp {
            return Err(bad(cmd, "occurrence belongs to another component"));
        }
        Some(occurrence)
    } else {
        s.doc.occurrence_of(comp).cloned()
    };
    let t = move_vector(p, "translate", Vec3::ZERO, true, cmd)?;
    let base = occ.as_ref().map(|o| s.pending_moves.get(&o.id).copied().unwrap_or(o.transform)).unwrap_or(solidcraft_doc::IDENTITY);
    let transform = solidcraft_doc::mat_mul(&translation(t), &base);
    if !valid_position(&transform) {
        return Err(bad(cmd, "copy transform exceeds supported numeric range"));
    }
    let parent = occ.as_ref().map(|o| o.parent).unwrap_or(src.parent);
    super::component_clone::paste(s, comp, occ, transform, parent, p)
}

pub(super) fn occurrence_path(s: &Session, p: &Value, id: u64, cmd: &str) -> Result<Vec<u64>> {
    let path = if let Some(value) = p.get("path") {
        value
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 64)
            .and_then(|a| a.iter().map(Value::as_u64).collect::<Option<Vec<_>>>())
            .ok_or_else(|| bad(cmd, "`path` must list 1..64 occurrence ids"))?
    } else {
        let o = s.doc.occurrences.iter().find(|o| o.id == id).ok_or_else(|| bad(cmd, "no occurrence"))?;
        let mut path = s.doc.first_path(o.parent);
        // A world frame requires an explicit containing placement when any ancestor is instanced.
        for ancestor in &path {
            if let Some(a) = s.doc.occurrences.iter().find(|a| a.id == *ancestor)
                && s.doc.occurrences.iter().filter(|x| x.component == a.component).count() > 1
            {
                return Err(bad(cmd, "occurrence has multiple placements; supply its `path`"));
            }
        }
        path.push(id);
        path
    };
    if path.last() != Some(&id) {
        return Err(bad(cmd, "path and occurrence disagree"));
    }
    s.occurrence_placement(&path).map_err(|e| bad(cmd, e.to_string()))?;
    Ok(path)
}

fn move_occurrence(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "occurrence.move";
    let id = occurrence(s, p, cmd)?;
    let o = s.doc.occurrences.iter().find(|o| o.id == id).cloned().ok_or_else(|| bad(cmd, "occurrence"))?;
    if o.grounded {
        return Err(bad(cmd, format!("{} is grounded", o.name)));
    }
    if let Some(mut group) = s.held_by_rigid_group(id) {
        // The outermost component the group holds it in moves it.
        for _ in 0..64 {
            match s.doc.occurrence_of(group).and_then(|o| s.held_by_rigid_group(o.id)) {
                Some(up) => group = up,
                None => break,
            }
        }
        let name = s.doc.components.iter().find(|c| c.id == group).map(|c| c.name.clone()).unwrap_or_default();
        return Err(bad(cmd, format!("{} is held in {name} by a rigid group: move {name}, or Unlock Contents (component.unlock) first", o.name)));
    }
    let t = move_vector(p, "translate", Vec3::ZERO, true, cmd)?;
    let axis = move_vector(p, "axis", Vec3::Z, false, cmd)?;
    let angle = match p.get("angle") {
        Some(Value::String(e)) => s.doc.eval(e, solidcraft_doc::expr::Kind::Angle).map_err(|e| bad(cmd, e.to_string()))?,
        None => 0.0,
        Some(value) => value.as_f64().ok_or_else(|| bad(cmd, "angle must be a number or angle expression"))?,
    };
    if !(t.is_finite() && axis.is_finite() && angle.is_finite()) {
        return Err(bad(cmd, "non-finite move"));
    }
    if angle != 0.0 && axis.normalized().is_none() {
        return Err(bad(cmd, "rotation axis must have a finite nonzero length"));
    }
    let origin = move_vector(p, "origin", Vec3::ZERO, true, cmd)?;
    let delta = solidcraft_doc::rigid(t, origin, axis, angle);
    let space = match p.get("space") {
        None => "local",
        Some(Value::String(space)) => space.as_str(),
        _ => return Err(bad(cmd, "space must be local or world")),
    };
    if !matches!(space, "local" | "world") {
        return Err(bad(cmd, "space must be local or world"));
    }
    let path = if p.get("path").is_some() || space == "world" { Some(occurrence_path(s, p, id, cmd)?) } else { None };
    let delta = if space == "world" {
        let placement = s.occurrence_placement(path.as_deref().unwrap_or(&[]))?;
        let inverse = solidcraft_doc::mat_inverse(&placement.parent_world).ok_or_else(|| bad(cmd, "parent frame is singular"))?;
        solidcraft_doc::mat_mul(&solidcraft_doc::mat_mul(&inverse, &delta), &placement.parent_world)
    } else {
        delta
    };
    let cur = if let Some(value) = p.get("base_transform") {
        let base: Mat = serde_json::from_value(value.clone()).map_err(|_| bad(cmd, "base_transform must be a 4x4 affine matrix"))?;
        if !valid_position(&base)
            || base[0][3].abs() > 1e-12
            || base[1][3].abs() > 1e-12
            || base[2][3].abs() > 1e-12
            || (base[3][3] - 1.0).abs() > 1e-12
            || solidcraft_doc::mat_inverse(&base).is_none()
        {
            return Err(bad(cmd, "base_transform must be finite, affine and invertible"));
        }
        base
    } else {
        s.pending_moves.get(&id).copied().unwrap_or(o.transform)
    };
    let next = solidcraft_doc::mat_mul(&delta, &cur);
    if !valid_position(&next) {
        return Err(bad(cmd, "move transform exceeds supported numeric range"));
    }
    if bool_(p, "capture").unwrap_or(true) {
        s.pending_moves.remove(&id);
        if let Some(x) = s.doc_mut().occurrences.iter_mut().find(|x| x.id == id) {
            x.transform = next;
        }
    } else {
        s.pending_moves.insert(id, next);
        s.revision += 1;
    }
    Ok(json!({"occurrence": id, "transform": next, "pending": !bool_(p, "capture").unwrap_or(true)}))
}

fn ground(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "occurrence.ground";
    let id = occurrence(s, p, cmd)?;
    if p.get("path").is_some() {
        occurrence_path(s, p, id, cmd)?;
    }
    let want = bool_(p, "grounded");
    let o = s.doc_mut().occurrences.iter_mut().find(|o| o.id == id).ok_or_else(|| bad(cmd, "occurrence"))?;
    o.grounded = want.unwrap_or(!o.grounded);
    let g = o.grounded;
    Ok(json!({"occurrence": id, "grounded": g}))
}

fn capture(s: &mut Session, _p: &Value) -> Result<Value> {
    let moves = std::mem::take(&mut s.pending_moves);
    let n = moves.len();
    for (id, m) in moves {
        if let Some(o) = s.doc_mut().occurrences.iter_mut().find(|o| o.id == id) {
            o.transform = m;
        }
    }
    s.revision += 1;
    Ok(json!({ "captured": n }))
}

fn revert(s: &mut Session, p: &Value) -> Result<Value> {
    let before = s.pending_moves.len();
    if let Some(value) = p.get("occurrences") {
        let ids = value
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 10_000)
            .and_then(|a| a.iter().map(Value::as_u64).collect::<Option<Vec<_>>>())
            .ok_or_else(|| bad("component.revert_position", "occurrences must list 1..10000 ids"))?;
        if ids.iter().any(|id| !s.doc.occurrences.iter().any(|o| o.id == *id)) {
            return Err(bad("component.revert_position", "no occurrence"));
        }
        for id in ids {
            s.pending_moves.remove(&id);
        }
    } else {
        s.pending_moves.clear();
    }
    let n = before - s.pending_moves.len();
    s.revision += 1;
    Ok(json!({ "reverted": n }))
}

/// The active component's frame from world coordinates (for new features' geometry).
pub fn to_active_frame(s: &Session, kind: &mut FeatureKind) {
    // Commands whose picks `frames` maps are already in their frames.
    if s.active_component == 0 || crate::frames::mapped() {
        return;
    }
    let m = s.active_component_frame();
    if solidcraft_doc::is_identity(&m) {
        return;
    }
    if let Some(inv) = solidcraft_doc::mat_inverse(&m) {
        kind.transform_geometry(&inv);
    }
}
