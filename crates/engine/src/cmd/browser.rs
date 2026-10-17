//! Browser commands: renaming bodies, moving a sketch to another plane, and the browser's
//! groups and item order (kept in the design, so they are saved and undone like features).
//!
//! Browser items are keyed per component folder (`bodies`, `sketches`, `construction`): a body
//! by name, a sketch or construction plane by feature id.

use serde_json::{Value, json};

use solidcraft_doc::FeatureKind;

use super::CommandSpec;
use crate::params::{bad, bool_, str_};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("body.rename", "Rename Body", rename_body).icon("rename").params("body: its name, name: the new name (features that use the body follow)"),
    CommandSpec::new("sketch.redefine", "Redefine Sketch Plane", redefine_sketch)
        .icon("plane")
        .params("sketch: id or name; plane: as sketch.create; author_path?: exact owner occurrence ids; explicitly rebinds or freezes the plane while preserving geometry and dimension names"),
    CommandSpec::new("browser.group", "New Group", group)
        .icon("folder")
        .params("folder: bodies|sketches|construction; component?: id (default 0); name?; items?: [keys] (they leave other groups)"),
    CommandSpec::new("browser.ungroup", "Ungroup", ungroup).icon("folder").params("group: id (its items go back to the folder)"),
    CommandSpec::new("browser.rename_group", "Rename Group", rename_group).icon("rename").params("group: id, name"),
    CommandSpec::new("browser.move", "Move Browser Items", move_items).icon("reorder")
        .params("folder, component?, items: [keys]; group?: id (into it; omit to take them out of groups); before?: key (position in the group)"),
    CommandSpec::new("document.units", "Change Design Units", document_units)
        .icon("settings")
        .params("units: mm|cm|m|in|ft (display and unit-less parameters; values keep their meaning)"),
    CommandSpec::new("view.save", "New Named View", view_save)
        .icon("camera")
        .params("name?; camera: {target: [x,y,z], yaw, pitch, distance, fov?} (replaces a view of the same name)"),
    CommandSpec::new("view.rename", "Rename Named View", view_rename).icon("rename").params("view: name, name"),
    CommandSpec::new("view.delete", "Delete Named View", view_delete).icon("delete").params("view: name"),
    CommandSpec::new("view.list", "List Named Views", view_list).icon("list").noundo().doc_only(),
    CommandSpec::new("view.apply", "Restore Named View", view_apply).icon("camera").noundo().params(
        "view?: front|back|top|bottom|left|right|iso|home (frames the model) | a named view's name; or camera?: {target: [x,y,z], yaw, pitch, distance, fov?} (radians, mm) → camera. The viewport (and the MCP screenshot) moves there. View state: not saved, not an undo step",
    ),
    CommandSpec::new("browser.order", "Order Browser Items", order).icon("reorder").params("folder, component?, order: [keys] (the folder's items in display order)"),
    CommandSpec::new("browser.visibility", "Show/Hide", visibility).icon("eye").noundo().params(
        "visible: bool; items?: [sketch id or name | body name | construction name | joint name | O|X|Y|Z|XY|XZ|YZ]; \
         or joints?: [joint id or name] (their glyphs; they still constrain and move parts); \
         or folder?: bodies|sketches|construction|origin|joints (all of it); or paths?: [[occurrence ids]] (exact placements and descendants), isolate?: bool (requires paths and visible:true). Saved with the design; not an undo step",
    ),
];

const FOLDERS: [&str; 3] = ["bodies", "sketches", "construction"];

fn folder_of(p: &Value, cmd: &str) -> Result<String> {
    match str_(p, "folder") {
        Some(f) if FOLDERS.contains(&f) => Ok(f.to_string()),
        _ => Err(bad(cmd, "`folder` must be bodies, sketches or construction")),
    }
}

fn component_of(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    let c = p.get("component").and_then(Value::as_u64).unwrap_or(0);
    if c != 0 && !s.doc.components.iter().any(|x| x.id == c) {
        return Err(bad(cmd, format!("no component {c}")));
    }
    Ok(c)
}

fn keys(p: &Value, k: &str, cmd: &str) -> Result<Vec<String>> {
    let v = match p.get(k) {
        Some(Value::Array(a)) => {
            a.iter().map(|x| x.as_str().map(str::to_string).or_else(|| x.as_u64().map(|n| n.to_string()))).collect::<Option<Vec<_>>>()
        }
        None => Some(Vec::new()),
        _ => None,
    };
    let v = v.ok_or_else(|| bad(cmd, format!("`{k}` must be a list of item keys")))?;
    if v.len() > 10_000 || v.iter().any(|x| x.is_empty() || x.len() > 256) {
        return Err(bad(cmd, format!("`{k}` has bad keys")));
    }
    Ok(v)
}

fn group_id(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    let id = p.get("group").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`group` must be a group id"))?;
    if s.doc.browser_groups.iter().any(|g| g.id == id) { Ok(id) } else { Err(bad(cmd, format!("no group {id}"))) }
}

fn group(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "browser.group";
    let folder = folder_of(p, cmd)?;
    let component = component_of(s, p, cmd)?;
    let items = keys(p, "items", cmd)?;
    let d = s.doc_mut();
    let id = d.browser_groups.iter().map(|g| g.id).max().unwrap_or(0) + 1;
    let name = match str_(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if n.len() <= 128 => n.to_string(),
        Some(_) => return Err(bad(cmd, "`name` is too long")),
        None => format!("Group{}", d.browser_groups.iter().filter(|g| g.folder == folder).count() + 1),
    };
    for g in d.browser_groups.iter_mut().filter(|g| g.component == component && g.folder == folder) {
        g.items.retain(|k| !items.contains(k));
    }
    d.browser_groups.push(solidcraft_doc::BrowserGroup { id, name: name.clone(), component, folder, items });
    Ok(json!({ "group": id, "name": name }))
}

fn ungroup(s: &mut Session, p: &Value) -> Result<Value> {
    let id = group_id(s, p, "browser.ungroup")?;
    s.doc_mut().browser_groups.retain(|g| g.id != id);
    Ok(json!({ "group": id }))
}

fn rename_group(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "browser.rename_group";
    let id = group_id(s, p, cmd)?;
    let name =
        str_(p, "name").map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128).ok_or_else(|| bad(cmd, "`name` must be 1…128 characters"))?;
    if let Some(g) = s.doc_mut().browser_groups.iter_mut().find(|g| g.id == id) {
        g.name = name.to_string();
    }
    Ok(json!({ "group": id, "name": name }))
}

