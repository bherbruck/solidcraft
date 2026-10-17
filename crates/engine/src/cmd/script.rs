//! A design's history as a command script (#886): `timeline.export_script` writes the steps
//! that rebuild the design (`solidcraft-cli eval` / `run` replay them), and `timeline.add`
//! appends one recorded feature, as the design file holds it, so every feature kind replays
//! exactly: same ids, names, parameter names and stored edge and face names. Trimming the
//! script after a step gives the design as it was at that step.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use solidcraft_doc::{Document, Feature, FeatureKind, Param, expr};

use super::CommandSpec;
use crate::params::{bad, str_};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("timeline.add", "Add Recorded Feature", add).icon("new").params(
        "feature: a feature as the design file and timeline.inspect hold it ({type, name?, id?, …}; id and name are kept when free, \
         so later steps can refer to them); params?: [{name, expression, unit?, comment?}] (the model parameters its sketch \
         dimensions use); the feature goes at the marker → feature, name, error, warning",
    ),
    CommandSpec::new("timeline.export_script", "Export Command Script", export_script).icon("export").noundo().doc_only().params(
        "path?: write the script to this file → script: {commands: [...]} that rebuilds the design (solidcraft-cli eval/run), \
         steps, omitted: what the script leaves out (appearances, folders, views, configurations); components and assemblies \
         are not supported yet",
    ),
];

/// Most model parameters one `timeline.add` step may bring (a sketch's dimensions).
const MAX_STEP_PARAMS: usize = solidcraft_sketch::MAX_CONSTRAINTS;

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.add";
    let raw = p
        .get("feature")
        .and_then(Value::as_object)
        .ok_or_else(|| bad(cmd, "`feature` must be a feature object, as a design file or timeline.inspect has it"))?;
    let feature = recorded_feature(cmd, raw)?;
    let params = step_params(cmd, p.get("params"))?;
    let id = s.doc_mut().insert_feature(feature, params)?;
    s.refresh();
    let name = s.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default();
    let r = s.model.result(id);
    Ok(json!({"feature": id, "name": name, "error": r.and_then(|r| r.error.clone()), "warning": r.and_then(|r| r.warning.clone())}))
}

/// The feature, through the checks a design file gets when it opens (numbers, sketch sizes, 3D
/// curves, face attachments…), so a script step can hold nothing a design could not.
fn recorded_feature(cmd: &str, raw: &Map<String, Value>) -> Result<Feature> {
    let mut raw = raw.clone();
    // A hand-written step may leave out the id and the name: they are then given new ones.
    raw.entry("id").or_insert(json!(0));
    raw.entry("name").or_insert(json!(""));
    // Opening fills in missing input names with the next free `dN` of the one-feature design
    // below; the real design names them instead (`Document::insert_feature`).
    let inputs = match raw.get("param_names") {
        None => Vec::new(),
        Some(Value::Array(a)) if a.len() <= 1000 => {
            a.iter().map(|n| n.as_str().map(str::to_string)).collect::<Option<Vec<_>>>().ok_or_else(|| bad(cmd, "`param_names` must be names"))?
        }
        Some(_) => return Err(bad(cmd, "`param_names` must be a list of names")),
    };
    let design = json!({
        "format": solidcraft_doc::format::format_string(solidcraft_doc::format::FORMAT),
        "name": "",
        "params": [],
        "features": [Value::Object(raw)],
    });
    let mut d = Document::from_json(&design.to_string()).map_err(|e| bad(cmd, format!("invalid feature: {e}")))?;
    let mut f = d.features.pop().ok_or_else(|| bad(cmd, "invalid feature"))?;
    f.param_names = inputs;
    Ok(f)
}

fn step_params(cmd: &str, v: Option<&Value>) -> Result<Vec<Param>> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
    let list = v.as_array().filter(|a| a.len() <= MAX_STEP_PARAMS).ok_or_else(|| bad(cmd, "`params` must be a list of parameters"))?;
    let text = |p: &Value, k: &str| match p.get(k) {
        None | Some(Value::Null) => Some(String::new()),
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) if k == "expression" => n.as_f64().filter(|x| x.is_finite()).map(|x| x.to_string()),
        Some(_) => None,
    };
    let mut out = Vec::with_capacity(list.len());
    for p in list {
        let name = str_(p, "name").ok_or_else(|| bad(cmd, "each parameter needs a `name`"))?;
        let expr =
            text(p, "expression").filter(|e| !e.trim().is_empty()).ok_or_else(|| bad(cmd, format!("parameter `{name}` needs an `expression`")))?;
        let unit = text(p, "unit").ok_or_else(|| bad(cmd, format!("parameter `{name}`: `unit` must be text")))?;
        let comment = text(p, "comment").ok_or_else(|| bad(cmd, format!("parameter `{name}`: `comment` must be text")))?;
        out.push(Param { name: name.trim().to_string(), expr: expr.trim().to_string(), unit: unit.trim().to_string(), comment, model: true });
    }
    Ok(out)
}

