//! Configurations: the design's configuration table (rows of parameter values and feature
//! suppressions) and switching between its rows.

use serde_json::{Value, json};
use solidcraft_doc::config::{Column, ConfigRow, MAX_COLUMNS, MAX_ROWS};

use super::CommandSpec;
use crate::params::{bad, bool_, str_, string_list};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("config.configure", "Configure", configure).at("SOLID", "CONFIGURE").icon("params").params(
        "params?: [parameter names]; features?: [feature names]; materials?: [body names]; appearances?: [body names] (columns to add; with no rows yet, a \"Default\" row takes the current values)",
    ),
    CommandSpec::new("config.table", "Display Configuration Table", show)
        .at("SOLID", "CONFIGURE")
        .icon("configuration_table")
        .noundo()
        .params("→ columns (param:<name> or suppress:<feature>), rows, the active row and whether the design still matches it"),
    CommandSpec::new("config.column", "Configuration Column", column).icon("configuration_table").params("param? | feature? | material? | appearance? (target name); delete?: bool"),
    CommandSpec::new("config.row", "Configuration Row", row).icon("configuration_table").params(
        "name; from? (row to copy; default current values); values?: {column key: expression, suppression, material name, appearance name or Look object}; rename?; delete?: bool",
    ),
    CommandSpec::new("config.activate", "Activate Configuration", activate).icon("activate").params("row: name — sets its parameter values and suppressions"),
    CommandSpec::new("config.preview", "Preview Configuration", preview).icon("eye").noundo().params("row: name — validates a rebuild without changing the design or history"),
    CommandSpec::new("config.rule", "Configuration Rule", rule).icon("configuration_table").params("rule?: {id,name,enabled,condition,effect}; id? + delete?; id? + index? (ordered move)"),
    CommandSpec::new("config.rules", "Configuration Rules", show).icon("list").noundo().params("ordered typed rules, rows and active state"),
];

fn column_of(s: &Session, p: &Value, cmd: &str) -> Result<Column> {
    if ["param", "feature", "material", "appearance", "component_appearance", "face_appearance"].iter().filter(|k| p.get(**k).is_some()).count() != 1
    {
        return Err(bad(cmd, "give exactly one configuration target"));
    }
    if let Some(value) = p.get("component_appearance") {
        let key = value
            .as_str()
            .map(str::to_string)
            .or_else(|| value.as_u64().map(|id| id.to_string()))
            .ok_or_else(|| bad(cmd, "component name or ID required"))?;
        if bool_(p, "delete") == Some(true)
            && let Some(column) = s.doc.configs.columns.iter().find(|column| matches!(column,Column::ComponentAppearance(id) if id.to_string()==key))
        {
            return Ok(column.clone());
        }
        let id = s
            .doc
            .find_component(&key)
            .or_else(|| s.doc.occurrences.iter().find(|occurrence| occurrence.name == key).map(|occurrence| occurrence.component))
            .filter(|id| *id != 0)
            .ok_or_else(|| bad(cmd, "no component definition"))?;
        return Ok(Column::ComponentAppearance(id));
    }
    if let Some(value) = p.get("face_appearance") {
        if bool_(p, "delete") == Some(true) {
            let key = value.as_str().ok_or_else(|| bad(cmd, "face column key required for deletion"))?;
            return s
                .doc
                .configs
                .columns
                .iter()
                .find(|column| matches!(column, Column::FaceAppearance(_)) && s.doc.config_key(column) == key)
                .cloned()
                .ok_or_else(|| bad(cmd, "no face column"));
        }
        if let Some(target) = value.get("target") {
            solidcraft_doc::config::validate_face_target_json(target).map_err(|error| bad(cmd, error.to_string()))?;
            let target: solidcraft_doc::config::ConfigFaceTarget =
                serde_json::from_value(target.clone()).map_err(|error| bad(cmd, error.to_string()))?;
            let state = s.model.state();
            let body = state.body(&target.body).ok_or_else(|| bad(cmd, "no face definition"))?;
            s.doc.configuration_face_indices(body, &target).map_err(|error| bad(cmd, error.to_string()))?;
            return Ok(Column::FaceAppearance(target));
        }
        let body_name = value.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "face body required"))?;
        let point = value.get("point").and_then(crate::params::vec3).ok_or_else(|| bad(cmd, "finite face point required"))?;
        let world = s.world_state();
        let body = world.body(body_name).ok_or_else(|| bad(cmd, "no face body"))?;
        if body.body.is_mesh() {
            return Err(bad(cmd, "mesh faces have no supported semantic lineage"));
        }
        let index = solidcraft_doc::appearance::face_index_at(body, point).ok_or_else(|| bad(cmd, "no face at picked point"))?;
        let target = s.doc.configuration_face_target(body, index, point).map_err(|error| bad(cmd, error.to_string()))?;
        s.doc.certify_config_face(&target).map_err(|error| bad(cmd, error.to_string()))?;
        s.doc.configuration_face_indices(body, &target).map_err(|error| bad(cmd, error.to_string()))?;
        return Ok(Column::FaceAppearance(target));
    }
    for (key, appearance) in [("material", false), ("appearance", true)] {
        if let Some(body) = str_(p, key) {
            let c = if appearance { Column::Appearance(body.to_string()) } else { Column::Material(body.to_string()) };
            if !s.doc.configs.columns.contains(&c) && s.model.state().body(body).is_none() {
                return Err(bad(cmd, format!("no body `{body}`")));
            }
            return Ok(c);
        }
    }
    if let Some(n) = str_(p, "param") {
        if !s.doc.all_param_exprs().iter().any(|(m, _, _)| m == n) {
            return Err(bad(cmd, format!("no parameter `{n}`")));
        }
        return Ok(Column::Param(n.to_string()));
    }
    let f = str_(p, "feature").ok_or_else(|| bad(cmd, "give `param` or `feature`"))?;
    let id = s.doc.find_feature(f).map(|x| x.id).ok_or_else(|| bad(cmd, format!("no feature `{f}`")))?;
    Ok(Column::Suppress(id))
}

