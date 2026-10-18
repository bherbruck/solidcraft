//! Part properties and the bill of materials. `component.metadata` and
//! `component.metadata.set` read and change a component definition's part number, description,
//! revision and free properties (shared by every occurrence, saved with the component);
//! `assembly.bom` counts the placed components of the design, or of one component, and lists
//! them as rows, optionally written to a CSV or JSON file. With `group_children`, a part with a
//! part number keeps the parts placed inside it (a bolt's washers and nut) in its own row.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Map, Value, json};
use solidcraft_doc::metadata::{DESCRIPTION, MAX_KEYS, MAX_TEXT_BYTES, PART_NUMBER, REVISION, WELL_KNOWN};

use super::CommandSpec;
use crate::params::{bad, str_};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("component.metadata", "Part Properties", read).icon("settings").noundo().doc_only().params(
        "component: id or name (0 or \"root\": the design's own properties) → component, name, part_number, description, revision, \
         metadata (as saved, other tools' keys too), properties: {name: text}",
    ),
    CommandSpec::new("component.metadata.set", "Change Part Properties", set).icon("settings").doc_only().params(
        "component: id or name (not the root: use document.metadata.set); part_number?, description?, revision?: text (empty or null \
         removes); values?: {key: value} (SolidCraft's keys project, part_number, revision, author, organization, description, \
         license, or a reverse-DNS key holding an object); properties?: {name: text, …} (free properties; empty or null removes). \
         All or nothing, one undo step → component, metadata, properties, changed: [keys]",
    ),
    CommandSpec::new("assembly.bom", "Bill of Materials", bom).at("SOLID", "INSPECT").icon("bom").noundo().params(
        "component?: id or name (default the design); levels?: flat (default: every part at any depth, counted through its \
         subassemblies) | top (the component's direct children, subassemblies as one row each); group_by?: [\"part_number\"] \
         (default; components without a part number get a row per definition) | [\"definition\"]; group_children?: true (a component \
         with a part number is one row, the parts placed inside it listed under it as children: [{item, part_number, description, \
         revision, each, count, definition}], not as rows of their own); path?: write a .csv or .json file (children as items 1.1, \
         1.2…); format?: csv|json (default from the path) → rows: [{item, part_number, description, revision, count, definition, \
         components, properties, children?}] sorted by part number, total_occurrences (children included)",
    ),
];

/// Longest free property name, in bytes.
const MAX_PROPERTY_NAME_BYTES: usize = 128;
/// How deep, and over how many placements, the bill of materials walks.
const MAX_DEPTH: usize = 64;
const MAX_PLACEMENTS: usize = 100_000;

fn key(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn component_id(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    let k = key(p.get("component")).ok_or_else(|| bad(cmd, "`component` must be a component id or name"))?;
    s.doc.find_component(&k).ok_or_else(|| bad(cmd, format!("no component `{k}`")))
}

fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "component.metadata";
    let id = component_id(s, p, cmd)?;
    if id == 0 {
        let m = &s.doc.metadata;
        return Ok(json!({"component": 0, "name": s.doc.name, "part_number": m.part_number(), "description": m.description(),
            "revision": m.revision(), "metadata": m, "properties": {}}));
    }
    let c = s.doc.components.iter().find(|c| c.id == id).ok_or_else(|| bad(cmd, format!("no component `{id}`")))?;
    let m = &c.metadata;
    Ok(json!({"component": id, "name": c.name, "part_number": m.part_number(), "description": m.description(),
        "revision": m.revision(), "metadata": m, "properties": c.properties}))
}

