//! Reference-test recipes (`tests/oracle/*/recipe.json`) → SolidCraft command scripts.
//!
//! A recipe lists features with sketch entities in sketch-local coordinates; point references
//! (`l1.end`) share points. Each feature becomes one or more commands.

use serde_json::{Value, json};

fn plane_param(pl: &Value) -> Value {
    let r = pl.get("ref").and_then(Value::as_str).unwrap_or("");
    let origin_zero = pl.get("origin").and_then(Value::as_array).is_none_or(|o| o.iter().all(|x| x.as_f64().unwrap_or(0.0).abs() < 1e-12));
    let std = match r.to_ascii_uppercase().as_str() {
        "XY" => Some(([1.0, 0.0, 0.0], [0.0, 1.0, 0.0])),
        "XZ" => Some(([1.0, 0.0, 0.0], [0.0, 0.0, -1.0])),
        "YZ" => Some(([0.0, 1.0, 0.0], [0.0, 0.0, 1.0])),
        _ => None,
    };
    let axes_match = |k: &str, want: [f64; 3]| {
        pl.get(k).and_then(Value::as_array).is_some_and(|a| a.iter().zip(want).all(|(x, w)| (x.as_f64().unwrap_or(f64::NAN) - w).abs() < 1e-9))
    };
    match std {
        Some((x, y)) if origin_zero && axes_match("x_dir", x) && axes_match("y_dir", y) => json!(r.to_ascii_uppercase()),
        _ => json!({"origin": pl.get("origin").cloned().unwrap_or(json!([0, 0, 0])), "x_dir": pl.get("x_dir"), "y_dir": pl.get("y_dir")}),
    }
}

/// A point argument: the shared reference if any, else the coordinates.
fn parg(p: Option<&Value>) -> Value {
    match p {
        Some(v) => match (v.get("ref"), v.get("at")) {
            (Some(Value::String(r)), _) => json!(r),
            (_, Some(at)) => at.clone(),
            _ => v.clone(),
        },
        None => Value::Null,
    }
}

fn op(f: &Value) -> Value {
    f.get("operation").cloned().unwrap_or(json!("new"))
}

fn sketch_cmds(f: &Value, out: &mut Vec<Value>) -> Result<(), String> {
    let mut p = json!({"plane": plane_param(f.get("plane").unwrap_or(&Value::Null))});
    if let Some(n) = f.get("name") {
        p["name"] = n.clone();
    }
    out.push(json!({"command": "sketch.create", "params": p}));
    for e in f.get("entities").and_then(Value::as_array).into_iter().flatten() {
        let id = e.get("id").cloned().unwrap_or(Value::Null);
        let construction = e.get("construction").cloned().unwrap_or(json!(false));
        match e.get("type").and_then(Value::as_str) {
            Some("line") => out.push(json!({"command": "sketch.line", "params": {"points": [parg(e.get("start")), parg(e.get("end"))], "ids": [id], "construction": construction}})),
            Some("circle") => out.push(json!({"command": "sketch.circle.center", "params": {"center": parg(e.get("center")), "radius": e.get("radius"), "id": id, "construction": construction}})),
            Some("arc_three_point") => out.push(json!({"command": "sketch.arc.three_point", "params": {"start": parg(e.get("start")), "end": parg(e.get("end")), "through": e.get("mid").or(e.get("through")), "id": id}})),
            Some("arc_center_start_sweep") => out.push(json!({"command": "sketch.arc.center_point", "params": {"center": parg(e.get("center")), "start": parg(e.get("start")), "sweep": e.get("sweep_deg").or(e.get("sweep")), "id": id}})),
            Some("arc") => {
                if e.get("center").is_some() {
                    out.push(json!({"command": "sketch.arc.center_point", "params": {"center": parg(e.get("center")), "start": parg(e.get("start")), "end": parg(e.get("end")), "id": id}}))
                } else {
                    out.push(json!({"command": "sketch.arc.three_point", "params": {"start": parg(e.get("start")), "end": parg(e.get("end")), "through": parg(e.get("through").or(e.get("mid"))), "id": id}}))
                }
            }
            Some("point") => out.push(json!({"command": "sketch.point", "params": {"point": parg(e.get("at").map(|a| json!({"at": a})).as_ref().or(Some(e))), "id": id}})),
            // A 3D helix fit: the sweep along it becomes a coil (see `helix_path`).
            Some("fitted_spline") if e.get("fit_points").and_then(Value::as_array).is_some_and(|a| a.iter().any(|p| p.get(2).is_some())) => {}
            Some(other) => return Err(format!("recipe: sketch entity type `{other}` is not supported yet")),
            None => return Err("recipe: sketch entity without type".into()),
        }
    }
    for c in f.get("constraints").and_then(Value::as_array).into_iter().flatten() {
        out.push(constraint_cmd(c)?);
    }
    for d in f.get("dimensions").and_then(Value::as_array).into_iter().flatten() {
        let mut p = json!({"entities": d.get("refs").or(d.get("entities")), "type": d.get("type"), "value": d.get("value")});
        for k in ["orientation", "text_at"] {
            if let Some(v) = d.get(k) {
                p[k] = v.clone();
            }
        }
        out.push(json!({"command": "sketch.dimension", "params": p}));
    }
    out.push(json!({"command": "sketch.finish", "params": {}}));
    Ok(())
}