fn move_items(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "browser.move";
    let folder = folder_of(p, cmd)?;
    let component = component_of(s, p, cmd)?;
    let items = keys(p, "items", cmd)?;
    if items.is_empty() {
        return Err(bad(cmd, "`items` must list item keys"));
    }
    let to = match p.get("group") {
        Some(Value::Null) | None => None,
        Some(_) => Some(group_id(s, p, cmd)?),
    };
    if let Some(g) = to
        && !s.doc.browser_groups.iter().any(|x| x.id == g && x.component == component && x.folder == folder)
    {
        return Err(bad(cmd, "that group is in another folder"));
    }
    let before = str_(p, "before").map(str::to_string);
    let d = s.doc_mut();
    for g in d.browser_groups.iter_mut().filter(|g| g.component == component && g.folder == folder) {
        g.items.retain(|k| !items.contains(k));
        if Some(g.id) == to {
            let at = before.as_ref().and_then(|b| g.items.iter().position(|k| k == b)).unwrap_or(g.items.len());
            for (i, k) in items.iter().enumerate() {
                g.items.insert(at + i, k.clone());
            }
        }
    }
    Ok(json!({ "items": items, "group": to }))
}

fn order(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "browser.order";
    let folder = folder_of(p, cmd)?;
    let component = component_of(s, p, cmd)?;
    let order = keys(p, "order", cmd)?;
    let key = format!("{component}/{folder}");
    let d = s.doc_mut();
    if order.is_empty() {
        d.browser_order.remove(&key);
    } else {
        d.browser_order.insert(key, order);
    }
    Ok(json!({}))
}

const ORIGIN: [&str; 7] = ["O", "X", "Y", "Z", "XY", "XZ", "YZ"];

/// A browser item whose visibility can change.
enum Shown {
    Sketch(u64),
    Body(String),
    /// Origin items and construction geometry, by name.
    Origin(String),
    /// An assembly joint's glyph, by joint id.
    Joint(u64),
}

fn is_construction(k: &FeatureKind) -> bool {
    matches!(k, FeatureKind::ConstructionPlane { .. } | FeatureKind::ConstructionAxis { .. } | FeatureKind::ConstructionPoint { .. })
}

fn shown_item(s: &Session, key: &str) -> Option<Shown> {
    if let Some(f) = s.doc.find_feature(key) {
        if matches!(f.kind, FeatureKind::Sketch { .. }) {
            return Some(Shown::Sketch(f.id));
        }
        if is_construction(&f.kind) {
            return Some(Shown::Origin(f.name.clone()));
        }
    }
    if s.world_state().bodies.iter().any(|b| b.name == key) {
        return Some(Shown::Body(key.to_string()));
    }
    if let Some(j) = s.doc.assembly.joints.iter().find(|j| j.name == key) {
        return Some(Shown::Joint(j.id));
    }
    ORIGIN.iter().find(|o| o.eq_ignore_ascii_case(key)).map(|o| Shown::Origin(o.to_string()))
}

fn visibility(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "browser.visibility";
    let visible = bool_(p, "visible").ok_or_else(|| bad(cmd, "`visible` must be true or false"))?;
    if let Some(value) = p.get("paths") {
        if p.get("items").is_some() || p.get("folder").is_some() {
            return Err(bad(cmd, "give paths, items or folder, not several"));
        }
        let paths =
            value.as_array().filter(|a| !a.is_empty() && a.len() <= 10_000).ok_or_else(|| bad(cmd, "paths must list 1..10000 occurrence paths"))?;
        let paths: Vec<Vec<u64>> = paths
            .iter()
            .map(|value| {
                let path = value
                    .as_array()
                    .filter(|a| !a.is_empty() && a.len() <= 64)
                    .and_then(|a| a.iter().map(Value::as_u64).collect::<Option<Vec<_>>>())
                    .ok_or_else(|| bad(cmd, "each path must list 1..64 occurrence ids"))?;
                s.occurrence_placement(&path).map_err(|e| bad(cmd, e.to_string()))?;
                Ok(path)
            })
            .collect::<Result<_>>()?;
        if bool_(p, "isolate").unwrap_or(false) {
            if !visible {
                return Err(bad(cmd, "isolate requires visible:true"));
            }
            let mut hidden = Vec::new();
            let mut ancestor_bodies = Vec::new();
            let mut stack = vec![Vec::<u64>::new()];
            let mut count = 0;
            while let Some(parent) = stack.pop() {
                for child in s.occurrence_children(&parent)? {
                    count += 1;
                    if count > 100_000 {
                        return Err(bad(cmd, "isolate exceeds 100000 placements"));
                    }
                    if paths.iter().any(|selected| child.path.starts_with(selected)) {
                        continue;
                    }
                    if paths.iter().any(|selected| selected.starts_with(&child.path)) {
                        ancestor_bodies.extend(child.bodies.into_iter().map(|b| b.world));
                        stack.push(child.path);
                    } else {
                        hidden.push(child.path);
                    }
                }
            }
            let mut root_bodies: Vec<String> =
                s.model.state().bodies.iter().filter(|b| s.doc.model_body_component(b) == 0).map(|b| b.name.clone()).collect();
            root_bodies.extend(ancestor_bodies);
            s.visibility.hidden_occurrences = hidden;
            s.visibility.hidden_bodies = root_bodies;
            s.revision += 1;
            return Ok(json!({"visible":true,"isolated":paths,"visibility":s.visibility}));
        }
        for path in &paths {
            s.visibility.hidden_occurrences.retain(|p| p != path);
            if !visible {
                s.visibility.hidden_occurrences.push(path.clone());
            }
        }
        s.revision += 1;
        return Ok(json!({"visible":visible,"changed":paths.len(),"visibility":s.visibility}));
    }
    let items: Vec<Shown> = match (str_(p, "folder"), p.get("items")) {
        _ if p.get("joints").is_some() => {
            if p.get("items").is_some() || p.get("folder").is_some() {
                return Err(bad(cmd, "give `joints`, `items` or `folder`, not several"));
            }
            let list = p
                .get("joints")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty() && a.len() <= 10_000)
                .ok_or_else(|| bad(cmd, "`joints` must list 1..10000 joint ids or names"))?;
            list.iter().map(|j| super::joints::joint_id(s, Some(j), cmd).map(Shown::Joint)).collect::<Result<_>>()?
        }
        (Some(_), Some(_)) => return Err(bad(cmd, "give `items` or `folder`, not both")),
        (Some("bodies"), None) => s.world_state().bodies.iter().map(|b| Shown::Body(b.name.clone())).collect(),
        (Some("sketches"), None) => s.model.state().sketches.iter().map(|x| Shown::Sketch(x.feature)).collect(),
        (Some("construction"), None) => s.doc.features.iter().filter(|f| is_construction(&f.kind)).map(|f| Shown::Origin(f.name.clone())).collect(),
        (Some("origin"), None) => ORIGIN.iter().map(|o| Shown::Origin(o.to_string())).collect(),
        (Some("joints"), None) => s.doc.assembly.joints.iter().map(|j| Shown::Joint(j.id)).collect(),
        (Some(f), None) => return Err(bad(cmd, format!("unknown folder `{f}` (bodies, sketches, construction, origin, joints)"))),
        (None, Some(_)) => {
            let keys = keys(p, "items", cmd)?;
            if keys.is_empty() {
                return Err(bad(cmd, "`items` must list sketches, bodies or construction geometry"));
            }
            // Every item is found before anything changes.
            keys.iter()
                .map(|k| shown_item(s, k).ok_or_else(|| bad(cmd, format!("no sketch, body, construction item or joint `{k}`"))))
                .collect::<Result<_>>()?
        }
        (None, None) => return Err(bad(cmd, "`items` or `folder` is required")),
    };
    let v = &mut s.visibility;
    if visible && str_(p, "folder") == Some("bodies") {
        v.hidden_occurrences.clear();
    }
    for it in &items {
        match it {
            Shown::Sketch(id) => {
                v.hidden_sketches.retain(|x| x != id);
                v.shown_sketches.retain(|x| x != id);
                if visible { v.shown_sketches.push(*id) } else { v.hidden_sketches.push(*id) }
            }
            Shown::Body(n) => {
                v.hidden_bodies.retain(|x| x != n);
                if !visible {
                    v.hidden_bodies.push(n.clone());
                }
            }
            Shown::Origin(n) => {
                v.hidden_origin.retain(|x| x != n);
                if !visible {
                    v.hidden_origin.push(n.clone());
                }
            }
            Shown::Joint(id) => {
                v.hidden_joints.retain(|x| x != id);
                if !visible {
                    v.hidden_joints.push(*id);
                }
            }
        }
    }
    s.revision += 1;
    Ok(json!({ "visible": visible, "changed": items.len(), "visibility": s.visibility }))
}

