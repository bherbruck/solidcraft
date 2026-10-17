//! Parameters: Change Parameters (add, edit, rename, delete) and the `parameters.*` commands
//! behind the parameters dialog — list, add, rename, favourite, comment, users, dependency
//! graph, CSV/JSON import and export, expression evaluation and autocomplete.

use serde_json::{Value, json};
use solidcraft_doc::expr::{self, Kind};

use super::CommandSpec;
use crate::params::{bad, bool_, str_};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("parameters.change", "Change Parameters", change_param).at("SOLID", "MODIFY").icon("params").params(
        "name, expression (number or text), unit?: mm|cm|m|in|ft|deg|rad|\"\" (new parameters default to their value's unit), comment?, \
             favorite?: bool, new_name?: rename (references follow); or delete: name → recomputed: features rebuilt, \
         restored: features whose earlier result for the same inputs was reused (e.g. a value set back)",
    ),
    CommandSpec::new("parameters.list", "List Parameters", list).icon("list").noundo().doc_only().params(
        "favorites?: bool (only favourites), filter?: all|user|sketch|feature|favorites → parameters (name, expression, unit, value in its unit, \
         source, feature/input, comment, favorite, uses, used_by, error) and cycles",
    ),
    CommandSpec::new("parameters.add", "Add User Parameter", add).icon("new").params("name, expression, unit?, comment?, favorite?: bool"),
    CommandSpec::new("parameters.rename", "Rename Parameter", rename)
        .icon("rename")
        .params("name, new_name (expressions, feature inputs and dimensions that use it follow)"),
    CommandSpec::new("parameters.delete", "Delete Parameter", delete)
        .icon("delete")
        .params("name (refused while something uses it; the error lists the users)"),
    CommandSpec::new("parameters.favorite", "Favourite Parameter", favorite).icon("star").params("name, favorite?: bool (default true)"),
    CommandSpec::new("parameters.move", "Move Parameter", move_param)
        .icon("reorder")
        .params("name, before | after: another user parameter; the user parameter is listed just before or after it"),
    CommandSpec::new("parameters.comment", "Comment Parameter", comment).icon("text").params("name, comment"),
    CommandSpec::new("parameters.users", "Parameter Users", users)
        .icon("graph")
        .noundo()
        .params("name → users (parameters, feature inputs, dimensions) and the features it drives"),
    CommandSpec::new("parameters.graph", "Parameter Graph", graph)
        .icon("graph")
        .noundo()
        .params("→ nodes, edges [user, used] (users include Feature:<name>), cycles, errors"),
    CommandSpec::new("parameters.export", "Export Parameters", export)
        .icon("export")
        .noundo()
        .params("format?: csv|json (default csv, or from the path), path?: write to a file → text"),
    CommandSpec::new("parameters.import", "Import Parameters", import)
        .icon("import")
        .params("text: CSV (header with name, expression, unit?, comment?) or JSON [{name, expression, unit?, comment?}] | path; all or nothing"),
    CommandSpec::new("expr.evaluate", "Evaluate Expression", evaluate)
        .icon("params")
        .noundo()
        .doc_only()
        .params("expression, kind?: length|angle|unitless (or unit?: mm|in|deg…) → ok, value (mm/rad), display_value, references, error"),
    CommandSpec::new("parameters.complete", "Complete Expression", complete)
        .icon("params")
        .noundo()
        .doc_only()
        .params("prefix?: the text before the cursor → word, suggestions [{text, kind: parameter|function|unit|constant, detail}]"),
];

fn expr_arg(p: &Value, k: &str) -> Option<String> {
    match p.get(k)? {
        Value::String(x) => Some(x.clone()),
        Value::Number(n) => n.as_f64().filter(|v| v.is_finite()).map(|v| format!("{v}")),
        _ => None,
    }
}

fn param_name<'a>(cmd: &str, p: &'a Value) -> Result<&'a str> {
    str_(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`name` is required"))
}

/// Features that fail (name and error).
fn failing(s: &Session) -> Vec<Value> {
    s.model.results.iter().filter_map(|r| r.error.as_ref().map(|e| json!({"feature": r.name, "error": e}))).collect()
}