/// Extent parameters for the Extrude command.
fn extent(f: &Value, p: &mut Value) {
    let e = f.get("extent").cloned().unwrap_or(Value::Null);
    let ty = e.get("type").and_then(Value::as_str).unwrap_or("distance");
    let dist = e.get("distance").cloned().or_else(|| f.get("distance").cloned()).unwrap_or(Value::Null);
    match ty {
        "symmetric" => {
            let full = e.get("is_full_length").and_then(Value::as_bool).unwrap_or(false);
            p["direction"] = json!("symmetric");
            p["distance"] = if full { dist } else { json!(format!("({}) * 2", expr_text(&dist))) };
        }
        "two_sides" => {
            p["distance"] = e.get("side1").cloned().unwrap_or(Value::Null);
            p["distance2"] = e.get("side2_opposite").or(e.get("side2")).cloned().unwrap_or(Value::Null);
        }
        "through_all" => {
            p["through_all"] = json!(true);
            p["direction"] = e.get("direction").cloned().unwrap_or(json!("positive"));
        }
        _ => {
            p["distance"] = dist;
            p["direction"] = e.get("direction").cloned().unwrap_or(json!("positive"));
        }
    }
    if let Some(t) = e.get("taper_deg") {
        p["taper"] = t.clone();
    }
}