/// A free property as it may be saved (`None`: remove it), or why not.
fn property(name: &str, value: &Value, cmd: &str) -> Result<Option<String>> {
    let shown: String = name.chars().take(64).collect();
    if name.trim().is_empty() || name.len() > MAX_PROPERTY_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(bad(cmd, format!("property name `{shown}` must be 1…{MAX_PROPERTY_NAME_BYTES} bytes of text without control characters")));
    }
    let text = match value {
        Value::Null => return Ok(None),
        Value::String(t) if t.is_empty() => return Ok(None),
        Value::String(t) => t.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return Err(bad(cmd, format!("property `{shown}` takes text"))),
    };
    if text.len() > MAX_TEXT_BYTES {
        return Err(bad(cmd, format!("property `{shown}` is longer than {MAX_TEXT_BYTES} bytes")));
    }
    if text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')) {
        return Err(bad(cmd, format!("property `{shown}` has a control character")));
    }
    Ok(Some(text))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "component.metadata.set";
    let id = component_id(s, p, cmd)?;
    if id == 0 {
        return Err(bad(cmd, "the root is the design: its properties are document.metadata.set's"));
    }
    let c = s.doc.components.iter().find(|c| c.id == id).ok_or_else(|| bad(cmd, format!("no component `{id}`")))?;
    let (mut metadata, mut properties) = (c.metadata.clone(), c.properties.clone());
    let mut entries: Vec<(String, Value)> = Vec::new();
    match p.get("values") {
        None => {}
        Some(Value::Object(v)) if v.len() <= MAX_KEYS => entries.extend(v.iter().map(|(k, v)| (k.clone(), v.clone()))),
        Some(Value::Object(_)) => return Err(bad(cmd, format!("`values` can hold at most {MAX_KEYS} keys"))),
        Some(_) => return Err(bad(cmd, "`values` must be an object: {key: value, …}")),
    }
    for k in [PART_NUMBER, DESCRIPTION, REVISION] {
        if let Some(v) = p.get(k) {
            entries.push((k.to_string(), v.clone()));
        }
    }
    let props: Vec<(&String, &Value)> = match p.get("properties") {
        None => Vec::new(),
        Some(Value::Object(v)) if v.len() <= MAX_KEYS => v.iter().collect(),
        Some(Value::Object(_)) => return Err(bad(cmd, format!("`properties` can hold at most {MAX_KEYS} names"))),
        Some(_) => return Err(bad(cmd, "`properties` must be an object: {name: text, …}")),
    };
    if entries.is_empty() && props.is_empty() {
        return Err(bad(cmd, format!("give part_number, description, revision, values or properties ({})", WELL_KNOWN.join(", "))));
    }
    let mut changed: Vec<String> = Vec::new();
    for (k, v) in entries {
        if metadata.set(&k, v).map_err(|e| bad(cmd, e.to_string()))? && !changed.contains(&k) {
            changed.push(k);
        }
    }
    for (name, value) in props {
        let was = properties.get(name).cloned();
        match property(name, value, cmd)? {
            Some(text) => {
                properties.insert(name.clone(), text);
            }
            None => {
                properties.remove(name);
            }
        }
        if properties.get(name) != was.as_ref() {
            changed.push(format!("properties.{name}"));
        }
    }
    if properties.len() > MAX_KEYS {
        return Err(bad(cmd, format!("a component holds at most {MAX_KEYS} properties")));
    }
    // Only a real change touches the design (an undo step, unsaved changes).
    if !changed.is_empty()
        && let Some(c) = s.doc_mut().components.iter_mut().find(|c| c.id == id)
    {
        c.metadata = metadata;
        c.properties = properties;
    }
    let c = s.doc.components.iter().find(|c| c.id == id);
    Ok(json!({"component": id, "metadata": c.map(|c| &c.metadata), "properties": c.map(|c| &c.properties), "changed": changed}))
}

#[derive(Clone, Copy, PartialEq)]
enum Levels {
    Flat,
    Top,
}

#[derive(Clone, Copy, PartialEq)]
enum Group {
    PartNumber,
    Definition,
}

/// One row being gathered.
struct Row {
    part_number: String,
    description: String,
    revision: String,
    count: u64,
    components: Vec<u64>,
    names: Vec<String>,
    properties: BTreeMap<String, String>,
    /// With `group_children`: what is placed inside one of this row's parts, and how often.
    kit: BTreeMap<u64, u64>,
}

/// What the bill of materials lists and how.
#[derive(Clone, Copy)]
struct Options {
    levels: Levels,
    group: Group,
    /// A part with a part number keeps its sub-parts in its row.
    fold: bool,
}

fn options(p: &Value, cmd: &str) -> Result<Options> {
    let levels = match p.get("levels") {
        None | Some(Value::Null) => Levels::Flat,
        Some(Value::String(l)) if l == "flat" => Levels::Flat,
        Some(Value::String(l)) if l == "top" => Levels::Top,
        Some(_) => return Err(bad(cmd, "`levels` is flat or top")),
    };
    let group = match p.get("group_by") {
        None | Some(Value::Null) => Group::PartNumber,
        Some(Value::String(g)) => group_key(g, cmd)?,
        Some(Value::Array(a)) => match a.as_slice() {
            [Value::String(g)] => group_key(g, cmd)?,
            _ => return Err(bad(cmd, "`group_by` is [\"part_number\"] or [\"definition\"]")),
        },
        Some(_) => return Err(bad(cmd, "`group_by` is [\"part_number\"] or [\"definition\"]")),
    };
    let fold = match p.get("group_children") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`group_children` is true or false")),
    };
    Ok(Options { levels, group, fold })
}