/// Add a column, filling existing rows with the design's current value.
fn add_column(s: &mut Session, c: Column, cmd: &str) -> Result<()> {
    if s.doc.configs.columns.iter().any(|existing| {
        *existing == c || matches!((existing,&c),(Column::FaceAppearance(a),Column::FaceAppearance(b)) if a.body == b.body && a.name == b.name)
    }) {
        return Ok(());
    }
    if s.doc.configs.columns.len() >= MAX_COLUMNS {
        return Err(bad(cmd, "too many columns"));
    }
    let cur = s.doc.config_current(&c).unwrap_or_default();
    let t = &mut s.doc_mut().configs;
    t.columns.push(c);
    for r in &mut t.rows {
        r.cells.push(cur.clone());
    }
    Ok(())
}

fn configure(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "config.configure";
    for n in string_list(p, "params") {
        let c = column_of(s, &json!({ "param": n }), cmd)?;
        add_column(s, c, cmd)?;
    }
    for n in string_list(p, "features") {
        let c = column_of(s, &json!({ "feature": n }), cmd)?;
        add_column(s, c, cmd)?;
    }
    for key in ["materials", "appearances"] {
        for n in string_list(p, key) {
            let field = if key == "materials" { "material" } else { "appearance" };
            let c = column_of(s, &json!({ field: n }), cmd)?;
            add_column(s, c, cmd)?;
        }
    }
    for (plural, singular) in [("component_appearances", "component_appearance"), ("face_appearances", "face_appearance")] {
        if let Some(values) = p.get(plural) {
            let values = values
                .as_array()
                .filter(|values| values.len() <= MAX_COLUMNS)
                .ok_or_else(|| bad(cmd, "configuration targets must be a bounded array"))?;
            for value in values {
                let c = column_of(s, &json!({singular:value}), cmd)?;
                add_column(s, c, cmd)?;
            }
        }
    }
    if s.doc.configs.rows.is_empty() {
        let cells = s.doc.configs.columns.iter().map(|c| s.doc.config_current(c).unwrap_or_default()).collect();
        let t = &mut s.doc_mut().configs;
        t.rows.push(ConfigRow { name: "Default".into(), cells });
        t.active = Some("Default".into());
    }
    show(s, p)
}

fn show(s: &mut Session, _p: &Value) -> Result<Value> {
    let t = &s.doc.configs;
    let keys: Vec<String> = t.columns.iter().map(|c| s.doc.config_key(c)).collect();
    let rows: Vec<Value> = t
        .rows
        .iter()
        .map(|r| json!({"name": r.name, "values": keys.iter().zip(&r.cells).map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>()}))
        .collect();
    let targets: Vec<Value> = t
        .columns
        .iter()
        .map(|column| {
            let label = match column {
                Column::ComponentAppearance(id) => {
                    format!("Component: {}", s.doc.components.iter().find(|c| c.id == *id).map(|c| c.name.clone()).unwrap_or_else(|| id.to_string()))
                }
                Column::FaceAppearance(target) => {
                    format!("Face: {} / {}", target.body, target.name.split_once(':').map(|(_, role)| role).unwrap_or(&target.name))
                }
                _ => s.doc.config_key(column),
            };
            json!({"key":s.doc.config_key(column),"label":label,"target":column})
        })
        .collect();
    let styles = s.doc.configuration_style_status(&s.model.state());
    Ok(
        json!({"columns": keys, "targets":targets, "rows": rows, "rules": t.rules, "active": t.active, "matches": s.doc.configuration_matches() && styles.is_ok(), "style_status":styles.as_ref().ok(),"style_error":styles.err().map(|error|error.to_string())}),
    )
}

fn column(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "config.column";
    let c = column_of(s, p, cmd)?;
    if bool_(p, "delete").unwrap_or(false) {
        let t = &mut s.doc_mut().configs;
        let i = t.columns.iter().position(|x| *x == c).ok_or_else(|| bad(cmd, "not a column of the table"))?;
        t.columns.remove(i);
        for r in &mut t.rows {
            if i < r.cells.len() {
                r.cells.remove(i);
            }
        }
    } else {
        add_column(s, c, cmd)?;
    }
    if !s.doc.configs.rules.is_empty() {
        validate_rows(s, cmd)?;
    }
    show(s, p)
}