fn expr_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn refs(c: &Value) -> Vec<Value> {
    c.get("refs").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// A recipe constraint `{type, refs}` → a constraint command.
fn constraint_cmd(c: &Value) -> Result<Value, String> {
    let ty = c.get("type").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    let r = refs(c);
    let at = |i: usize| r.get(i).cloned().unwrap_or(Value::Null);
    let is_point = |v: &Value| v.as_str().is_some_and(|s| s.contains('.') || s == "origin" || s.starts_with('p'));
    let (cmd, p) = match ty.as_str() {
        "horizontal" | "vertical" if r.len() >= 2 => ("sketch.constraint.horizontal_vertical", json!({"points": [at(0), at(1)], "mode": ty})),
        "horizontal" | "vertical" => ("sketch.constraint.horizontal_vertical", json!({"line": at(0), "mode": ty})),
        "horizontal_points" => ("sketch.constraint.horizontal_vertical", json!({"points": [at(0), at(1)], "mode": "horizontal"})),
        "vertical_points" => ("sketch.constraint.horizontal_vertical", json!({"points": [at(0), at(1)], "mode": "vertical"})),
        "coincident" | "point_on_curve" => {
            // The point goes first.
            if is_point(&at(0)) {
                ("sketch.constraint.coincident", json!({"a": at(0), "b": at(1)}))
            } else {
                ("sketch.constraint.coincident", json!({"a": at(1), "b": at(0)}))
            }
        }
        "midpoint" => {
            if is_point(&at(0)) {
                ("sketch.constraint.midpoint", json!({"point": at(0), "line": at(1)}))
            } else {
                ("sketch.constraint.midpoint", json!({"point": at(1), "line": at(0)}))
            }
        }
        "symmetric" | "symmetry" => ("sketch.constraint.symmetry", json!({"a": at(0), "b": at(1), "line": at(2)})),
        "fix" => ("sketch.constraint.fix", json!({"entity": at(0), "fixed": true})),
        "tangent" => ("sketch.constraint.tangent", json!({"a": at(0), "b": at(1)})),
        "equal" => ("sketch.constraint.equal", json!({"a": at(0), "b": at(1)})),
        "parallel" => ("sketch.constraint.parallel", json!({"a": at(0), "b": at(1)})),
        "perpendicular" => ("sketch.constraint.perpendicular", json!({"a": at(0), "b": at(1)})),
        "concentric" => ("sketch.constraint.concentric", json!({"a": at(0), "b": at(1)})),
        "collinear" => ("sketch.constraint.collinear", json!({"a": at(0), "b": at(1)})),
        other => return Err(format!("recipe: constraint `{other}` is not supported yet")),
    };
    Ok(json!({"command": cmd, "params": p}))
}

/// The profiles a feature uses (`profiles` list or a single `profile`).
fn profiles_of(f: &Value) -> Value {
    match (f.get("profiles"), f.get("profile")) {
        (Some(p), _) => p.clone(),
        (None, Some(p)) => json!([p]),
        _ => json!("all"),
    }
}

/// The sketch a feature uses (name), from its profiles.
fn profile_sketch(f: &Value) -> Value {
    profiles_of(f)
        .as_array()
        .and_then(|a| a.first())
        .and_then(|p| p.get("sketch"))
        .cloned()
        .or_else(|| f.get("sketch").cloned())
        .unwrap_or(Value::Null)
}

fn sketch_entity<'a>(features: &'a [Value], sketch: &Value, id: &Value) -> Option<&'a Value> {
    features
        .iter()
        .find(|x| x.get("op").and_then(Value::as_str) == Some("sketch") && x.get("name") == Some(sketch))?
        .get("entities")?
        .as_array()?
        .iter()
        .find(|e| e.get("id") == Some(id))
}

/// A sweep of a circle along a fitted 3D spline that is a helix about a Z-parallel axis:
/// (base, diameter, pitch, turns, start angle, clockwise, section size).
#[allow(clippy::type_complexity)]
fn helix_path(features: &[Value], f: &Value) -> Option<([f64; 3], f64, f64, f64, f64, bool, f64)> {
    let path = f.get("path")?;
    let curve = sketch_entity(features, path.get("sketch")?, path.get("curves")?.as_array()?.first()?)?;
    if curve.get("type")?.as_str()? != "fitted_spline" {
        return None;
    }
    let pts: Vec<[f64; 3]> = curve
        .get("fit_points")?
        .as_array()?
        .iter()
        .filter_map(|p| {
            let a = p.as_array()?;
            Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?])
        })
        .collect();
    if pts.len() < 8 {
        return None;
    }
    let n = pts.len() as f64;
    let (cx, cy) = circle_fit(&pts)?;
    let r: Vec<f64> = pts.iter().map(|p| (p[0] - cx).hypot(p[1] - cy)).collect();
    let rm = r.iter().sum::<f64>() / n;
    if rm <= 0.0 || r.iter().any(|x| (x - rm).abs() > 1e-3 * rm) {
        return None;
    }
    // Unwrapped angle and height: both linear along a helix.
    let mut total = 0.0;
    let mut prev = (pts[0][1] - cy).atan2(pts[0][0] - cx);
    let start = prev;
    for p in pts.iter().skip(1) {
        let a = (p[1] - cy).atan2(p[0] - cx);
        let mut d = a - prev;
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        total += d;
        prev = a;
    }
    let turns = total.abs() / std::f64::consts::TAU;
    let rise = pts.last()?[2] - pts[0][2];
    if turns < 1e-6 || rise.abs() < 1e-9 {
        return None;
    }
    // The section: a circle in the profile sketch.
    let prof = f.get("profile").or_else(|| f.get("profiles").and_then(|x| x.get(0)))?;
    let loop0 = prof.get("loops")?.get(0)?.get("curves")?.get(0)?;
    let circle = sketch_entity(features, prof.get("sketch")?, loop0)?;
    let size = 2.0 * circle.get("radius")?.as_f64()?;
    Some(([cx, cy, pts[0][2]], 2.0 * rm, rise.abs() / turns, turns, start, (total < 0.0) != (rise < 0.0), size))
}