fn export_script(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.export_script";
    let (commands, omitted) = script_steps(&s.doc)?;
    let steps = commands.len();
    let script = json!({
        "about": {
            "design": s.doc.name,
            "format": solidcraft_doc::format::format_string(solidcraft_doc::format::FORMAT),
            "version": env!("CARGO_PKG_VERSION"),
            "replay": "solidcraft-cli eval script.json (timeline.add steps need a build that reads this design format)",
        },
        "commands": commands,
    });
    let path = str_(p, "path");
    if let Some(path) = path {
        let text = serde_json::to_string_pretty(&script).map_err(|e| EngineError::Other(e.to_string()))?;
        solidcraft_io::vfs::write(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    } else if p.get("path").is_some_and(|v| !v.is_null()) {
        return Err(bad(cmd, "`path` must be a file path"));
    }
    Ok(json!({"script": script, "steps": steps, "omitted": omitted, "path": path}))
}

fn step(command: &str, params: Value) -> Value {
    json!({"command": command, "params": params})
}

fn not_supported(what: &str) -> EngineError {
    EngineError::Other(format!("not supported yet: a command script of a design with {what} (#886)"))
}

/// The steps that rebuild `doc` on an empty design, and what they leave out.
pub(crate) fn script_steps(doc: &Document) -> Result<(Vec<Value>, Vec<String>)> {
    if !doc.components.is_empty() || !doc.occurrences.is_empty() || !doc.parts.is_empty() || !doc.links.is_empty() {
        return Err(not_supported("components"));
    }
    if !doc.assembly.is_empty() {
        return Err(not_supported("joints"));
    }
    if !doc.body_components.is_empty()
        || !doc.body_offsets.is_empty()
        || doc.features.iter().any(|f| f.component != 0 || matches!(f.kind, FeatureKind::BodyTransfer { .. }))
    {
        return Err(not_supported("bodies moved between components"));
    }
    let mut omitted = Vec::new();
    for (empty, what) in [
        (doc.appearances.is_empty(), "appearances (colours of bodies and faces)"),
        (doc.configs.is_empty(), "configurations"),
        (doc.canvases.is_empty(), "canvases (reference images)"),
        (doc.named_views.is_empty(), "named views"),
        (doc.browser_groups.is_empty() && doc.browser_order.is_empty(), "Browser folders and order"),
        (doc.timeline_groups.is_empty(), "timeline folders"),
        (doc.visibility.is_empty(), "hidden items"),
    ] {
        if !empty {
            omitted.push(what.to_string());
        }
    }

    let mut out = Vec::new();
    if doc.units != "mm" {
        out.push(step("document.units", json!({"units": doc.units})));
    }
    // Design properties, in one step. Keys `document.metadata.set` would refuse (a well-known
    // key holding something other than text, a key another tool wrote outside the rules) are
    // listed as left out instead of failing the replay.
    let mut properties = solidcraft_doc::Metadata::default();
    let mut odd = Vec::new();
    for (key, value) in &doc.metadata.0 {
        if properties.set(key, value.clone()).is_err() {
            odd.push(key.as_str());
        }
    }
    if !properties.is_empty() {
        out.push(step("document.metadata.set", json!({"values": properties})));
    }
    if !odd.is_empty() {
        omitted.push(format!("design properties document.metadata.set can't set ({})", odd.join(", ")));
    }
    for rule in &doc.sheet.rules {
        let mut p = serde_json::to_value(rule).map_err(|e| EngineError::Other(e.to_string()))?;
        if doc.sheet.active.as_deref() == Some(rule.name.as_str())
            && let Some(o) = p.as_object_mut()
        {
            o.insert("active".into(), json!(true));
        }
        out.push(step("sheet.manage_rules", p));
    }
    if let Some(active) = doc.sheet.active.as_deref().filter(|a| !doc.sheet.rules.iter().any(|r| r.name == *a)) {
        out.push(step("sheet.manage_rules", json!({"name": active, "active": true})));
    }
    for rule in &doc.plastic.rules {
        out.push(step("plastic.manage_rules", serde_json::to_value(rule).map_err(|e| EngineError::Other(e.to_string()))?));
    }
    if let Some(active) = &doc.plastic.active {
        out.push(step("plastic.manage_rules", json!({"name": active, "active": true})));
    }
    for m in &doc.user_materials {
        let mut p = json!({"name": m.name, "density_g_cm3": m.density});
        if let (Some(look), Some(o)) = (&m.appearance, p.as_object_mut()) {
            // A colour of its own carries the material's name; otherwise it is a library look.
            if look.name == m.name {
                o.insert("color".into(), json!(look.color));
            } else {
                o.insert("appearance".into(), json!(look.name));
            }
            o.insert("opacity".into(), json!(look.opacity));
        }
        out.push(step("material.create", p));
    }

    // Each sketch brings the model parameters of its dimensions.
    let models: BTreeMap<&str, &Param> = doc.params.iter().filter(|p| p.model).map(|p| (p.name.as_str(), p)).collect();
    let mut owned: BTreeSet<&str> = BTreeSet::new();
    let mut brings: BTreeMap<u64, Vec<&Param>> = BTreeMap::new();
    for f in &doc.features {
        if let FeatureKind::Sketch { sketch, .. } = &f.kind {
            for name in sketch.constraints.iter().filter_map(|c| c.param.as_deref()) {
                if let Some(p) = models.get(name)
                    && owned.insert(name)
                {
                    brings.entry(f.id).or_default().push(p);
                }
            }
        }
    }
    if let Some(p) = models.keys().find(|n| !owned.contains(*n)) {
        return Err(not_supported(&format!("model parameter `{p}` outside a sketch dimension")));
    }

    // User parameters go in as soon as everything they refer to exists, so each step sees the
    // values the design has (parameters are global; the order only has to let each one evaluate).
    let mut known: BTreeSet<String> = BTreeSet::new();
    let mut pending: Vec<&Param> = doc.params.iter().filter(|p| !p.model).collect();
    let flush = |pending: &mut Vec<&Param>, known: &mut BTreeSet<String>, out: &mut Vec<Value>| {
        while let Some(i) = pending.iter().position(|p| expr::references(&p.expr).iter().all(|r| known.contains(r))) {
            let p = pending.remove(i);
            let mut params = json!({"name": p.name, "expression": p.expr, "unit": p.unit});
            if let Some(o) = params.as_object_mut() {
                if !p.comment.is_empty() {
                    o.insert("comment".into(), json!(p.comment));
                }
                if doc.favorites.contains(&p.name) {
                    o.insert("favorite".into(), json!(true));
                }
            }
            out.push(step("parameters.add", params));
            known.insert(p.name.clone());
        }
    };
    for f in &doc.features {
        flush(&mut pending, &mut known, &mut out);
        let feature = serde_json::to_value(f).map_err(|e| EngineError::Other(e.to_string()))?;
        let mut params = json!({"feature": feature});
        if let (Some(list), Some(o)) = (brings.get(&f.id), params.as_object_mut()) {
            let list: Vec<Value> = list
                .iter()
                .map(|p| {
                    let mut v = json!({"name": p.name, "expression": p.expr, "unit": p.unit});
                    if let (false, Some(o)) = (p.comment.is_empty(), v.as_object_mut()) {
                        o.insert("comment".into(), json!(p.comment));
                    }
                    v
                })
                .collect();
            o.insert("params".into(), Value::Array(list));
            known.extend(brings.get(&f.id).into_iter().flatten().map(|p| p.name.clone()));
        }
        known.extend(f.param_names.iter().filter(|n| !n.is_empty()).cloned());
        out.push(step("timeline.add", params));
    }
    flush(&mut pending, &mut known, &mut out);
    if let Some(p) = pending.first() {
        return Err(EngineError::Other(format!(
            "parameter `{}` refers to a parameter this design does not define (or to itself): fix it before exporting a script",
            p.name
        )));
    }

    // Comments and favourites of feature inputs and dimensions.
    for (name, comment) in &doc.param_comments {
        out.push(step("parameters.comment", json!({"name": name, "comment": comment})));
    }
    let users: BTreeSet<&str> = doc.params.iter().filter(|p| !p.model).map(|p| p.name.as_str()).collect();
    for name in doc.favorites.iter().filter(|n| !users.contains(n.as_str())) {
        out.push(step("parameters.favorite", json!({"name": name, "favorite": true})));
    }

    // Materials and plastic rules of bodies, once the bodies exist.
    let mut by_material: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (body, material) in &doc.materials {
        by_material.entry(material.as_str()).or_default().push(body.as_str());
    }
    for (material, bodies) in by_material {
        out.push(step("material.assign", json!({"bodies": bodies, "material": material})));
    }
    let mut by_rule: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (body, rule) in &doc.plastic.assigned {
        by_rule.entry(rule.as_str()).or_default().push(body.as_str());
    }
    for (rule, bodies) in by_rule {
        out.push(step("plastic.assign_rule", json!({"bodies": bodies, "rule": rule})));
    }

    if let Some(m) = doc.marker.filter(|m| *m < doc.features.len()) {
        out.push(step("timeline.rollback", json!({"position": m})));
    }
    Ok((out, omitted))
}