fn document_units(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "document.units";
    let u = str_(p, "units").ok_or_else(|| bad(cmd, "`units` is required"))?;
    if !["mm", "cm", "m", "in", "ft"].contains(&u) {
        return Err(bad(cmd, "`units` must be mm, cm, m, in or ft"));
    }
    let old = s.doc.units.clone();
    if old == u {
        return Ok(json!({ "units": u, "kept": [] }));
    }
    // What the geometry reads: every feature's length inputs.
    let matched_configuration = s.doc.configuration_matches().then(|| s.doc.configs.active.clone()).flatten();
    let before = s.doc.length_inputs();
    let solved = s.solved_sketches();
    s.doc_mut().units = u.to_string();
    // Unit-less user parameters holding a bare number read it in the design's units where they
    // stand for lengths. If any input moved, those numbers keep the old unit, written out.
    let same = |a: &[Option<f64>]| {
        a.iter().zip(&before).filter(|(x, y)| matches!((x, y), (Some(x), Some(y)) if (x - y).abs() <= 1e-9 * y.abs().max(1.0))).count()
    };
    let after = s.doc.length_inputs();
    let mut kept = Vec::new();
    if same(&after) < before.len() {
        let bare: Vec<(String, String)> = s
            .doc
            .params
            .iter()
            .filter(|p| !p.model && p.unit.is_empty() && solidcraft_doc::expr::is_literal(&p.expr) && p.expr.trim().parse::<f64>().is_ok())
            .map(|p| (p.name.clone(), p.expr.trim().to_string()))
            .collect();
        for (name, e) in bare {
            let now = same(&s.doc.length_inputs());
            let probe = {
                let mut d = (*s.doc).clone();
                let _ = d.change_param(&name, &format!("{e} {old}"), Some(&old), None);
                same(&d.length_inputs())
            };
            // Only when writing the old unit puts more inputs back where they were.
            if probe > now {
                s.doc_mut().change_param(&name, &format!("{e} {old}"), Some(&old), None)?;
                kept.push(name);
            }
        }
    }
    s.doc_mut().preserve_configuration_units(&old, &kept);
    if let Some(row) = matched_configuration {
        s.doc_mut().apply_configuration(&row)?;
    }
    s.settle_value_edit(&solved)?;
    Ok(json!({ "units": u, "kept": kept }))
}

fn view_save(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "view.save";
    let c = p.get("camera").ok_or_else(|| bad(cmd, "`camera` is required"))?;
    let mut v: solidcraft_doc::NamedView = serde_json::from_value(c.clone()).map_err(|e| bad(cmd, format!("bad camera: {e}")))?;
    let ok = [v.target.x, v.target.y, v.target.z, v.yaw, v.pitch, v.distance, v.fov].iter().all(|x| x.is_finite()) && v.distance > 0.0;
    if !ok {
        return Err(bad(cmd, "the camera must be finite with a positive distance"));
    }
    let n = s.doc.named_views.len();
    v.name = match str_(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if n.len() <= 128 => n.to_string(),
        Some(_) => return Err(bad(cmd, "`name` is too long")),
        None => (n + 1..).map(|k| format!("Named View{k}")).find(|x| !s.doc.named_views.iter().any(|v| v.name == *x)).unwrap_or_default(),
    };
    if n >= 1000 {
        return Err(bad(cmd, "too many named views"));
    }
    let name = v.name.clone();
    let d = s.doc_mut();
    d.named_views.retain(|x| x.name != name);
    d.named_views.push(v);
    Ok(json!({ "view": name }))
}

fn view_apply(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "view.apply";
    let cam = match (str_(p, "view"), p.get("camera")) {
        (Some(_), Some(_)) => return Err(bad(cmd, "give `view` or `camera`, not both")),
        (None, None) => return Err(bad(cmd, "`view` (a name) or `camera` is required")),
        (None, Some(c)) => {
            let v: solidcraft_doc::NamedView = serde_json::from_value(c.clone()).map_err(|e| bad(cmd, format!("bad camera: {e}")))?;
            camera_of(&v)
        }
        // Standard view names come first, as in the browser's Named Views folder.
        (Some(name), None) => match crate::render::StandardView::parse(name) {
            Some(sv) => {
                let mut c = crate::view::home_camera(s);
                c.set_view(sv);
                c.fit(&crate::view::bounds(s));
                c
            }
            None => s.doc.named_views.iter().find(|v| v.name == name).map(camera_of).ok_or_else(|| bad(cmd, format!("no named view `{name}`")))?,
        },
    };
    if !cam.is_valid() || !cam.fov.is_finite() || cam.fov < 0.0 {
        return Err(bad(cmd, "the camera must be finite with a positive distance"));
    }
    s.camera = Some(cam);
    Ok(json!({ "camera": cam }))
}