fn row(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "config.row";
    let name = str_(p, "name").map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128).ok_or_else(|| bad(cmd, "`name` is required"))?.to_string();
    let idx = s.doc.configs.rows.iter().position(|r| r.name == name);
    if bool_(p, "delete").unwrap_or(false) {
        let i = idx.ok_or_else(|| bad(cmd, format!("no row `{name}`")))?;
        if s.doc.configs.rows.len() == 1 && !s.doc.configs.rules.is_empty() {
            return Err(bad(cmd, "delete rules before deleting the last configuration row"));
        }
        let t = &mut s.doc_mut().configs;
        t.rows.remove(i);
        if t.active.as_deref() == Some(name.as_str()) {
            t.active = None;
        }
        return show(s, p);
    }
    let mut cells: Vec<String> = match (idx, str_(p, "from")) {
        (_, Some(f)) => s.doc.configs.rows.iter().find(|r| r.name == f).map(|r| r.cells.clone()).ok_or_else(|| bad(cmd, format!("no row `{f}`")))?,
        (Some(i), None) => s.doc.configs.rows.get(i).map(|r| r.cells.clone()).unwrap_or_default(),
        (None, None) => s.doc.configs.columns.iter().map(|c| s.doc.config_current(c).unwrap_or_default()).collect(),
    };
    if let Some(vals) = p.get("values") {
        let vals = vals.as_object().ok_or_else(|| bad(cmd, "`values` must map columns to values"))?;
        for (k, v) in vals {
            let mut text = match v {
                Value::String(t) => t.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Object(_) if k.starts_with("appearance:") || k.starts_with("component_appearance:") || k.starts_with("face_appearance:") => {
                    serde_json::to_string(v).map_err(|e| bad(cmd, e.to_string()))?
                }
                _ => return Err(bad(cmd, format!("value for `{k}` must be an expression or true/false"))),
            };
            if text.len() > 4096 {
                return Err(bad(cmd, "value too long"));
            }
            let col = s
                .doc
                .configs
                .columns
                .iter()
                .position(|c| {
                    s.doc.config_key(c) == *k
                        || match c {
                            Column::Param(n) => n == k,
                            Column::Suppress(id) => s.doc.feature(*id).is_some_and(|f| f.name == *k),
                            Column::Material(_) | Column::Appearance(_) | Column::ComponentAppearance(_) | Column::FaceAppearance(_) => false,
                        }
                })
                .ok_or_else(|| bad(cmd, format!("`{k}` is not a column (config.column adds one)")))?;
            match s.doc.configs.columns.get(col) {
                Some(Column::Material(_)) => {
                    text = s.doc.material_cell(&text).map_err(|e| bad(cmd, e.to_string()))?.unwrap_or_else(|| "Default".into());
                }
                Some(Column::Appearance(_) | Column::ComponentAppearance(_) | Column::FaceAppearance(_)) => {
                    text = match solidcraft_doc::config::appearance_cell(&text).map_err(|e| bad(cmd, e.to_string()))? {
                        Some(look) => serde_json::to_string(&look).map_err(|e| bad(cmd, e.to_string()))?,
                        None => String::new(),
                    };
                }
                _ => {}
            }
            if let Some(cell) = cells.get_mut(col) {
                *cell = text;
            }
        }
    }
    let rename = str_(p, "rename").map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128).map(str::to_string);
    if p.get("rename").is_some() && (rename.is_none() || idx.is_none()) {
        return Err(bad(cmd, "rename requires an existing row and a nonempty name up to 128 bytes"));
    }
    if rename.as_ref().is_some_and(|n| n != &name && s.doc.configs.rows.iter().any(|r| &r.name == n)) {
        return Err(bad(cmd, "configuration name already exists"));
    }
    let t = &mut s.doc_mut().configs;
    let validated_name = rename.clone().unwrap_or_else(|| name.clone());
    match idx {
        Some(i) => {
            if let Some(r) = t.rows.get_mut(i) {
                r.cells = cells;
                if let Some(n) = rename {
                    if t.active.as_deref() == Some(r.name.as_str()) {
                        t.active = Some(n.clone());
                    }
                    r.name = n;
                }
            }
        }
        None => {
            if t.rows.len() >= MAX_ROWS {
                return Err(bad(cmd, "too many rows"));
            }
            t.rows.push(ConfigRow { name, cells });
        }
    }
    if !s.doc.configs.rules.is_empty() {
        let mut trial = s.scratch();
        activate(&mut trial, &json!({"row":validated_name})).map_err(|e| bad(cmd, e.to_string()))?;
    }
    show(s, p)
}

fn validate_rows(s: &Session, cmd: &str) -> Result<()> {
    if !s.doc.configs.rules.is_empty() && s.doc.configs.rows.is_empty() {
        return Err(bad(cmd, "rules require at least one configuration row"));
    }
    for row in &s.doc.configs.rows {
        let mut trial = s.scratch();
        activate(&mut trial, &json!({"row":row.name})).map_err(|e| bad(cmd, format!("row `{}`: {e}", row.name)))?;
    }
    Ok(())
}