/// Least-squares circle through the points' (x, y): x² + y² + D x + E y + F = 0 (Kåsa).
#[allow(clippy::needless_range_loop)]
fn circle_fit(pts: &[[f64; 3]]) -> Option<(f64, f64)> {
    let mut m = [[0.0f64; 4]; 3];
    for p in pts {
        let row = [p[0], p[1], 1.0];
        let rhs = -(p[0] * p[0] + p[1] * p[1]);
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += row[i] * row[j];
            }
            m[i][3] += row[i] * rhs;
        }
    }
    // Gauss-Jordan on the 3x4 normal equations.
    for c in 0..3 {
        let piv = (c..3).max_by(|a, b| m[*a][c].abs().total_cmp(&m[*b][c].abs()))?;
        m.swap(c, piv);
        if m[c][c].abs() < 1e-12 {
            return None;
        }
        for r in 0..3 {
            if r != c {
                let f = m[r][c] / m[c][c];
                for k in c..4 {
                    m[r][k] -= f * m[c][k];
                }
            }
        }
    }
    let (d, e) = (m[0][3] / m[0][0], m[1][3] / m[1][1]);
    Some((-d / 2.0, -e / 2.0))
}

pub fn to_script(recipe: &Value) -> Result<Value, String> {
    let mut out: Vec<Value> = Vec::new();
    for p in recipe.get("parameters").and_then(Value::as_array).into_iter().flatten() {
        out.push(json!({"command": "parameters.change", "params": {"name": p.get("name"), "expression": p.get("expression").or(p.get("value")), "unit": p.get("unit").cloned().unwrap_or(json!("mm"))}}));
    }
    let features = recipe.get("features").and_then(Value::as_array).ok_or("recipe: missing `features`")?;
    for f in features {
        let name = f.get("name").cloned().unwrap_or(Value::Null);
        let bodies = f.get("result_bodies").cloned().unwrap_or(json!([]));
        match f.get("op").and_then(Value::as_str) {
            Some("sketch") => sketch_cmds(f, &mut out)?,
            Some("extrude") => {
                let mut p = json!({"sketch": profile_sketch(f), "profiles": profiles_of(f), "operation": op(f), "name": name, "body_names": bodies});
                // Thin: {sketch, curves, thickness, side?, thickness2?} instead of profiles.
                if let (Some(thin), Some(params)) = (f.get("thin"), p.as_object_mut()) {
                    params.remove("profiles");
                    params.insert("thin".into(), json!(true));
                    params.insert("sketch".into(), thin.get("sketch").cloned().unwrap_or(Value::Null));
                    for key in ["curves", "thickness", "side", "thickness2"] {
                        if let Some(v) = thin.get(key) {
                            params.insert(key.into(), v.clone());
                        }
                    }
                }
                extent(f, &mut p);
                out.push(json!({"command": "solid.extrude", "params": p}));
            }
            Some("combine") => out.push(json!({"command": "solid.combine", "params": {
                "target": f.get("target_body").or(f.get("target")), "tools": f.get("tool_bodies").or(f.get("tools")),
                "operation": op(f), "keep_tools": f.get("keep_tools").cloned().unwrap_or(json!(false)), "name": name}})),
            Some("revolve") => {
                let axis = match f.get("axis") {
                    Some(a) if a.get("dir").is_some() => json!({"origin": a.get("origin").cloned().unwrap_or(json!([0, 0, 0])), "dir": a.get("dir")}),
                    Some(a) => a.get("line").or(a.get("ref")).cloned().unwrap_or_else(|| a.clone()),
                    None => Value::Null,
                };
                out.push(json!({"command": "solid.revolve", "params": {
                    "sketch": profile_sketch(f), "profiles": profiles_of(f), "axis": axis,
                    "angle": f.get("angle").cloned().unwrap_or(json!("360 deg")), "operation": op(f), "name": name, "body_names": bodies}}));
            }
            Some("rectangular_pattern") => {
                let d1 = f.get("direction1").cloned().unwrap_or(Value::Null);
                let d2 = f.get("direction2").cloned().unwrap_or(Value::Null);
                let mut p = json!({"features": feature_refs(f.get("features")), "dir1": d1.get("axis"), "count1": d1.get("count"), "spacing1": d1.get("spacing"), "name": name});
                if !d2.is_null() {
                    p["dir2"] = d2.get("axis").cloned().unwrap_or(Value::Null);
                    p["count2"] = d2.get("count").cloned().unwrap_or(Value::Null);
                    p["spacing2"] = d2.get("spacing").cloned().unwrap_or(Value::Null);
                }
                out.push(json!({"command": "solid.pattern.rectangular", "params": p}));
            }
            Some("circular_pattern") => {
                let axis = f.get("axis").map(|a| json!({"origin": a.get("origin").cloned().unwrap_or(json!([0, 0, 0])), "dir": a.get("dir")})).unwrap_or(Value::Null);
                out.push(json!({"command": "solid.pattern.circular", "params": {"features": feature_refs(f.get("features")), "axis": axis, "count": f.get("count"), "angle": f.get("total_angle").cloned().unwrap_or(json!(360)), "name": name}}));
            }
            Some("sm_flange") => match f.get("type").and_then(Value::as_str) {
                Some("edge") => {
                    let mut p = json!({"type": "edge", "edges": edge_points(f), "height": f.get("height"), "name": name});
                    if let Some(a) = f.get("angle") {
                        p["angle"] = a.clone();
                    }
                    if f.get("flip").and_then(Value::as_bool) == Some(true) {
                        p["flip"] = json!(true);
                    }
                    out.push(json!({"command": "sheet.flange", "params": p}));
                }
                Some("contour") => out.push(json!({"command": "sheet.flange", "params": {
                    "type": "contour", "sketch": f.get("profile_sketch"), "curves": f.get("profile_curves"), "distance": f.get("distance"), "name": name}})),
                _ => out.push(json!({"command": "sheet.flange", "params": {
                    "type": "base", "sketch": profile_sketch(f), "profiles": profiles_of(f), "name": name, "body_names": bodies}})),
            },
            Some("sheet_metal_rule_edit") => {
                let rule = f.get("rule").cloned().unwrap_or(json!("Steel (mm)"));
                let mut p = json!({"name": rule});
                if let Some(set) = f.get("set") {
                    for (from, to) in [("thickness", "thickness"), ("kFactor", "k_factor"), ("bendRadius", "bend_radius")] {
                        if let Some(v) = set.get(from) {
                            p[to] = v.clone();
                        }
                    }
                }
                // "thickness 3 mm (final state…)": the last step of a sequence.
                if let Some(last) = f.get("sequence").and_then(Value::as_array).and_then(|a| a.last()).and_then(Value::as_str) {
                    let w: Vec<&str> = last.split_whitespace().collect();
                    if let (Some(&"thickness"), Some(v), Some(u)) = (w.first(), w.get(1), w.get(2)) {
                        p["thickness"] = json!(format!("{v} {u}"));
                    }
                }
                out.push(json!({"command": "sheet.manage_rules", "params": p}));
            }
            Some("sm_hem") => {
                let e = f.get("edge").and_then(|e| e.get("point")).cloned().unwrap_or(Value::Null);
                let mut p = json!({"edges": [e], "length": f.get("length"), "name": name});
                if f.get("flipped").and_then(Value::as_bool) == Some(true) {
                    p["flip"] = json!(true);
                }
                out.push(json!({"command": "sheet.hem", "params": p}));
            }
            Some("sm_unfold") => out.push(json!({"command": "sheet.unfold", "params": {"name": name}})),
            Some("sm_refold") => out.push(json!({"command": "sheet.refold", "params": {"name": name}})),
            Some("path_pattern") => {
                let path = f.get("path").cloned().unwrap_or(Value::Null);
                let mut p = json!({"path_sketch": path.get("sketch"), "path": path.get("curves"), "count": f.get("count"), "name": name});
                match f.get("spacing") {
                    Some(sp) => p["spacing"] = sp.clone(),
                    None => p["distance"] = f.get("distance").cloned().unwrap_or(Value::Null),
                }
                if f.get("pattern_of").and_then(Value::as_str) == Some("bodies") {
                    p["bodies"] = f.get("bodies").cloned().unwrap_or(Value::Null);
                } else {
                    p["features"] = feature_refs(f.get("features"));
                }
                out.push(json!({"command": "solid.pattern.path", "params": p}));
            }
            Some("loft") => {
                let sections: Vec<Value> = f
                    .get("sections")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|sec| match sec.get("point") {
                        Some(pt) => json!({"sketch": sec.get("sketch"), "point": pt}),
                        None => json!({"sketch": sec.get("sketch"), "profiles": [sec]}),
                    })
                    .collect();
                out.push(json!({"command": "solid.loft", "params": {"sections": sections, "operation": op(f), "name": name, "body_names": bodies}}));
            }
            Some("sweep") if helix_path(features, f).is_some() => {
                // A swept circle along a fitted 3D helix: a coil (the true helix).
                let Some((base, diameter, pitch, turns, start, cw, size)) = helix_path(features, f) else { continue };
                out.push(json!({"command": "solid.coil", "params": {
                    "base": base, "diameter": diameter, "pitch": pitch, "revolutions": turns, "section_size": size,
                    "start_angle": format!("{start} rad"), "clockwise": cw, "operation": op(f), "name": name, "body_names": bodies}}));
            }
            Some("sweep") => {
                let path = f.get("path").cloned().unwrap_or(Value::Null);
                out.push(json!({"command": "solid.sweep", "params": {
                    "sketch": profile_sketch(f), "profiles": profiles_of(f), "path_sketch": path.get("sketch"), "path": path.get("curves"),
                    "operation": op(f), "name": name, "body_names": bodies}}));
            }
            // A boundary surface: a loop of one sketch's curves (`sketch`, `curves`), or a network
            // (`u`, `v`), as `surface.boundary` takes them.
            Some("boundary_surface") => {
                let mut p = json!({"name": name});
                for k in ["sketch", "curves", "continuity", "u", "v", "operation"] {
                    if let Some(v) = f.get(k) {
                        p[k] = v.clone();
                    }
                }
                if let Some(b) = bodies.as_array().and_then(|a| a.first()) {
                    p["body_name"] = b.clone();
                }
                out.push(json!({"command": "surface.boundary", "params": p}));
            }
            Some("thicken") => out.push(json!({"command": "surface.thicken", "params": {
                "bodies": f.get("bodies"), "thickness": f.get("thickness"), "symmetric": f.get("symmetric").cloned().unwrap_or(json!(false)),
                "operation": op(f), "name": name}})),
            Some("remove") => out.push(json!({"command": "solid.remove", "params": {"bodies": f.get("bodies"), "name": name}})),
            Some("shell") => {
                let faces: Vec<Value> = f.get("faces_removed").and_then(Value::as_array).into_iter().flatten().filter_map(|x| x.get("point").cloned()).collect();
                out.push(json!({"command": "solid.shell", "params": {"faces": faces, "thickness": f.get("inside_thickness"), "name": name}}));
            }
            Some("draft") => {
                let faces: Vec<Value> = f.get("faces").and_then(Value::as_array).into_iter().flatten().filter_map(|x| x.get("point").cloned()).collect();
                let np = f.get("neutral_plane").cloned().unwrap_or(Value::Null);
                let n = np.get("plane_normal_outward").or(np.get("normal_at_point_on_face")).cloned().unwrap_or(json!([0, 0, 1]));
                let neutral = json!({"origin": np.get("point").or(np.get("centroid")).cloned().unwrap_or(json!([0, 0, 0])), "normal": n.clone()});
                let mut angle = f.get("angle").and_then(Value::as_f64).unwrap_or(0.0);
                if f.get("flipped").and_then(Value::as_bool).unwrap_or(false) {
                    angle = -angle;
                }
                out.push(json!({"command": "solid.draft", "params": {"faces": faces, "angle": angle, "neutral": neutral, "pull": n, "name": name}}));
            }
            Some("thread") => {
                let face = f.get("face").and_then(|x| x.get("point")).cloned().unwrap_or(Value::Null);
                let mut p = json!({"face": face, "designation": f.get("designation"), "name": name});
                if f.get("full_length").and_then(Value::as_bool) == Some(false)
                    && let Some(l) = f.get("length")
                {
                    p["length"] = l.clone();
                }
                out.push(json!({"command": "solid.thread", "params": p}));
            }
            Some("hole") => {
                let n = f.get("face").and_then(|fc| fc.get("plane_normal_outward").or(fc.get("normal_at_point_on_face"))).cloned();
                let mut dir = n.and_then(|n| n.as_array().map(|a| a.iter().map(|x| -x.as_f64().unwrap_or(0.0)).collect::<Vec<f64>>()));
                // On a cylinder the normal given is at the face's sample point: drill toward the axis.
                let v3 = |v: Option<&Value>| -> Option<[f64; 3]> {
                    let a = v?.as_array()?;
                    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?])
                };
                if let Some(fc) = f.get("face").filter(|fc| fc.get("type").and_then(Value::as_str) == Some("cylinder"))
                    && let (Some(o), Some(ax), Some(p)) = (v3(fc.get("axis_origin")), v3(fc.get("axis")), v3(f.get("position")))
                {
                    let d = [p[0] - o[0], p[1] - o[1], p[2] - o[2]];
                    let t = d[0] * ax[0] + d[1] * ax[1] + d[2] * ax[2];
                    let r = [d[0] - t * ax[0], d[1] - t * ax[1], d[2] - t * ax[2]];
                    let l = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
                    if l > 1e-9 {
                        dir = Some(vec![-r[0] / l, -r[1] / l, -r[2] / l]);
                    }
                }
                let ty = f.get("hole_type").and_then(Value::as_str).unwrap_or("simple");
                let mut p = json!({"position": f.get("position"), "diameter": f.get("diameter"), "type": ty, "name": name});
                // "sketch HolePoints points p1..p6"
                if let Some(src) = f.get("positions_from").and_then(Value::as_str) {
                    let w: Vec<&str> = src.split_whitespace().collect();
                    if let (Some(sk), Some(range)) = (w.iter().position(|x| *x == "sketch").and_then(|i| w.get(i + 1)), w.iter().position(|x| *x == "points").and_then(|i| w.get(i + 1))) {
                        let ids: Vec<String> = match range.split_once("..") {
                            Some((a, b)) => {
                                let pre: String = a.chars().take_while(|c| !c.is_ascii_digit()).collect();
                                let (lo, hi) = (a[pre.len()..].parse::<u32>().unwrap_or(1), b.trim_start_matches(pre.as_str()).parse::<u32>().unwrap_or(0));
                                (lo..=hi).map(|k| format!("{pre}{k}")).collect()
                            }
                            None => range.split(',').map(str::to_string).collect(),
                        };
                        p = json!({"sketch": sk, "points": ids, "diameter": f.get("diameter"), "type": ty, "name": name});
                    }
                }
                if let Some(d) = dir {
                    p["direction"] = json!(d);
                }
                if f.get("extent").and_then(Value::as_str) != Some("through_all") {
                    p["depth"] = f.get("depth").cloned().unwrap_or(Value::Null);
                    if let Some(t) = f.get("tip_angle") {
                        p["tip_angle"] = t.clone();
                    }
                }
                for (k, src) in [("cb_diameter", "counterbore_diameter"), ("cb_depth", "counterbore_depth"), ("cs_diameter", "countersink_diameter"), ("cs_angle", "countersink_angle")] {
                    if let Some(v) = f.get(src) {
                        p[k] = v.clone();
                    }
                }
                out.push(json!({"command": "solid.hole", "params": p}));
            }
            Some("construction_plane") => match f.get("method").and_then(Value::as_str) {
                Some("offset") => out.push(json!({"command": "construct.plane.offset", "params": {"base": f.get("base"), "offset": f.get("offset"), "name": name}})),
                _ => {
                    // Use the resulting plane as recorded.
                    let (o, n) = (f.get("result_origin").cloned().unwrap_or(json!([0, 0, 0])), f.get("result_normal").cloned().unwrap_or(Value::Null));
                    if n.is_null() {
                        return Err(format!("recipe: construction plane method {:?} without result_normal", f.get("method")));
                    }
                    out.push(json!({"command": "construct.plane.offset", "params": {"base": {"origin": o, "normal": n}, "offset": 0, "name": name}}));
                }
            },
            Some("split_body") => out.push(json!({"command": "solid.split_body", "params": {"body": f.get("body"), "plane": f.get("tool"), "name": name}})),
            Some("mirror") => {
                let pl = f.get("plane").cloned().unwrap_or(Value::Null);
                let plane = match pl.get("ref").and_then(Value::as_str) {
                    Some(r @ ("XY" | "XZ" | "YZ")) => json!(r),
                    _ => json!({"origin": pl.get("origin").cloned().unwrap_or(json!([0, 0, 0])), "normal": pl.get("normal")}),
                };
                let mut p = json!({"features": feature_refs(f.get("features")), "plane": plane, "name": name});
                if f.get("mirror_of").and_then(Value::as_str) == Some("bodies") {
                    p = json!({"bodies": f.get("bodies"), "combine": f.get("combine").cloned().unwrap_or(json!(false)), "plane": plane, "name": name});
                }
                out.push(json!({"command": "solid.mirror", "params": p}));
            }
            Some("fillet") => {
                let mut p = json!({"edges": edge_points(f), "radius": f.get("radius"), "name": name});
                // Constant radius (the default), chord length, or a radius varying along each edge
                // (`radius` at the end nearest `start`, `radius2` at the other, `mid` between,
                // `smooth`), or a B-spline along each edge (`radius` its first control radius,
                // `radii` the rest, `knots`); `fit` for a radius too large for the geometry.
                match f.get("type").and_then(Value::as_str) {
                    Some("chord_length") => p["type"] = json!("chord"),
                    Some("variable_radius") => p["type"] = json!("variable"),
                    Some("spline_radius") => p["type"] = json!("spline"),
                    _ => {}
                }
                for key in ["radius2", "start", "mid", "smooth", "fit", "radii", "knots"] {
                    if let Some(v) = f.get(key) {
                        p[key] = v.clone();
                    }
                }
                out.push(json!({"command": "solid.fillet", "params": p}))
            }
            Some("chamfer") => {
                out.push(json!({"command": "solid.chamfer", "params": {"edges": edge_points(f), "distance": f.get("distance"), "name": name}}))
            }
            Some(other) => return Err(format!("recipe: feature op `{other}` is not supported yet")),
            None => return Err("recipe: feature without `op`".into()),
        }
    }
    Ok(json!({"commands": out}))
}

fn edge_points(f: &Value) -> Value {
    Value::Array(
        f.get("edges")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|e| e.get("point").or(e.get("mid")).cloned().unwrap_or_else(|| e.clone()))
            .collect(),
    )
}

/// A pattern's source features may carry an occurrence suffix ("Hole1 (1)"); the features are
/// the recipe's own names.
fn feature_refs(v: Option<&Value>) -> Value {
    let list: Vec<Value> = v
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|x| match x.as_str() {
            Some(s) => {
                let t = s.trim();
                let base = match t.rfind(" (") {
                    Some(i) if t.ends_with(')') && t[i + 2..t.len() - 1].chars().all(|c| c.is_ascii_digit()) => &t[..i],
                    _ => t,
                };
                json!(base)
            }
            None => x.clone(),
        })
        .collect();
    json!(list)
}