/// The orbit camera of a named view (Z up, no roll).
fn camera_of(v: &solidcraft_doc::NamedView) -> crate::render::Camera {
    crate::render::Camera { target: v.target, yaw: v.yaw, pitch: v.pitch, distance: v.distance, fov: v.fov, ..Default::default() }
}

fn view_index(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    let k = str_(p, "view").ok_or_else(|| bad(cmd, "`view` must be a view name"))?;
    s.doc.named_views.iter().position(|v| v.name == k).ok_or_else(|| bad(cmd, format!("no named view `{k}`")))
}

fn view_rename(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "view.rename";
    let i = view_index(s, p, cmd)?;
    let name =
        str_(p, "name").map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128).ok_or_else(|| bad(cmd, "`name` must be 1…128 characters"))?;
    if s.doc.named_views.iter().enumerate().any(|(j, v)| j != i && v.name == name) {
        return Err(bad(cmd, format!("another view is called `{name}`")));
    }
    if let Some(v) = s.doc_mut().named_views.get_mut(i) {
        v.name = name.to_string();
    }
    Ok(json!({ "view": name }))
}

fn view_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let i = view_index(s, p, "view.delete")?;
    let v = s.doc_mut().named_views.remove(i);
    Ok(json!({ "deleted": v.name }))
}

fn view_list(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({ "views": s.doc.named_views }))
}

/// Rename an item key in the browser's groups and orders of a folder.
fn rename_key(d: &mut solidcraft_doc::Document, folder: &str, old: &str, new: &str) {
    for g in d.browser_groups.iter_mut().filter(|g| g.folder == folder) {
        for k in g.items.iter_mut().filter(|k| *k == old) {
            *k = new.to_string();
        }
    }
    for (_, v) in d.browser_order.iter_mut().filter(|(k, _)| k.ends_with(&format!("/{folder}"))) {
        for k in v.iter_mut().filter(|k| *k == old) {
            *k = new.to_string();
        }
    }
}

/// The keys whose strings name bodies, alone or in a list (`body`, `targets`…), at any depth of a
/// feature: its own fields and the face, edge and link references inside it.
const BODY_KEYS: [&str; 5] = ["body", "bodies", "target", "targets", "tools"];

/// Replace the body name `old` with `new` where a string names a body (the value of one of
/// [`BODY_KEYS`]); other strings equal to it (sketch text, plane and feature names, expressions)
/// stay. True when something changed.
fn replace_body_name(v: &mut Value, old: &str, new: &str, names_body: bool, depth: usize) -> bool {
    if depth > 64 {
        return false;
    }
    match v {
        Value::String(s) if names_body && s == old => {
            *s = new.to_string();
            true
        }
        Value::Array(a) => a.iter_mut().fold(false, |acc, x| replace_body_name(x, old, new, names_body, depth + 1) | acc),
        Value::Object(o) => o.iter_mut().fold(false, |acc, (k, x)| replace_body_name(x, old, new, BODY_KEYS.contains(&k.as_str()), depth + 1) | acc),
        _ => false,
    }
}