fn known(s: &Session, name: &str) -> bool {
    s.doc.param(name).is_some() || s.doc.features.iter().any(|f| f.param_names.iter().any(|n| n == name))
}

fn change_param(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.change";
    let solved = s.solved_sketches();
    if let Some(n) = str_(p, "delete") {
        s.doc_mut().remove_param(n.trim())?;
        s.settle_value_edit(&solved)?;
        return Ok(json!({"deleted": n.trim()}));
    }
    let mut name = param_name(cmd, p)?.to_string();
    let mut out = json!({});
    if let Some(new) = str_(p, "new_name") {
        let n = s.doc_mut().rename_param(&name, new)?;
        out["renamed"] = json!({"from": name, "to": new.trim(), "references_updated": n});
        name = new.trim().to_string();
    }
    let has_expr = p.get("expression").is_some() || p.get("value").is_some();
    let e = if has_expr {
        Some(expr_arg(p, "expression").or_else(|| expr_arg(p, "value")).ok_or_else(|| bad(cmd, "`expression` must be a number or text"))?)
    } else if out.get("renamed").is_some() || p.get("favorite").is_some() || p.get("comment").is_some() {
        None
    } else {
        return Err(bad(cmd, "`expression` must be a number or text"));
    };
    let (unit, comment) = (str_(p, "unit"), str_(p, "comment"));
    match &e {
        Some(e) => s.doc_mut().change_param(&name, e, unit, comment)?,
        None => {
            if !known(s, &name) {
                return Err(bad(cmd, format!("no parameter `{name}`")));
            }
            if let Some(c) = comment {
                s.doc_mut().set_param_comment(&name, c)?;
            }
        }
    }
    if let Some(f) = bool_(p, "favorite") {
        s.doc_mut().set_favorite(&name, f)?;
    }
    s.settle_value_edit(&solved)?;
    let (vals, _) = s.doc.param_values();
    out["name"] = json!(name);
    out["expression"] = json!(e);
    out["value"] = json!(vals.get(&name).map(|v| v.v));
    out["display_value"] = json!(s.doc.param_display_value(&vals, &name));
    out["recomputed"] = json!(s.model.last_recomputed);
    out["restored"] = json!(s.model.last_restored);
    out["errors"] = json!(failing(s));
    Ok(out)
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let filter = match (bool_(p, "favorites"), str_(p, "filter")) {
        (Some(true), _) => "favorites".to_string(),
        (_, Some(f)) => f.to_ascii_lowercase(),
        _ => "all".to_string(),
    };
    let mut rows: Vec<solidcraft_doc::ParamRow> = s
        .doc
        .param_rows()
        .into_iter()
        .filter(|r| match filter.as_str() {
            "favorites" | "favourites" | "favorite" => r.favorite,
            "user" | "sketch" | "feature" => r.source == filter,
            "model" => r.source != "user",
            _ => true,
        })
        .collect();
    // Favourites first; otherwise the table order (user, sketch, feature).
    rows.sort_by_key(|r| !r.favorite);
    let cycles = solidcraft_doc::cycles(&s.doc.all_param_exprs());
    Ok(json!({"parameters": rows, "count": rows.len(), "cycles": cycles}))
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.add";
    let name = param_name(cmd, p)?.to_string();
    let e = expr_arg(p, "expression").or_else(|| expr_arg(p, "value")).ok_or_else(|| bad(cmd, "`expression` must be a number or text"))?;
    let solved = s.solved_sketches();
    s.doc_mut().add_user_param(&name, &e, str_(p, "unit"), str_(p, "comment"))?;
    s.settle_value_edit(&solved)?;
    if let Some(f) = bool_(p, "favorite") {
        s.doc_mut().set_favorite(&name, f)?;
    }
    let (vals, _) = s.doc.param_values();
    Ok(
        json!({"name": name, "unit": s.doc.param(&name).map(|x| x.unit.clone()), "value": vals.get(&name).map(|v| v.v), "display_value": s.doc.param_display_value(&vals, &name)}),
    )
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.rename";
    let name = param_name(cmd, p)?.to_string();
    let new = str_(p, "new_name").ok_or_else(|| bad(cmd, "`new_name` is required"))?;
    let n = s.doc_mut().rename_param(&name, new)?;
    Ok(json!({"from": name, "to": new.trim(), "references_updated": n}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let name = param_name("parameters.delete", p)?.to_string();
    let solved = s.solved_sketches();
    s.doc_mut().remove_param(&name)?;
    s.settle_value_edit(&solved)?;
    Ok(json!({"deleted": name}))
}

fn favorite(s: &mut Session, p: &Value) -> Result<Value> {
    let name = param_name("parameters.favorite", p)?.to_string();
    let on = bool_(p, "favorite").unwrap_or(true);
    s.doc_mut().set_favorite(&name, on)?;
    Ok(json!({"name": name, "favorite": on}))
}

fn move_param(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.move";
    let name = param_name(cmd, p)?.to_string();
    let (target, after) = match (str_(p, "before"), str_(p, "after")) {
        (Some(t), None) => (t.to_string(), false),
        (None, Some(t)) => (t.to_string(), true),
        _ => return Err(bad(cmd, "give one of `before` or `after`: another user parameter")),
    };
    s.doc_mut().move_user_param(&name, &target, after)?;
    let order: Vec<&str> = s.doc.params.iter().filter(|q| !q.model).map(|q| q.name.as_str()).collect();
    Ok(json!({"name": name, "order": order}))
}

fn comment(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.comment";
    let name = param_name(cmd, p)?.to_string();
    let c = str_(p, "comment").ok_or_else(|| bad(cmd, "`comment` must be text"))?;
    s.doc_mut().set_param_comment(&name, c)?;
    Ok(json!({"name": name, "comment": c}))
}

fn users(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.users";
    let name = param_name(cmd, p)?;
    if !known(s, name) {
        return Err(bad(cmd, format!("no parameter `{name}`")));
    }
    let users = s.doc.param_users(name);
    let drives: Vec<Value> =
        s.doc.features_using_param(name).iter().filter_map(|id| s.doc.feature(*id)).map(|f| json!({"id": f.id, "name": f.name})).collect();
    Ok(json!({"name": name, "users": users, "drives": drives}))
}

fn graph(s: &mut Session, _p: &Value) -> Result<Value> {
    let (_, errs) = s.doc.param_values();
    let nodes: Vec<String> = s.doc.param_defs().into_iter().map(|d| d.name).collect();
    let edges: Vec<[String; 2]> = s.doc.param_graph().into_iter().map(|(a, b)| [a, b]).collect();
    let cycles = solidcraft_doc::cycles(&s.doc.all_param_exprs());
    Ok(json!({"nodes": nodes, "edges": edges, "cycles": cycles, "errors": errs}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.export";
    let path = str_(p, "path");
    let format = str_(p, "format").map(str::to_ascii_lowercase).unwrap_or_else(|| match path {
        Some(x) if x.to_ascii_lowercase().ends_with(".json") => "json".into(),
        _ => "csv".into(),
    });
    let text = match format.as_str() {
        "csv" => s.doc.params_csv(),
        "json" => {
            let rows: Vec<Value> = s
                .doc
                .param_rows()
                .into_iter()
                .filter(|r| r.source == "user")
                .map(|r| json!({"name": r.name, "expression": r.expression, "unit": r.unit, "value": r.value, "comment": r.comment, "favorite": r.favorite}))
                .collect();
            serde_json::to_string_pretty(&rows).map_err(|e| bad(cmd, e.to_string()))?
        }
        other => return Err(bad(cmd, format!("unknown format `{other}` (csv or json)"))),
    };
    if let Some(path) = path {
        solidcraft_io::vfs::write(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    }
    Ok(json!({"format": format, "path": path, "text": text}))
}

/// Largest parameter table we read from a file.
const MAX_PARAMETER_BYTES: usize = 16 << 20;

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.import";
    let text = match (str_(p, "text"), str_(p, "path")) {
        (Some(t), _) => t.to_string(),
        (None, Some(path)) => {
            solidcraft_io::vfs::read_to_string(path, MAX_PARAMETER_BYTES).map_err(|e| EngineError::Other(format!("{path}: {e}")))?
        }
        _ => return Err(bad(cmd, "give `text` or `path`")),
    };
    let solved = s.solved_sketches();
    let names = s.doc_mut().import_params(&text)?;
    s.settle_value_edit(&solved)?;
    Ok(json!({"imported": names, "errors": failing(s)}))
}

fn evaluate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "expr.evaluate";
    let e = expr_arg(p, "expression").ok_or_else(|| bad(cmd, "`expression` must be text or a number"))?;
    let unit = str_(p, "unit");
    if let Some(u) = unit
        && expr::unit_info(u).is_none()
    {
        return Err(bad(cmd, format!("unknown unit `{u}`")));
    }
    let kind = match (str_(p, "kind"), unit) {
        (Some("length"), _) => Some(Kind::Length),
        (Some("angle"), _) => Some(Kind::Angle),
        (Some("unitless" | "number"), _) => Some(Kind::Unitless),
        (Some(other), _) => return Err(bad(cmd, format!("unknown kind `{other}` (length, angle or unitless)"))),
        (None, Some(u)) => Some(Kind::from_unit(u)),
        (None, None) => None,
    };
    let (vals, _) = s.doc.param_values();
    let defaults = expr::Defaults::for_unit(unit.unwrap_or(""), 1.0);
    let refs = expr::references(&e);
    let lookup = |n: &str| vals.get(n).copied().ok_or_else(|| solidcraft_doc::DocError::Expr(format!("unknown parameter `{n}`")));
    let r = expr::eval_with_defaults(&e, &lookup, defaults).and_then(|v| match kind {
        Some(k) => v.to_kind_in(k, defaults).map(|x| (x, k)),
        None => v.kind().map(|k| (v.v, k)).ok_or_else(|| solidcraft_doc::DocError::Expr(format!("the result is {}", v.describe()))),
    });
    Ok(match r {
        Ok((v, k)) => {
            let value = match k {
                Kind::Length => expr::Value::length(v),
                Kind::Angle => expr::Value::angle(v),
                Kind::Unitless => expr::Value::num(v),
            };
            let shown = match (unit, k) {
                (Some(u), _) => expr::value_in_unit(value, u),
                (None, Kind::Angle) => v.to_degrees(),
                (None, _) => v,
            };
            json!({"ok": true, "value": v, "kind": k, "display_value": shown, "references": refs})
        }
        Err(err) => json!({"ok": false, "error": err.to_string().trim_start_matches("expression: ").to_string(), "references": refs}),
    })
}

fn complete(s: &mut Session, p: &Value) -> Result<Value> {
    let text = str_(p, "prefix").unwrap_or("");
    // The identifier being typed: trailing letters, digits and underscores.
    let word: String = text.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
    let starts = |n: &str| word.is_empty() || n.to_ascii_lowercase().starts_with(&word.to_ascii_lowercase());
    let mut out: Vec<Value> = Vec::new();
    let mut rows: Vec<solidcraft_doc::ParamRow> = s.doc.param_rows().into_iter().filter(|r| starts(&r.name)).collect();
    // Favourites, then user parameters, then the rest.
    rows.sort_by_key(|r| (!r.favorite, r.source != "user"));
    for r in rows.into_iter().take(200) {
        let v = r.value.map(|v| format!("{v:.4} {}", r.unit).trim().to_string()).unwrap_or_else(|| "error".into());
        let mut detail = format!("{} = {v}", r.expression);
        if let (Some(f), Some(i)) = (&r.feature, r.input) {
            detail.push_str(&format!(" ({f} {i})"));
        }
        if !r.comment.is_empty() {
            detail.push_str(&format!(" — {}", r.comment));
        }
        out.push(json!({"text": r.name, "kind": "parameter", "detail": detail}));
    }
    for (f, sig) in expr::FUNCTIONS {
        if starts(f) {
            out.push(json!({"text": format!("{f}("), "kind": "function", "detail": sig}));
        }
    }
    if starts("pi") {
        out.push(json!({"text": "pi", "kind": "constant", "detail": "3.14159…"}));
    }
    for (u, k, _) in expr::UNITS {
        if !word.is_empty() && starts(u) {
            out.push(json!({"text": u, "kind": "unit", "detail": k}));
        }
    }
    Ok(json!({"word": word, "suggestions": out}))
}

#[cfg(test)]
#[path = "parameters_tests.rs"]
mod tests;