/// The design's structure, looked up once per bill of materials.
struct Index {
    /// What each component places (by its id; 0 is the root).
    children: BTreeMap<u64, Vec<u64>>,
    /// Components with bodies of their own.
    owners: HashSet<u64>,
    /// Components with a part number: with `group_children`, one row however much is inside.
    numbered: HashSet<u64>,
}

impl Index {
    fn new(s: &Session) -> Index {
        let mut children: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for o in &s.doc.occurrences {
            children.entry(o.parent).or_default().push(o.component);
        }
        let numbered = s.doc.components.iter().filter(|c| c.metadata.part_number().is_some_and(|n| !n.is_empty())).map(|c| c.id).collect();
        Index { children, owners: s.doc.body_owners(&s.model.state()), numbered }
    }
}

fn group_key(g: &str, cmd: &str) -> Result<Group> {
    match g {
        "part_number" => Ok(Group::PartNumber),
        "definition" => Ok(Group::Definition),
        o => Err(bad(cmd, format!("can't group by `{o}` (part_number or definition)"))),
    }
}

/// How many times each component is placed below `root`: in `Top`, its direct children; in
/// `Flat`, every placement at any depth (a part in a subassembly placed twice counts twice),
/// not below a numbered part when `fold`.
fn counts(ix: &Index, root: u64, levels: Levels, fold: bool, cmd: &str) -> Result<BTreeMap<u64, u64>> {
    let mut out: BTreeMap<u64, u64> = BTreeMap::new();
    let mut placed = 0usize;
    let mut stack: Vec<(u64, usize)> = vec![(root, 0)];
    while let Some((comp, depth)) = stack.pop() {
        for child in ix.children.get(&comp).into_iter().flatten() {
            placed += 1;
            if depth >= MAX_DEPTH || placed > MAX_PLACEMENTS {
                return Err(bad(cmd, format!("the assembly is deeper than {MAX_DEPTH} levels or has more than {MAX_PLACEMENTS} placements")));
            }
            *out.entry(*child).or_default() += 1;
            if levels == Levels::Flat && !(fold && ix.numbered.contains(child)) {
                stack.push((*child, depth + 1));
            }
        }
    }
    if levels == Levels::Flat {
        // Parts only: a subassembly is listed through its parts, unless it has bodies of its own
        // (or, folding, a part number).
        out.retain(|c, _| !ix.children.contains_key(c) || (fold && ix.numbered.contains(c)) || ix.owners.contains(c));
    }
    Ok(out)
}

/// How a kit reads when rows are compared: its parts by part number (else definition) and count.
fn kit_key(s: &Session, kit: &BTreeMap<u64, u64>) -> String {
    let mut parts: Vec<String> = kit
        .iter()
        .map(|(id, n)| {
            let pn = s.doc.components.iter().find(|c| c.id == *id).and_then(|c| c.metadata.part_number()).unwrap_or_default();
            if pn.is_empty() { format!("#{id}×{n}") } else { format!("{pn}×{n}") }
        })
        .collect();
    parts.sort();
    parts.join("\n")
}