fn rename_body(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "body.rename";
    let old = str_(p, "body").ok_or_else(|| bad(cmd, "`body` must be a body name"))?.to_string();
    let name =
        str_(p, "name").map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128).ok_or_else(|| bad(cmd, "`name` must be 1…128 characters"))?;
    if name == old {
        return Ok(json!({"body": old, "name": name}));
    }
    let st = s.model.state();
    let b = st.body(&old).ok_or_else(|| bad(cmd, format!("no body `{old}`")))?;
    if st.body(name).is_some() {
        return Err(bad(cmd, format!("another body is called `{name}`")));
    }
    // The body is the k-th new body of its feature; its siblings keep the names they have now
    // (unpinned pattern copies would otherwise be renumbered into the freed name, #265). That
    // includes the siblings whose stored name is shared: the solids of a STEP file with one label
    // are stored as `X`, `X`, `X` and built as `X`, `X (2)`, `X (3)`, so the stored names are set
    // to the numbered ones, or `X (3)` would turn into `X (2)` once the second solid is renamed.
    let fid = b.feature;
    let siblings: Vec<String> = st.bodies.iter().filter(|x| x.feature == fid).map(|x| x.name.clone()).collect();
    let k = siblings.iter().position(|n| *n == old).unwrap_or(0);
    // A body numbered by the counter (`BodyN`) gives its number back once it has a name of its
    // own, and every counted body after it would move down one (`Body3` becoming `Body1`) and
    // break what names it. Those later features keep the names they made too (#1109): each one's
    // own result holds them, also for bodies a later feature joins away.
    let counted = |f: &solidcraft_doc::Feature, i: usize| {
        let first = f.body_names.first().is_some_and(|n| !n.trim().is_empty());
        f.body_names.get(i).is_none_or(|n| n.trim().is_empty()) && (f.shell_piece_order || i < f.body_names.len() || !first)
    };
    let later: Vec<(u64, Vec<String>)> = match s.doc.features.iter().position(|f| f.id == fid) {
        Some(at) if s.doc.features.get(at).is_some_and(|f| counted(f, k)) => s
            .doc
            .features
            .iter()
            .skip(at + 1)
            .filter_map(|f| {
                let made: Vec<String> = s.model.result(f.id)?.state.bodies.iter().filter(|x| x.feature == f.id).map(|x| x.name.clone()).collect();
                (0..made.len()).any(|i| counted(f, i)).then_some((f.id, made))
            })
            .collect(),
        _ => Vec::new(),
    };
    let d = s.doc_mut();
    for (id, made) in &later {
        if let Some(f) = d.feature_mut(*id) {
            f.body_names.clone_from(made);
        }
    }
    let f = d.feature_mut(fid).ok_or_else(|| bad(cmd, "the body's feature is gone"))?;
    while f.body_names.len() < siblings.len().max(k + 1) {
        let i = f.body_names.len();
        f.body_names.push(siblings.get(i).cloned().unwrap_or_default());
    }
    for (slot, current) in f.body_names.iter_mut().zip(&siblings) {
        slot.clone_from(current);
    }
    if let Some(slot) = f.body_names.get_mut(k) {
        *slot = name.to_string();
    }
    // Later features refer to bodies by name.
    let mut failed = None;
    for f in d.features.iter_mut().filter(|f| f.id != fid) {
        // The bodies a join/cut/intersect may change (a primitive's `targets`) are kept beside
        // the feature's kind, not in it (#1108).
        for b in f.participants.iter_mut().filter(|b| **b == old) {
            *b = name.to_string();
        }
        if let FeatureKind::Sketch { plane, .. } = &mut f.kind {
            solidcraft_doc::sheet::rename_plane_sheet_body(plane, &old, name);
        }
        if let FeatureKind::Sketch { plane, sketch } = &mut f.kind
            && let Some(anchor) = super::sketch::face::anchor_mut(plane)
            && anchor.target.body == old
        {
            let ids = std::collections::BTreeMap::from([(anchor.target.origin_feature, anchor.target.origin_feature)]);
            let bodies = std::collections::BTreeMap::from([(old.clone(), name.to_string())]);
            anchor.remap_target(&ids, &bodies)?;
            for link in &mut sketch.links {
                if link.auto_face && matches!(&link.source,solidcraft_sketch::LinkSource::Face {body,..} if *body==old) {
                    link.face_name = Some(anchor.target.name.clone());
                }
            }
        }
        let Ok(mut v) = serde_json::to_value(&f.kind) else { continue };
        if replace_body_name(&mut v, &old, name, false, 0) {
            match serde_json::from_value(v) {
                Ok(k) => f.kind = k,
                Err(e) => failed = Some(e.to_string()),
            }
        }
    }
    if let Some(e) = failed {
        return Err(bad(cmd, e));
    }
    if let Some(m) = d.materials.remove(&old) {
        d.materials.insert(name.to_string(), m);
    }
    if let Some(c) = d.appearances.bodies.remove(&old) {
        d.appearances.bodies.insert(name.to_string(), c);
    }
    for f in d.appearances.faces.iter_mut().filter(|f| f.body == old) {
        f.body = name.to_string();
    }
    for column in &mut d.configs.columns {
        match column {
            solidcraft_doc::config::Column::Material(body) | solidcraft_doc::config::Column::Appearance(body) if *body == old => {
                *body = name.to_string();
            }
            solidcraft_doc::config::Column::FaceAppearance(target) if target.body == old => {
                target.body = name.to_string();
            }
            _ => {}
        }
    }
    if let Some(o) = d.body_offsets.remove(&old) {
        d.body_offsets.insert(name.to_string(), o);
    }
    if let Some(c) = d.body_components.remove(&old) {
        d.body_components.insert(name.to_string(), c);
    }
    rename_key(d, "bodies", &old, name);
    for b in s.visibility.hidden_bodies.iter_mut().filter(|b| **b == old) {
        *b = name.to_string();
    }
    Ok(json!({"body": old, "name": name}))
}