fn rule(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "config.rule";
    if let Some(value) = p.get("rule") {
        solidcraft_doc::config::validate_rules_json(&json!({"configs":{"rules":[value]}})).map_err(|e| bad(cmd, e.to_string()))?;
        let mut rule: solidcraft_doc::config::ConfigRule = serde_json::from_value(value.clone()).map_err(|e| bad(cmd, e.to_string()))?;
        if rule.length_unit.is_none() {
            rule.length_unit = s
                .doc
                .configs
                .rules
                .iter()
                .find(|existing| existing.id == rule.id)
                .and_then(|existing| existing.length_unit.clone())
                .or_else(|| Some(s.doc.units.clone()));
        }
        let rules = &mut s.doc_mut().configs.rules;
        if let Some(index) = rules.iter().position(|r| r.id == rule.id) {
            rules[index] = rule;
        } else {
            if rules.len() >= solidcraft_doc::config::MAX_RULES {
                return Err(bad(cmd, "too many rules"));
            }
            rules.push(rule);
        }
    } else {
        let id = str_(p, "id").ok_or_else(|| bad(cmd, "give rule or id"))?;
        let rules = &mut s.doc_mut().configs.rules;
        let index = rules.iter().position(|r| r.id == id).ok_or_else(|| bad(cmd, "unknown rule"))?;
        if bool_(p, "delete").unwrap_or(false) {
            rules.remove(index);
        } else {
            let position = p
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .filter(|v| *v < rules.len())
                .ok_or_else(|| bad(cmd, "valid index required"))?;
            let rule = rules.remove(index);
            rules.insert(position, rule);
        }
    }
    validate_rows(s, cmd)?;
    show(s, p)
}

fn activate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "config.activate";
    let r = str_(p, "row").ok_or_else(|| bad(cmd, "`row` is required"))?.to_string();
    let mut trial = s.scratch();
    let trace = trial.doc_mut().apply_configuration_with_trace(&r).map_err(|e| bad(cmd, e.to_string()))?;
    trial.refresh();
    let new_errors: Vec<String> = trial
        .model
        .results
        .iter()
        .filter_map(|result| {
            result
                .error
                .as_ref()
                .filter(|error| s.model.result(result.id).is_none_or(|before| before.error.as_ref() != Some(*error)))
                .map(|error| format!("{}: {error}", result.name))
        })
        .collect();
    if !new_errors.is_empty() {
        return Err(bad(cmd, format!("configuration `{r}` would fail to rebuild: {}", new_errors.join("; "))));
    }
    let styles = trial.doc.configuration_style_status(&trial.model.state()).map_err(|error| bad(cmd, format!("configuration `{r}`: {error}")))?;
    let solved = s.solved_sketches();
    s.doc = trial.doc;
    s.settle_value_edit(&solved)?;
    let errors: Vec<String> = s.model.results.iter().filter_map(|x| x.error.clone()).collect();
    let mut out = show(s, p)?;
    out["errors"] = json!(errors);
    out["trace"] = json!(trace);
    out["styles"] = json!(styles);
    Ok(out)
}

fn preview(s: &mut Session, p: &Value) -> Result<Value> {
    let mut trial = s.scratch();
    let mut out = activate(&mut trial, p)?;
    out["preview"] = json!(true);
    out["parameters"] = json!(trial.doc.param_rows());
    Ok(out)
}