fn rows(s: &Session, counts: &BTreeMap<u64, u64>, group: Group, kits: &BTreeMap<u64, BTreeMap<u64, u64>>) -> Vec<Row> {
    let mut grouped: BTreeMap<(String, String, u64), Row> = BTreeMap::new();
    for (id, n) in counts {
        let Some(c) = s.doc.components.iter().find(|c| c.id == *id) else { continue };
        let m = &c.metadata;
        let part_number = m.part_number().unwrap_or_default().to_string();
        let kit = kits.get(id).cloned().unwrap_or_default();
        // Without a part number a definition is its own row; parts with different contents
        // (a bolt with or without its nut) are different rows.
        let k = match group {
            Group::PartNumber if !part_number.is_empty() => (part_number.clone(), kit_key(s, &kit), 0),
            _ => (String::new(), String::new(), *id),
        };
        let row = grouped.entry(k).or_insert_with(|| Row {
            part_number: part_number.clone(),
            description: String::new(),
            revision: String::new(),
            count: 0,
            components: Vec::new(),
            names: Vec::new(),
            properties: BTreeMap::new(),
            kit,
        });
        row.count += n;
        row.components.push(*id);
        if !row.names.contains(&c.name) {
            row.names.push(c.name.clone());
        }
        if row.description.is_empty() {
            row.description = m.description().unwrap_or_default().to_string();
        }
        if row.revision.is_empty() {
            row.revision = m.revision().unwrap_or_default().to_string();
        }
        for (k, v) in &c.properties {
            row.properties.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let mut out: Vec<Row> = grouped.into_values().collect();
    // By part number; rows without one after them, by name.
    out.sort_by(|a, b| {
        (a.part_number.is_empty(), &a.part_number, &a.names, &a.components).cmp(&(b.part_number.is_empty(), &b.part_number, &b.names, &b.components))
    });
    out
}

/// A CSV field, always quoted.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// The bill of materials as CSV: fixed columns, every field quoted.
fn csv(rows: &[Value]) -> String {
    let mut out = ["item", "part_number", "description", "revision", "count", "definition"].map(quote).join(",");
    out.push('\n');
    let lines = rows.iter().flat_map(|r| std::iter::once(r).chain(r["children"].as_array().into_iter().flatten()));
    for r in lines {
        let text = |k: &str| match &r[k] {
            Value::String(t) => t.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        };
        let fields = ["item", "part_number", "description", "revision", "count", "definition"].map(|k| quote(&text(k)));
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

fn bom(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assembly.bom";
    let root = if p.get("component").is_some_and(|v| !v.is_null()) { component_id(s, p, cmd)? } else { 0 };
    let o = options(p, cmd)?;
    let ix = Index::new(s);
    let counts = counts(&ix, root, o.levels, o.fold, cmd)?;
    // Folding: what one of each numbered part holds, as parts at any depth.
    let mut kits: BTreeMap<u64, BTreeMap<u64, u64>> = BTreeMap::new();
    if o.fold {
        for id in counts.keys().filter(|id| ix.numbered.contains(id)) {
            let kit = self::counts(&ix, *id, Levels::Flat, false, cmd)?;
            if !kit.is_empty() {
                kits.insert(*id, kit);
            }
        }
    }
    let mut total: u64 = 0;
    let rows: Vec<Value> = rows(s, &counts, o.group, &kits)
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            total += r.count;
            let mut v = json!({"item": i + 1, "part_number": r.part_number, "description": r.description, "revision": r.revision,
                "count": r.count, "definition": r.names.join("; "), "components": r.components, "properties": r.properties});
            if !r.kit.is_empty() {
                let children: Vec<Value> = self::rows(s, &r.kit, Group::PartNumber, &BTreeMap::new())
                    .into_iter()
                    .enumerate()
                    .map(|(j, c)| {
                        let count = c.count.saturating_mul(r.count);
                        total += count;
                        json!({"item": format!("{}.{}", i + 1, j + 1), "part_number": c.part_number, "description": c.description,
                            "revision": c.revision, "each": c.count, "count": count, "definition": c.names.join("; ")})
                    })
                    .collect();
                v["children"] = Value::Array(children);
            }
            v
        })
        .collect();
    let mut out = Map::new();
    out.insert("component".into(), json!(root));
    out.insert("levels".into(), json!(if o.levels == Levels::Top { "top" } else { "flat" }));
    out.insert("group_by".into(), json!([if o.group == Group::Definition { "definition" } else { "part_number" }]));
    out.insert("group_children".into(), json!(o.fold));
    if let Some(path) = str_(p, "path") {
        let format = match str_(p, "format").map(str::to_ascii_lowercase) {
            Some(f) => f,
            None if path.to_ascii_lowercase().ends_with(".json") => "json".into(),
            None if path.to_ascii_lowercase().ends_with(".csv") => "csv".into(),
            None => return Err(bad(cmd, "`path` must end in .csv or .json, or give `format`")),
        };
        let text = match format.as_str() {
            "csv" => csv(&rows),
            "json" => serde_json::to_string_pretty(&json!({"rows": rows, "total_occurrences": total})).map_err(|e| bad(cmd, e.to_string()))?,
            other => return Err(bad(cmd, format!("unknown format `{other}` (csv or json)"))),
        };
        solidcraft_io::vfs::write(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        out.insert("path".into(), json!(path));
        out.insert("format".into(), json!(format));
    }
    out.insert("rows".into(), Value::Array(rows));
    out.insert("total_occurrences".into(), json!(total));
    Ok(Value::Object(out))
}

#[cfg(test)]
#[path = "bom_tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