fn redefine_sketch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.redefine";
    let key = match p.get("sketch") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, "`sketch` must be an id or name")),
    };
    let id = match s.doc.find_feature(&key) {
        Some(f) if matches!(f.kind, FeatureKind::Sketch { .. }) => f.id,
        _ => return Err(bad(cmd, format!("no sketch `{key}`"))),
    };
    if p.get("plane").is_none() {
        return Err(bad(cmd, "`plane` is required"));
    }
    let feature = s.doc.feature(id).ok_or_else(|| bad(cmd, "sketch disappeared"))?;
    let component = feature.component;
    let index = s.doc.feature_index(id).ok_or_else(|| bad(cmd, "sketch disappeared"))?;
    let author_path = super::sketch::author_path_for_rebind(s, feature, p, cmd)?;
    let marker = s.doc.marker;
    let mut baseline = s.scratch();
    baseline.doc_mut().marker = Some(index + 1);
    baseline.refresh();
    let visible_geometry = baseline.model.state().sketch(id).map(|resolved| resolved.sketch.clone());
    let mut trial = s.scratch();
    trial.active_component = component;
    trial.active_occurrence_path = (!author_path.is_empty()).then_some(author_path);
    // A new attachment may only depend on geometry preceding this sketch.
    trial.doc_mut().marker = Some(index);
    trial.refresh();
    let old_anchor = trial
        .doc
        .feature(id)
        .and_then(|feature| match &feature.kind {
            FeatureKind::Sketch { plane, .. } => super::sketch::face::anchor(plane),
            _ => None,
        })
        .cloned();
    let plane = super::sketch::plane_ref(&trial, p, cmd)?;
    if super::sketch::face::is_face(p) {
        super::sketch::face::capture_positions(&mut trial, &plane)?;
    }
    if let Some(anchor) = old_anchor
        && super::sketch::face::anchor(&plane)
            .is_none_or(|next| next.target != anchor.target || next.component != anchor.component || next.path != anchor.path)
    {
        let sketch = trial.doc_mut().sketch_mut(id)?;
        if let Some(mut visible) = visible_geometry {
            let constraints: std::collections::BTreeMap<_, _> =
                sketch.constraints.iter().map(|constraint| (constraint.id.clone(), constraint.clone())).collect();
            for constraint in &mut visible.constraints {
                if let Some(original) = constraints.get(&constraint.id) {
                    if std::mem::discriminant(&constraint.kind) != std::mem::discriminant(&original.kind) {
                        return Err(bad(cmd, "a dimension role changed during border refresh; repair it explicitly before rebinding"));
                    }
                    // Keep evaluated entity indices after linked topology rebuilds.
                    constraint.param = original.param.clone();
                    constraint.driven = original.driven;
                    constraint.text = original.text;
                    if let Some(value) = original.kind.value() {
                        constraint.kind.set_value(value);
                    }
                }
            }
            *sketch = visible;
        }
        let affected: Vec<_> = sketch
            .links
            .iter()
            .filter(|link| {
                link.auto_face
                    && link.face_name.as_ref() == Some(&anchor.target.name)
                    && matches!(&link.source,solidcraft_sketch::LinkSource::Face {body,..} if *body==anchor.target.body)
            })
            .map(|link| link.id.clone())
            .collect();
        for link in affected {
            sketch.break_link(&link)?;
        }
    }
    if let Some(FeatureKind::Sketch { plane: slot, .. }) = trial.doc_mut().feature_mut(id).map(|feature| &mut feature.kind) {
        *slot = plane;
    }
    trial.doc_mut().marker = Some(index + 1);
    trial.refresh();
    if let Some(error) = trial.model.result(id).and_then(|result| result.error.as_ref()) {
        return Err(bad(cmd, error.clone()));
    }
    trial.doc_mut().marker = marker;
    trial.refresh();
    s.doc = trial.doc;
    s.pending_moves = trial.pending_moves;

    let attachment = s.doc.feature(id).and_then(|feature| match &feature.kind {
        FeatureKind::Sketch { plane, .. } => Some(if super::sketch::face::anchor(plane).is_some() {
            "exact_face"
        } else if solidcraft_doc::sheet::plane_sheet_face(plane).is_some() {
            "sheet_face"
        } else {
            "fixed"
        }),
        _ => None,
    });
    Ok(json!({ "sketch": id,"attachment":attachment }))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Session;

    fn run(s: &mut Session, id: &str, p: Value) -> Value {
        match s.execute(id, &p) {
            Ok(v) => v,
            Err(e) => panic!("{id} {p}: {e}"),
        }
    }

    fn names(s: &Session) -> Vec<String> {
        s.model.state().bodies.iter().map(|b| b.name.clone()).collect()
    }

    #[test]
    fn renamed_body_keeps_its_users() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0]}));
        let [a, b] = [names(&s)[0].clone(), names(&s)[1].clone()];
        run(&mut s, "solid.move", json!({"bodies": [b.clone()], "translate": [0, 0, 5]}));
        run(&mut s, "body.rename", json!({"body": b, "name": "Lid"}));
        assert_eq!(names(&s), vec![a.clone(), "Lid".to_string()]);
        assert!(s.model.results.iter().all(|r| r.error.is_none()), "the move still finds its body");
        // Duplicate names and unknown bodies are refused; undo restores the old name.
        assert!(s.execute("body.rename", &json!({"body": "Lid", "name": a})).is_err());
        assert!(s.execute("body.rename", &json!({"body": "Nope", "name": "X"})).is_err());
        assert!(s.execute("body.rename", &json!({"body": "Lid", "name": " "})).is_err());
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(names(&s)[1], b);
    }

    /// #1108: a primitive's `targets` are its participants, kept beside its kind; a rename
    /// follows them, as it follows the names inside the kind (a combine's target and tools).
    #[test]
    fn renamed_body_keeps_the_primitives_that_target_it() {
        let mut s = Session::default();
        let total = |s: &mut Session| run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap();
        run(&mut s, "solid.box", json!({"length": 20, "width": 20, "height": 10, "name": "Block", "body_name": "A"}));
        run(&mut s, "solid.cylinder", json!({"diameter": 5, "height": 12, "base": [5, 5, -1], "operation": "cut", "targets": ["A"], "name": "Hole"}));
        run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [20, 0, 0], "operation": "new", "body_name": "Tab"}));
        run(&mut s, "solid.combine", json!({"target": "A", "tools": ["Tab"], "operation": "join", "name": "Joined"}));
        let want = total(&mut s);
        run(&mut s, "body.rename", json!({"body": "A", "name": "B"}));
        assert!(
            s.model.results.iter().all(|r| r.error.is_none()),
            "{:?}",
            s.model.results.iter().filter_map(|r| r.error.clone()).collect::<Vec<_>>()
        );
        assert_eq!(s.doc.find_feature("Hole").unwrap().participants, vec!["B".to_string()]);
        assert!((total(&mut s) - want).abs() < 1e-6);
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.find_feature("Hole").unwrap().participants, vec!["A".to_string()]);
    }

    /// #1109: renaming counted bodies (`Body1`, `Body2` of one pattern) leaves the counted bodies
    /// of later features as they were (`Body3`, `Body4`), and what names them keeps working.
    #[test]
    fn renaming_counted_bodies_keeps_later_counted_names() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "center": [0, 0, 2.5], "name": "B1", "body_name": "First"}));
        run(&mut s, "solid.pattern.rectangular", json!({"features": ["B1"], "dir1": [1, 0, 0], "count1": 3, "spacing1": 10, "name": "P1"}));
        run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "center": [0, 20, 2.5], "name": "B2", "body_name": "Second"}));
        run(&mut s, "solid.pattern.rectangular", json!({"features": ["B2"], "dir1": [1, 0, 0], "count1": 3, "spacing1": 10, "name": "P2"}));
        run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "center": [10, 25, 2.5], "operation": "new", "name": "Bar"}));
        let before = names(&s);
        assert!(["Body1", "Body2", "Body3", "Body4", "Body5"].iter().all(|n| before.contains(&n.to_string())), "{before:?}");
        run(&mut s, "solid.combine", json!({"target": "Body3", "tools": ["Body5"], "operation": "join", "name": "Joined"}));
        // Renaming a body that already has a name of its own stores nothing elsewhere.
        run(&mut s, "body.rename", json!({"body": "Second", "name": "Row"}));
        assert!(s.doc.find_feature("P2").unwrap().body_names.is_empty());
        run(&mut s, "body.rename", json!({"body": "Body1", "name": "FirstCopyA"}));
        run(&mut s, "body.rename", json!({"body": "Body2", "name": "FirstCopyB"}));
        assert!(
            s.model.results.iter().all(|r| r.error.is_none()),
            "{:?}",
            s.model.results.iter().filter_map(|r| r.error.clone()).collect::<Vec<_>>()
        );
        let after = names(&s);
        assert!(after.contains(&"Body3".to_string()) && after.contains(&"Body4".to_string()), "{after:?}");
        assert!(!after.contains(&"Body1".to_string()) && !after.contains(&"Body5".to_string()), "{after:?}");
        run(&mut s, "edit.undo", json!({}));
        assert!(names(&s).contains(&"Body2".to_string()) && names(&s).contains(&"Body4".to_string()), "{:?}", names(&s));
    }

    #[test]
    fn renaming_a_body_leaves_other_text_with_its_name_alone() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        let body = names(&s)[0].clone();
        run(&mut s, "solid.move", json!({"bodies": [body.clone()], "translate": [0, 0, 5]}));
        // Sketch text that happens to spell the body's name is not a reference to it.
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Label"}));
        run(&mut s, "sketch.text", json!({"text": body.clone(), "at": [30, 30], "height": 5}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "body.rename", json!({"body": body.clone(), "name": "Lid"}));
        let kind = |name: &str| serde_json::to_value(&s.doc.find_feature(name).unwrap().kind).unwrap();
        let label = kind("Label").to_string();
        assert!(label.contains(&format!("\"{body}\"")) && !label.contains("\"Lid\""), "{label}");
        let moved = s.doc.features.iter().find_map(|f| match &f.kind {
            solidcraft_doc::FeatureKind::Move { bodies, .. } => Some(bodies.clone()),
            _ => None,
        });
        assert_eq!(moved, Some(vec![solidcraft_doc::BodyName::from("Lid")]), "the move follows the rename");
        assert!(s.model.results.iter().all(|r| r.error.is_none()));
    }

    #[test]
    fn redefined_sketch_moves_its_extrude() {
        let mut s = Session::default();
        let sk = run(&mut s, "sketch.create", json!({"plane": "XY"}))["sketch"].as_u64().unwrap();
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [10, 20]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance": 5}));
        let zmax = |s: &mut Session| run(s, "inspect.measure", json!({}))["bodies"][0]["bbox"]["max"][2].as_f64().unwrap();
        assert!((zmax(&mut s) - 5.0).abs() < 1e-6);
        run(&mut s, "sketch.redefine", json!({"sketch": sk, "plane": "XZ"}));
        let z = zmax(&mut s);
        assert!(z.abs() < 1e-6, "positive sketch Y on XZ extends toward -Z: {z}");
        let ymin = run(&mut s, "inspect.measure", json!({}))["bodies"][0]["bbox"]["min"][2].as_f64().unwrap();
        assert!((ymin + 20.0).abs() < 1e-6);
        assert!(s.execute("sketch.redefine", &json!({"sketch": sk, "plane": "nope"})).is_err());
        assert!(s.execute("sketch.redefine", &json!({"sketch": 999, "plane": "XY"})).is_err());
    }

    #[test]
    fn design_units_change_without_moving_geometry() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 20, "width": 10, "height": 5}));
        let vol = |s: &mut Session| run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap();
        let v0 = vol(&mut s);
        run(&mut s, "document.units", json!({"units": "in"}));
        assert_eq!(s.doc.units, "in");
        assert!((vol(&mut s) - v0).abs() < 1e-6, "feature inputs keep their millimetres");
        assert!(s.execute("document.units", &json!({"units": "furlong"})).is_err());
        // A unit-less parameter used as a length keeps its millimetres; a plain count stays a number.
        let mut s = Session::default();
        run(&mut s, "parameters.add", json!({"name": "w", "expression": "5", "unit": ""}));
        run(&mut s, "parameters.add", json!({"name": "n", "expression": "3", "unit": ""}));
        run(&mut s, "solid.box", json!({"length": "w", "width": 10, "height": 5}));
        let b = s.model.state().bodies[0].name.clone();
        run(&mut s, "solid.pattern.rectangular", json!({"bodies": [b], "dir1": [1, 0, 0], "count1": "n", "spacing1": 20}));
        let v0 = vol(&mut s);
        let r = run(&mut s, "document.units", json!({"units": "cm"}));
        assert!((vol(&mut s) - v0).abs() < 1e-6, "{r}");
        assert!(s.doc.param("n").is_some_and(|p| p.expr == "3"), "the count stays bare");
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.units, "mm");
    }

    #[test]
    fn a_bare_width_keeps_its_inches_when_the_design_goes_metric() {
        let mut s = Session::default();
        run(&mut s, "document.units", json!({"units": "in"}));
        run(&mut s, "parameters.add", json!({"name": "w", "expression": "2", "unit": ""}));
        run(&mut s, "solid.box", json!({"length": 10, "width": "w", "height": 5}));
        let width = |s: &mut Session| {
            let b = &run(s, "inspect.measure", json!({}))["bodies"][0]["bbox"];
            b["max"][1].as_f64().unwrap() - b["min"][1].as_f64().unwrap()
        };
        assert!((width(&mut s) - 50.8).abs() < 1e-6, "2 in an inch design");
        let r = run(&mut s, "document.units", json!({"units": "mm"}));
        assert!((width(&mut s) - 50.8).abs() < 1e-6, "still 50.8 mm: {r}");
        assert_eq!(r["kept"], json!(["w"]));
        assert_eq!(s.doc.param("w").map(|p| p.expr.as_str()), Some("2 in"));
    }

    #[test]
    fn applying_a_named_view_or_camera_moves_the_view() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 40, "width": 20, "height": 10}));
        run(&mut s, "view.save", json!({"name": "Photo", "camera": {"target": [1, 2, 3], "yaw": 0.5, "pitch": -0.6, "distance": 150}}));
        let undo = s.undo.len();
        assert!(s.camera.is_none());
        let r = run(&mut s, "view.apply", json!({"view": "Photo"}));
        let c = s.camera.expect("a camera for the view");
        assert_eq!((c.target, c.yaw, c.pitch, c.distance), (solidcraft_geom::Vec3::new(1.0, 2.0, 3.0), 0.5, -0.6, 150.0));
        assert_eq!(r["camera"]["yaw"], 0.5);
        // A standard view looks from its direction and frames the model.
        run(&mut s, "view.apply", json!({"view": "top"}));
        let c = s.camera.expect("a camera for the top view");
        assert!((c.pitch - std::f64::consts::FRAC_PI_2).abs() < 1e-3 && (c.target.x - 20.0).abs() < 1e-6, "{c:?}");
        run(&mut s, "view.apply", json!({"camera": {"target": [0, 0, 0], "yaw": 1, "pitch": 0.2, "distance": 80, "fov": 0.5}}));
        assert_eq!(s.camera.map(|c| (c.yaw, c.distance, c.fov)), Some((1.0, 80.0, 0.5)));
        assert_eq!(s.undo.len(), undo, "view state, not an undo step");
        for p in [
            json!({}),
            json!({"view": "Nope"}),
            json!({"view": "Photo", "camera": {"target": [0, 0, 0], "yaw": 0, "pitch": 0, "distance": 1}}),
            json!({"camera": {"target": [0, 0, 0], "yaw": 0, "pitch": 0, "distance": 0}}),
            json!({"camera": "x"}),
        ] {
            assert!(s.execute("view.apply", &p).is_err(), "{p}");
        }
    }

    #[test]
    fn named_views_are_saved_renamed_and_deleted() {
        let mut s = Session::default();
        let cam = json!({"target": [1, 2, 3], "yaw": 0.5, "pitch": 0.3, "distance": 100});
        assert_eq!(run(&mut s, "view.save", json!({"camera": cam}))["view"], "Named View1");
        run(&mut s, "view.save", json!({"camera": cam, "name": "Detail"}));
        run(&mut s, "view.rename", json!({"view": "Named View1", "name": "Overview"}));
        assert!(s.execute("view.rename", &json!({"view": "Overview", "name": "Detail"})).is_err());
        let back: solidcraft_doc::Document = serde_json::from_str(&serde_json::to_string(&*s.doc).unwrap()).unwrap();
        assert_eq!(back.named_views.len(), 2);
        assert_eq!(back.named_views[0].target.z, 3.0);
        run(&mut s, "view.delete", json!({"view": "Detail"}));
        assert_eq!(run(&mut s, "view.list", json!({}))["views"].as_array().unwrap().len(), 1);
        for p in [json!({}), json!({"camera": {"target": [0, 0, 0], "yaw": 0, "pitch": 0, "distance": -1}}), json!({"camera": "x"})] {
            assert!(s.execute("view.save", &p).is_err(), "{p}");
        }
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.named_views.len(), 2);
    }

    #[test]
    fn helper_sketches_hide_and_show() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        let floor = run(&mut s, "sketch.create", json!({"plane": "XY"}))["sketch"].as_u64().unwrap();
        run(&mut s, "sketch.line", json!({"points": [[0, 0], [50, 0]], "construction": true}));
        run(&mut s, "sketch.finish", json!({}));
        let drain = run(&mut s, "sketch.create", json!({"plane": "XY"}))["sketch"].as_u64().unwrap();
        run(&mut s, "sketch.point", json!({"point": [25, 5]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "construct.plane.offset", json!({"base": "XY", "offset": 5, "name": "Floor"}));
        let body = s.model.state().bodies[0].name.clone();
        let drain_name = s.doc.feature(drain).unwrap().name.clone();
        let cam = solidcraft_render::Camera::default();
        let lines = |s: &Session| crate::view::scene(s, &cam).lines.len();
        let shown = |s: &mut Session| {
            let d = run(s, "document.inspect", json!({}));
            d["sketches"].as_array().unwrap().iter().filter(|x| x["visible"] == true).count()
        };
        let (n0, undo0, doc0) = (lines(&s), s.undo.len(), s.doc.clone());
        assert_eq!(shown(&mut s), 2);
        // By id and by name; the view loses their lines, the design and its history don't change.
        let r = run(&mut s, "browser.visibility", json!({"items": [floor, drain_name], "visible": false}));
        assert_eq!(r["changed"], 2);
        assert_eq!(shown(&mut s), 0);
        assert!(lines(&s) < n0);
        assert!(std::sync::Arc::ptr_eq(&s.doc, &doc0) && s.undo.len() == undo0, "view state, not a design change");
        run(&mut s, "browser.visibility", json!({"folder": "sketches", "visible": true}));
        assert_eq!(lines(&s), n0);
        // Bodies (following a rename) and construction planes.
        run(&mut s, "browser.visibility", json!({"items": [body.clone(), "Floor", "xy"], "visible": false}));
        assert!(crate::view::scene(&s, &cam).meshes.is_empty());
        assert_eq!(s.visibility.hidden_origin, vec!["Floor".to_string(), "XY".to_string()]);
        run(&mut s, "body.rename", json!({"body": body, "name": "Tray"}));
        assert_eq!(s.visibility.hidden_bodies, vec!["Tray".to_string()]);
        let d = run(&mut s, "document.inspect", json!({}));
        assert_eq!(d["bodies"][0]["visible"], false);
        // Hostile input is refused and changes nothing.
        let before = s.visibility.clone();
        for p in [
            json!({"items": [floor]}),
            json!({"items": [floor], "visible": "no"}),
            json!({"items": [floor, "Nope"], "visible": false}),
            json!({"items": [], "visible": false}),
            json!({"items": "Floor", "visible": false}),
            json!({"folder": "sketches", "items": [floor], "visible": false}),
            json!({"folder": "nope", "visible": false}),
            json!({"visible": false}),
        ] {
            assert!(s.execute("browser.visibility", &p).is_err(), "{p}");
        }
        assert_eq!(s.visibility, before);
    }

    #[test]
    fn groups_hold_items_and_follow_renames() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0]}));
        let [a, b] = [names(&s)[0].clone(), names(&s)[1].clone()];
        let g = run(&mut s, "browser.group", json!({"folder": "bodies", "items": [a.clone()]}))["group"].as_u64().unwrap();
        assert_eq!(s.doc.browser_groups[0].name, "Group1");
        run(&mut s, "browser.move", json!({"folder": "bodies", "items": [b.clone()], "group": g, "before": a.clone()}));
        assert_eq!(s.doc.browser_groups[0].items, vec![b.clone(), a.clone()]);
        run(&mut s, "browser.rename_group", json!({"group": g, "name": "Fasteners"}));
        run(&mut s, "body.rename", json!({"body": a, "name": "Bolt"}));
        assert_eq!(s.doc.browser_groups[0].items, vec![b.clone(), "Bolt".to_string()]);
        // Out of the group, then a folder order; saved with the design.
        run(&mut s, "browser.move", json!({"folder": "bodies", "items": [b.clone()]}));
        assert_eq!(s.doc.browser_groups[0].items, vec!["Bolt".to_string()]);
        run(&mut s, "browser.order", json!({"folder": "bodies", "order": [b.clone(), "Bolt"]}));
        let saved: solidcraft_doc::Document = serde_json::from_str(&serde_json::to_string(&*s.doc).unwrap()).unwrap();
        assert_eq!(saved.browser_groups, s.doc.browser_groups);
        assert_eq!(saved.browser_order["0/bodies"], vec![b, "Bolt".to_string()]);
        run(&mut s, "browser.ungroup", json!({"group": g}));
        assert!(s.doc.browser_groups.is_empty());
        // Hostile input is refused.
        for (c, p) in [
            ("browser.group", json!({"folder": "nope"})),
            ("browser.group", json!({"folder": "bodies", "component": 99})),
            ("browser.group", json!({"folder": "bodies", "items": "x"})),
            ("browser.move", json!({"folder": "bodies", "items": []})),
            ("browser.move", json!({"folder": "bodies", "items": ["x"], "group": 42})),
            ("browser.ungroup", json!({"group": -1})),
            ("browser.rename_group", json!({"group": g, "name": "x"})),
        ] {
            assert!(s.execute(c, &p).is_err(), "{c} {p}");
        }
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.browser_groups.len(), 1, "ungroup undoes");
    }
}