#[cfg(test)]
#[path = "config_certificate_tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod certificate_tests;
#[cfg(test)]
#[path = "config_style_tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod style_tests;

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

    fn volume(s: &mut Session) -> f64 {
        run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap_or(f64::NAN)
    }

    #[test]
    fn configuration_activation_tolerates_identical_existing_errors_but_rejects_reenabled_failure() {
        let mut s = Session::default();
        run(&mut s, "parameters.change", json!({"name":"radius","expression":"10 mm"}));
        run(&mut s, "solid.sphere", json!({"radius":"radius"}));
        let feature = s.doc.features[0].name.clone();
        run(&mut s, "config.configure", json!({"params":["radius"],"features":[feature]}));
        run(&mut s, "config.row", json!({"name":"Existing error","values":{"radius":"-1 mm"}}));
        run(&mut s, "parameters.change", json!({"name":"radius","expression":"-1 mm"}));
        assert!(s.model.results[0].error.is_some());
        let existing = run(&mut s, "config.activate", json!({"row":"Existing error"}));
        assert_eq!(existing["errors"].as_array().unwrap().len(), 1);
        s.doc_mut().features[0].suppressed = true;
        s.refresh();
        assert!(s.model.results[0].error.is_none());
        run(&mut s, "config.row", json!({"name":"Reenabled failure","values":{"radius":"-1 mm",format!("suppress:{feature}"):false}}));
        let before = s.doc.clone();
        assert!(s.execute("config.activate", &json!({"row":"Reenabled failure"})).is_err());
        assert_eq!(s.doc, before);
        assert!(s.model.results[0].error.is_none());
    }

    #[test]
    fn configuration_rows_and_literal_rule_thresholds_keep_physical_units_on_switch() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length":10,"width":10,"height":10}));
        let length = s.doc.features[0].param_names[0].clone();
        run(&mut s, "config.configure", json!({"params":[length]}));
        run(
            &mut s,
            "config.rule",
            json!({"rule":{"id":"limit","name":"Metric stock","condition":{"type":"parameter","parameter":length,"operator":"le","expression":"20"},"effect":{"type":"parameter_range","parameter":length,"minimum":"5","maximum":"20"}}}),
        );
        run(&mut s, "document.units", json!({"units":"in"}));
        assert_eq!(s.doc.configs.rows[0].cells[0], "10 mm");
        let rule = serde_json::to_value(&s.doc.configs.rules[0]).unwrap();
        assert_eq!(rule["condition"]["expression"], "20 mm");
        assert_eq!(rule["effect"]["minimum"], "5 mm");
        let preview = run(&mut s, "config.preview", json!({"row":"Default"}));
        assert_eq!(preview["trace"][0]["matched"], true);
        run(&mut s, "config.activate", json!({"row":"Default"}));
        assert!((volume(&mut s) - 1000.0).abs() < 1e-6);
        assert!(s.doc.configuration_matches());
        run(&mut s, "edit.undo", json!({}));
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.units, "mm");
        assert_eq!(s.doc.configs.rows[0].cells[0], "10");
    }

    #[test]
    fn configuration_mixed_rule_expressions_keep_authoring_units_without_rewriting_source() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length":10,"width":20,"height":10}));
        let length = s.doc.features[0].param_names[0].clone();
        let width = s.doc.features[0].param_names[1].clone();
        run(&mut s, "config.configure", json!({"params":[length,width]}));
        let expression = format!("{width} + 1");
        run(
            &mut s,
            "config.rule",
            json!({"rule":{"id":"drive","name":"Stock extra","condition":{"type":"always"},"effect":{"type":"drive_parameter","parameter":length,"expression":expression}}}),
        );
        run(
            &mut s,
            "config.rule",
            json!({"rule":{"id":"range","name":"Stock bound","condition":{"type":"parameter","parameter":length,"operator":"lt","expression":format!("{width} + 2")},"effect":{"type":"parameter_range","parameter":length,"minimum":width,"maximum":format!("{width} + 2")}}}),
        );
        run(&mut s, "config.activate", json!({"row":"Default"}));
        let before = volume(&mut s);
        assert!((before - 4200.0).abs() < 1e-6);
        run(&mut s, "document.units", json!({"units":"in"}));
        assert!((volume(&mut s) - before).abs() < 1e-6, "matched row rederives drives in their authoring unit");
        assert!(s.doc.configuration_matches());
        let rule = serde_json::to_value(&s.doc.configs.rules[0]).unwrap();
        assert_eq!(rule["effect"]["expression"], expression);
        assert_eq!(rule["length_unit"], "mm");
        let preview = run(&mut s, "config.preview", json!({"row":"Default"}));
        assert_eq!(preview["trace"][1]["matched"], true);
    }

    #[test]
    fn configuration_rule_edits_enforce_existing_rows_and_are_undoable() {
        let mut s = Session::default();
        run(&mut s, "parameters.change", json!({"name":"length","expression":"10 mm"}));
        run(&mut s, "solid.box", json!({"length":"length","width":10,"height":10}));
        run(&mut s, "config.configure", json!({"params":["length"]}));
        let rule = json!({"id":"limit","name":"Length limit","condition":{"type":"always"},"effect":{"type":"parameter_range","parameter":"length","minimum":"5 mm","maximum":"20 mm"}});
        run(&mut s, "config.rule", json!({"rule":rule}));
        let before = s.doc.clone();
        assert!(s.execute("config.row", &json!({"name":"Default","delete":true})).is_err());
        assert_eq!(s.doc, before, "last row remains while rules exist");
        assert!(s.execute("config.row", &json!({"name":"Too large","values":{"length":"30 mm"}})).is_err());
        assert_eq!(s.doc, before);
        let impossible = json!({"id":"limit","name":"Length limit","condition":{"type":"always"},"effect":{"type":"parameter_range","parameter":"length","minimum":"15 mm","maximum":"20 mm"}});
        assert!(s.execute("config.rule", &json!({"rule":impossible})).is_err());
        assert_eq!(s.doc, before);
        let preview = run(&mut s, "config.preview", json!({"row":"Default"}));
        assert_eq!(preview["trace"][0]["name"], "Length limit");
        assert_eq!(preview["trace"][0]["matched"], true);
        assert_eq!(s.doc, before);
        run(&mut s, "config.rule", json!({"id":"limit","delete":true}));
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc, before);
        let reopened = solidcraft_doc::format::read(&s.doc.to_json()).unwrap();
        assert_eq!(reopened.configs.rules, s.doc.configs.rules);
    }

    #[test]
    fn configuration_preview_and_failed_geometry_leave_design_unchanged() {
        let mut s = Session::default();
        run(&mut s, "parameters.change", json!({"name":"length", "expression":"10 mm"}));
        run(&mut s, "solid.box", json!({"length":"length", "width":10, "height":10}));
        run(&mut s, "config.configure", json!({"params":["length"]}));
        run(&mut s, "config.row", json!({"name":"Large", "values":{"length":"20 mm"}}));
        run(&mut s, "config.row", json!({"name":"Degenerate", "values":{"length":"0 mm"}}));
        let before = s.doc.clone();
        let revision = s.revision;
        assert_eq!(run(&mut s, "config.preview", json!({"row":"Large"}))["preview"], true);
        assert_eq!(s.doc, before);
        assert_eq!(s.revision, revision);
        assert!(s.execute("config.preview", &json!({"row":"Degenerate"})).is_err());
        assert_eq!(s.doc, before);
        assert!(s.execute("config.activate", &json!({"row":"Degenerate"})).is_err());
        assert_eq!(s.doc, before);
        assert!((volume(&mut s) - 1000.0).abs() < 1e-6);
        run(&mut s, "config.activate", json!({"row":"Large"}));
        assert!((volume(&mut s) - 2000.0).abs() < 1e-6);
    }

    #[test]
    fn configured_feature_inputs_and_suppression_match_typed_cells() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length":10, "width":10, "height":10}));
        let feature = s.doc.features[0].name.clone();
        let length = s.doc.features[0].param_names[0].clone();
        s.doc_mut().units = "in".into();
        run(&mut s, "config.configure", json!({"params":[length], "features":[feature]}));
        assert_eq!(s.doc.configs.rows[0].cells[0], "10 mm", "Default captures stored feature literals in their base unit");
        run(&mut s, "config.activate", json!({"row":"Default"}));
        assert!((volume(&mut s) - 1000.0).abs() < 1e-6, "captured Default preserves geometry in an inch design");
        run(&mut s, "config.row", json!({"name":"Inches", "values":{length.clone():"2", format!("suppress:{feature}"):"NO"}}));
        run(&mut s, "config.activate", json!({"row":"Inches"}));
        assert_eq!(s.doc.config_current(&solidcraft_doc::config::Column::Param(length.clone())).as_deref(), Some("2 in"));
        assert!(s.doc.configuration_matches());
        assert!((volume(&mut s) - 5080.0).abs() < 1e-6);
        let feature_id = s.doc.features[0].id;
        run(
            &mut s,
            "config.rule",
            json!({"rule":{"id":"inch-condition","name":"Inch condition","condition":{"type":"parameter","parameter":length,"operator":"gt","expression":"5"},"effect":{"type":"drive_suppression","feature":feature_id,"value":true}}}),
        );
        run(
            &mut s,
            "config.rule",
            json!({"rule":{"id":"inch-range","name":"Inch range","condition":{"type":"always"},"effect":{"type":"parameter_range","parameter":length,"minimum":"0","maximum":"5"}}}),
        );
        let preview = run(&mut s, "config.preview", json!({"row":"Inches"}));
        assert_eq!(preview["trace"][0]["matched"], false, "bare comparison means inches for a feature length");
        assert_eq!(preview["trace"][1]["matched"], true);
    }

    #[test]
    fn configured_parameters_cannot_be_deleted_and_deleted_features_prune_columns() {
        let mut s = Session::default();
        run(&mut s, "parameters.change", json!({"name":"spare", "expression":"10 mm"}));
        run(&mut s, "config.configure", json!({"params":["spare"]}));
        assert!(s.doc_mut().remove_param("spare").is_err());
        run(&mut s, "config.column", json!({"param":"spare", "delete":true}));
        s.doc_mut().remove_param("spare").unwrap();
        run(&mut s, "solid.box", json!({"length":10,"width":10,"height":10}));
        let feature = &s.doc.features[0];
        let id = feature.id;
        let length = feature.param_names[0].clone();
        let name = feature.name.clone();
        run(&mut s, "config.configure", json!({"params":[length], "features":[name]}));
        s.doc_mut().delete_feature(id).unwrap();
        assert!(s.doc.configs.columns.is_empty());
        assert!(s.doc.configs.rows.iter().all(|row| row.cells.is_empty()));
    }

    #[test]
    fn configurations_switch_sizes_and_suppressions() {
        let mut s = Session::default();
        run(&mut s, "parameters.change", json!({"name": "len", "expression": "40 mm"}));
        run(&mut s, "solid.box", json!({"length": "len", "width": 20, "height": 10}));
        run(&mut s, "solid.fillet", json!({"edges": [[0, 0, 5]], "radius": 3}));
        let fillet = s.doc.features.last().map(|f| f.name.clone()).unwrap_or_default();
        let t = run(&mut s, "config.configure", json!({"params": ["len"], "features": [fillet]}));
        assert_eq!(t["rows"][0]["name"], "Default");
        run(&mut s, "config.row", json!({"name": "Long", "values": {"len": "80 mm"}}));
        run(&mut s, "config.row", json!({"name": "Sharp", "from": "Long", "values": {fillet.clone(): true}}));
        let filleted = 8000.0 - (9.0 - std::f64::consts::PI * 9.0 / 4.0) * 10.0;
        assert!((volume(&mut s) - filleted).abs() < 0.5);
        run(&mut s, "config.activate", json!({"row": "Long"}));
        assert!((volume(&mut s) - (16000.0 - (9.0 - std::f64::consts::PI * 9.0 / 4.0) * 10.0)).abs() < 0.5, "{}", volume(&mut s));
        run(&mut s, "config.activate", json!({"row": "Sharp"}));
        assert!((volume(&mut s) - 16000.0).abs() < 1e-6);
        let t = run(&mut s, "config.table", json!({}));
        assert_eq!((t["active"].as_str(), t["matches"].as_bool()), (Some("Sharp"), Some(true)), "{t}");
        // Undo goes back to the previous configuration's values.
        run(&mut s, "edit.undo", json!({}));
        assert!(volume(&mut s) > 15000.0 && volume(&mut s) < 16000.0);
        // Renaming the parameter keeps the column; editing it by hand breaks the match.
        run(&mut s, "parameters.rename", json!({"name": "len", "new_name": "length"}));
        let t = run(&mut s, "config.table", json!({}));
        assert_eq!(t["columns"][0], "param:length", "{t}");
        run(&mut s, "parameters.change", json!({"name": "length", "expression": "50 mm"}));
        assert_eq!(run(&mut s, "config.table", json!({}))["matches"], false);
        // Bad input.
        assert!(s.execute("config.row", &json!({"name": "X", "values": {"nope": 1}})).is_err());
        assert!(s.execute("config.activate", &json!({"row": "Missing"})).is_err());
        run(&mut s, "config.row", json!({"name": "Broken", "values": {"length": "1/"}}));
        assert!(s.execute("config.activate", &json!({"row": "Broken"})).is_err());
        run(&mut s, "config.row", json!({"name": "Broken", "delete": true}));
        run(&mut s, "config.column", json!({"feature": fillet, "delete": true}));
        assert_eq!(run(&mut s, "config.table", json!({}))["columns"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn configuration_materials_and_looks_are_undoable_and_persistent() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        let body = s.model.state().bodies.first().unwrap().name.clone();
        run(&mut s, "config.configure", json!({"materials": [body], "appearances": [body]}));
        let material = format!("material:{body}");
        let appearance = format!("appearance:{body}");
        run(&mut s, "config.row", json!({"name": "Metal", "values": {material.clone(): "steel", appearance.clone(): "ABS - Red"}}));
        run(
            &mut s,
            "config.row",
            json!({"name": "Custom", "from": "Metal", "values": {
                material.clone(): "Aluminum", appearance.clone(): {"name": "Ocean", "color": [1, 2, 3], "opacity": 0.4}
            }}),
        );
        run(&mut s, "config.activate", json!({"row": "Metal"}));
        assert_eq!(s.doc.materials.get(&body).map(String::as_str), Some("Steel"));
        assert_eq!(s.doc.appearances.bodies.get(&body).unwrap().color, [196, 40, 40]);
        let mass = run(&mut s, "inspect.measure", json!({}));
        assert!((mass["bodies"][0]["mass_g"].as_f64().unwrap() - 7.85).abs() < 1e-6);
        assert_eq!(run(&mut s, "config.table", json!({}))["matches"], true);
        run(&mut s, "config.activate", json!({"row": "Custom"}));
        let mass = run(&mut s, "inspect.measure", json!({}));
        assert!((mass["bodies"][0]["mass_g"].as_f64().unwrap() - 2.7).abs() < 1e-6);
        assert_eq!(s.doc.appearances.bodies.get(&body).unwrap().opacity, 0.4);
        let reopened = solidcraft_doc::format::read(&s.doc.to_json()).unwrap();
        assert_eq!(reopened, *s.doc);
        assert!(reopened.configuration_matches());
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.materials.get(&body).map(String::as_str), Some("Steel"));
        run(&mut s, "config.activate", json!({"row": "Default"}));
        assert!(!s.doc.materials.contains_key(&body));
        assert!(!s.doc.appearances.bodies.contains_key(&body));
        run(&mut s, "config.activate", json!({"row": "Metal"}));
        run(&mut s, "appearance.assign", json!({"bodies": [body], "appearance": "ABS - Blue"}));
        assert!(!s.doc.configuration_matches());
        run(&mut s, "config.column", json!({"appearance": body, "delete": true}));
        assert_eq!(s.doc.configs.columns.len(), 1);
        assert!(s.doc.configs.rows.iter().all(|r| r.cells.len() == 1));
    }

    #[test]
    fn configuration_styles_reject_hostile_values_without_changes() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        let body = s.model.state().bodies.first().unwrap().name.clone();
        run(&mut s, "config.configure", json!({"materials": [body], "appearances": [body]}));
        let material = format!("material:{body}");
        let appearance = format!("appearance:{body}");
        for params in [
            json!({"name": "Bad", "values": {material: "Unobtainium"}}),
            json!({"name": "Bad", "values": {appearance.clone(): "#oops"}}),
            json!({"name": "Bad", "values": {appearance.clone(): {"name": "Invalid", "color": [1, 2, 300], "opacity": 1}}}),
            json!({"name": "Bad", "values": {appearance: {"name": "Invalid", "color": [1, 2, 3], "opacity": -1}}}),
            json!({"name": "Default", "rename": ""}),
        ] {
            let before = s.doc.clone();
            assert!(s.execute("config.row", &params).is_err(), "{params}");
            assert_eq!(s.doc, before);
        }
        assert!(s.execute("config.column", &json!({"appearance": "missing"})).is_err());
        assert!(s.execute("config.column", &json!({"appearance": body, "material": body})).is_err());
        run(&mut s, "config.row", json!({"name": "Other"}));
        assert!(s.execute("config.row", &json!({"name": "Other", "rename": "Default"})).is_err());
        // A malformed persisted row must fail atomically rather than silently apply its prefix.
        s.doc_mut().configs.rows.first_mut().unwrap().cells.pop();
        let before = s.doc.clone();
        assert!(s.execute("config.activate", &json!({"row": "Default"})).is_err());
        assert_eq!(s.doc, before);
    }

    #[test]
    fn configuration_captures_unicode_custom_appearance_names() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        let body = s.model.state().bodies.first().unwrap().name.clone();
        let name = "é".repeat(128);
        run(&mut s, "appearance.assign", json!({"bodies": [body], "appearance": name, "color": [40, 90, 186]}));
        run(&mut s, "config.configure", json!({"appearances": [body]}));
        run(&mut s, "appearance.assign", json!({"bodies": [body]}));
        run(&mut s, "config.activate", json!({"row": "Default"}));
        assert_eq!(s.doc.appearances.bodies.get(&body).unwrap().name, name);
        assert!(s.doc.configuration_matches());
        let reopened = solidcraft_doc::format::read(&s.doc.to_json()).unwrap();
        assert!(reopened.configuration_matches());
    }

    #[test]
    fn configuration_styles_survive_parameter_and_body_renames() {
        let mut s = Session::default();
        run(&mut s, "parameters.change", json!({"name": "Steel", "expression": "10 mm"}));
        run(&mut s, "parameters.change", json!({"name": "double", "expression": "Steel * 2"}));
        run(&mut s, "solid.box", json!({"length": "double", "width": 10, "height": 10, "body_name": "Block"}));
        run(&mut s, "config.configure", json!({"params": ["Steel", "double"], "materials": ["Block"], "appearances": ["Block"]}));
        run(
            &mut s,
            "config.row",
            json!({"name": "Styled", "values": {
                "param:Steel": "20 mm", "param:double": "Steel * 3", "material:Block": "Steel", "appearance:Block": "Steel - Satin"
            }}),
        );
        run(&mut s, "config.activate", json!({"row": "Styled"}));
        let styles: Vec<Vec<String>> = s.doc.configs.rows.iter().map(|r| r.cells.iter().skip(2).cloned().collect()).collect();
        run(&mut s, "parameters.rename", json!({"name": "Steel", "new_name": "gauge"}));
        let after: Vec<Vec<String>> = s.doc.configs.rows.iter().map(|r| r.cells.iter().skip(2).cloned().collect()).collect();
        assert_eq!(after, styles, "renaming an expression reference must preserve style cells verbatim");
        let table = run(&mut s, "config.table", json!({}));
        assert_eq!(table["columns"][0], "param:gauge");
        assert_eq!(table["rows"][1]["values"]["param:double"], "gauge * 3");
        assert!(s.doc.configuration_matches());
        run(&mut s, "body.rename", json!({"body": "Block", "name": "Housing"}));
        let table = run(&mut s, "config.table", json!({}));
        assert_eq!(table["columns"][2], "material:Housing");
        assert_eq!(table["columns"][3], "appearance:Housing");
        assert!(s.doc.configuration_matches());
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(run(&mut s, "config.table", json!({}))["columns"][2], "material:Block");
        run(&mut s, "edit.redo", json!({}));
        let mut reopened = Session::new(solidcraft_doc::format::read(&s.doc.to_json()).unwrap());
        run(&mut reopened, "config.activate", json!({"row": "Default"}));
        assert!(!reopened.doc.materials.contains_key("Housing"));
        assert!(!reopened.doc.appearances.bodies.contains_key("Housing"));
        run(&mut reopened, "config.activate", json!({"row": "Styled"}));
        assert_eq!(reopened.doc.materials.get("Housing").map(String::as_str), Some("Steel"));
        assert_eq!(reopened.doc.appearances.bodies.get("Housing").unwrap().name, "Steel - Satin");
        assert!(!reopened.doc.materials.contains_key("Block"));
        assert!(!reopened.doc.appearances.bodies.contains_key("Block"));
        assert!((volume(&mut reopened) - 6000.0).abs() < 1e-6);
        assert!(reopened.doc.configuration_matches());
    }
}
