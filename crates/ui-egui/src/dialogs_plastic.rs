//! Plastic dialogs: Boss, Lip / Groove, Snap Fit and Rest (placed by a pick on a face), Manage
//! Plastic Rules and Assign Plastic Rule. Values left empty take the body's plastic rule.

use serde_json::{Map, Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::doc::FeatureKind;
use solidcraft_engine::doc::expr::Kind as ValueKind;
use solidcraft_engine::doc::plastic::PlasticRule;
use solidcraft_engine::geom::Vec3;

use crate::SolveApp;
use crate::dialogs::{Kind, combo, face_sel, field, row_label, shown_expr};
use crate::selection::{BODIES, FACES, PLANAR_FACES, SelInput};

#[derive(Clone, Debug)]
pub enum Pl {
    Boss {
        diameter: String,
        height: String,
        hole_diameter: String,
        hole_depth: String,
        draft: String,
        fillet: String,
        ribs: String,
        rib_thickness: String,
        rib_length: String,
        rib_offset: String,
    },
    Lip {
        groove: bool,
        width: String,
        height: String,
        gap: String,
        outside: bool,
    },
    SnapFit {
        hook: usize,
        length: String,
        thickness: String,
        width: String,
        catch_depth: String,
        catch_length: String,
    },
    Rest {
        round: bool,
        width: String,
        length: String,
        height: String,
        draft: String,
        thickness: String,
    },
    /// The rule manager: the rule shown (index into the rules, past the end for a new one).
    Rules {
        pick: usize,
        name: String,
        values: Vec<String>,
        active: bool,
        loaded: Option<usize>,
    },
    Assign {
        rule: usize,
    },
}

impl Pl {
    pub fn title(&self) -> &'static str {
        match self {
            Pl::Boss { .. } => "BOSS",
            Pl::Lip { groove: false, .. } => "LIP",
            Pl::Lip { .. } => "GROOVE",
            Pl::SnapFit { .. } => "SNAP FIT",
            Pl::Rest { .. } => "REST",
            Pl::Rules { .. } => "MANAGE PLASTIC RULES",
            Pl::Assign { .. } => "ASSIGN PLASTIC RULE",
        }
    }

    pub fn previews(&self) -> bool {
        !matches!(self, Pl::Rules { .. } | Pl::Assign { .. })
    }

    pub fn primary(&mut self) -> Option<(&'static str, ValueKind, &mut String)> {
        Some(match self {
            Pl::Boss { height, .. } | Pl::Lip { height, .. } | Pl::Rest { height, .. } => ("Height", ValueKind::Length, height),
            Pl::SnapFit { length, .. } => ("Length", ValueKind::Length, length),
            _ => return None,
        })
    }
}

fn boss() -> Pl {
    Pl::Boss {
        diameter: "6 mm".into(),
        height: "10 mm".into(),
        hole_diameter: "3 mm".into(),
        hole_depth: String::new(),
        draft: String::new(),
        fillet: String::new(),
        ribs: "0".into(),
        rib_thickness: "1 mm".into(),
        rib_length: "3 mm".into(),
        rib_offset: "1 mm".into(),
    }
}

fn snap_fit() -> Pl {
    Pl::SnapFit {
        hook: 0,
        length: "10 mm".into(),
        thickness: "1.5 mm".into(),
        width: "5 mm".into(),
        catch_depth: "1 mm".into(),
        catch_length: "2 mm".into(),
    }
}

fn rest() -> Pl {
    Pl::Rest { round: true, width: "10 mm".into(), length: "15 mm".into(), height: "2 mm".into(), draft: String::new(), thickness: String::new() }
}

fn lip(groove: bool) -> Pl {
    Pl::Lip { groove, width: "1.5 mm".into(), height: "2 mm".into(), gap: String::new(), outside: false }
}

/// The dialog for a plastic command.
pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let (pl, inputs) = match id {
        "plastic.boss" => (boss(), vec![SelInput::new("Position", PLANAR_FACES, false)]),
        "plastic.lip" => (lip(false), vec![SelInput::new("Rim face", FACES, false)]),
        "plastic.snap_fit" => (snap_fit(), vec![SelInput::new("Position", PLANAR_FACES, false)]),
        "plastic.rest" => (rest(), vec![SelInput::new("Position", PLANAR_FACES, false)]),
        "plastic.manage_rules" => {
            let rules = app.session.doc.plastic_rules();
            let pick = app.session.doc.plastic.active.as_ref().and_then(|a| rules.iter().position(|r| &r.name == a)).unwrap_or(0);
            (Pl::Rules { pick, name: String::new(), values: Vec::new(), active: false, loaded: None }, vec![])
        }
        "plastic.assign_rule" => (Pl::Assign { rule: 1 }, vec![SelInput::new("Bodies", BODIES, true)]),
        _ => return None,
    };
    Some((Kind::Plastic(pl), inputs))
}

/// A value row whose empty value means "from the rule" (or another default). `unit` leads the
/// hint ("mm, from the rule") so the empty field names the unit a typed number takes.
fn opt_field(ui: &mut egui::Ui, label: &str, v: &mut String, unit: &str, hint: &str) -> bool {
    row_label(ui, label);
    let hint = format!("{unit}, {}", crate::i18n::tr_label(hint));
    let r = ui.add(egui::TextEdit::singleline(v).hint_text(hint));
    crate::dialogs::publish_row(&r);
    crate::params_dialog::complete(ui, &r, v);
    ui.end_row();
    r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

fn value_row(ui: &mut egui::Ui, label: &str, v: &mut String) -> bool {
    row_label(ui, label);
    let e = field(ui, v);
    ui.end_row();
    e
}

/// Directions across a face (with normal `n`) a snap fit's catch can point: two axes and their
/// opposites, named after the world axis they run along.
pub fn hook_dirs(n: Vec3) -> Vec<(String, Vec3)> {
    let u = [Vec3::X, Vec3::Y, Vec3::Z].into_iter().filter_map(|a| (a - n * a.dot(n)).normalized()).next().unwrap_or_else(|| n.any_perp());
    let v = n.cross(u);
    let name = |d: Vec3| -> String {
        let (axis, c) = [("X", d.x), ("Y", d.y), ("Z", d.z)].into_iter().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap_or(("X", 0.0));
        format!("{}{axis}", if c < 0.0 { "−" } else { "+" })
    };
    [u, -u, v, -v].into_iter().map(|d| (name(d), d)).collect()
}

fn face_normal(app: &SolveApp, inputs: &[SelInput]) -> Option<Vec3> {
    match inputs.first()?.items.first()? {
        Sel::Face { body, index, .. } => crate::dialogs::planar_face(&app.session, body, *index).map(|(_, n)| n),
        _ => None,
    }
}

/// The dialog's own rows (after the selection inputs); true when Enter was pressed in a value.
pub fn rows(app: &mut SolveApp, ui: &mut egui::Ui, k: &mut Pl, inputs: &[SelInput]) -> bool {
    let mut enter = false;
    match k {
        Pl::Boss { diameter, height, hole_diameter, hole_depth, draft, fillet, ribs, rib_thickness, rib_length, rib_offset } => {
            enter |= value_row(ui, "Diameter", diameter);
            enter |= value_row(ui, "Height", height);
            enter |= opt_field(ui, "Hole Diameter", hole_diameter, "mm", "no hole");
            enter |= opt_field(ui, "Hole Depth", hole_depth, "mm", "to the face");
            enter |= opt_field(ui, "Draft Angle", draft, "deg", "from the rule");
            enter |= opt_field(ui, "Root Fillet", fillet, "mm", "none");
            enter |= value_row(ui, "Ribs", ribs);
            if app.session.doc.eval(ribs, ValueKind::Unitless).is_ok_and(|n| n >= 1.0) {
                enter |= value_row(ui, "  Rib Thickness", rib_thickness);
                enter |= value_row(ui, "  Rib Length", rib_length);
                enter |= value_row(ui, "  Rib Offset", rib_offset);
            }
        }
        Pl::Lip { groove, width, height, gap, outside } => {
            row_label(ui, "Type");
            let mut ty = usize::from(*groove);
            combo(ui, "pl_lip", &["Lip", "Groove"], &mut ty);
            *groove = ty == 1;
            ui.end_row();
            row_label(ui, "Rim Edge");
            let mut side = usize::from(*outside);
            combo(ui, "pl_side", &["Inside", "Outside"], &mut side);
            *outside = side == 1;
            ui.end_row();
            enter |= value_row(ui, "Width", width);
            enter |= value_row(ui, "Height", height);
            if *groove {
                enter |= opt_field(ui, "Clearance", gap, "mm", "from the rule");
            }
        }
        Pl::SnapFit { hook, length, thickness, width, catch_depth, catch_length } => {
            let dirs = face_normal(app, inputs).map(hook_dirs).unwrap_or_default();
            if !dirs.is_empty() {
                let names: Vec<&str> = dirs.iter().map(|(n, _)| n.as_str()).collect();
                row_label(ui, "Catch Direction");
                combo(ui, "pl_hook", &names, hook);
                ui.end_row();
            }
            enter |= value_row(ui, "Length", length);
            enter |= value_row(ui, "Thickness", thickness);
            enter |= value_row(ui, "Width", width);
            enter |= value_row(ui, "Catch Depth", catch_depth);
            enter |= value_row(ui, "Catch Length", catch_length);
        }
        Pl::Rest { round, width, length, height, draft, thickness } => {
            row_label(ui, "Shape");
            let mut sh = usize::from(!*round);
            combo(ui, "pl_rest", &["Round", "Rectangular"], &mut sh);
            *round = sh == 0;
            ui.end_row();
            enter |= value_row(ui, if *round { "Diameter" } else { "Width" }, width);
            if !*round {
                enter |= value_row(ui, "Length", length);
            }
            enter |= value_row(ui, "Height", height);
            enter |= opt_field(ui, "Draft Angle", draft, "deg", "from the rule");
            enter |= opt_field(ui, "Wall Thickness", thickness, "mm", "solid");
        }
        Pl::Rules { pick, name, values, active, loaded } => {
            let rules = app.session.doc.plastic_rules();
            let mut names: Vec<String> = rules.iter().map(|r| r.name.clone()).collect();
            names.push("New rule…".into());
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            row_label(ui, "Rule");
            combo(ui, "pl_rules", &refs, pick);
            ui.end_row();
            if *loaded != Some(*pick) {
                let base = rules.get(*pick).or(rules.first()).cloned();
                if let Some(b) = base {
                    *values = std::iter::once(b.material.clone()).chain(b.fields().iter().map(|(_, e, _)| (*e).clone())).collect();
                    *active = app.session.doc.plastic.active.as_deref() == Some(b.name.as_str()) && *pick < rules.len();
                }
                if *pick >= rules.len() {
                    *name = format!("Plastic Rule{}", app.session.doc.plastic.rules.len() + 1);
                }
                *loaded = Some(*pick);
            }
            if *pick >= rules.len() {
                row_label(ui, "Name");
                ui.text_edit_singleline(name);
                ui.end_row();
            }
            let keys = rule_keys();
            for (i, v) in values.iter_mut().enumerate() {
                let label = if i == 0 { "Material".to_string() } else { keys.get(i - 1).map(|k| label_of(k)).unwrap_or_default() };
                row_label(ui, &label);
                enter |= if i == 0 { ui.text_edit_singleline(v).lost_focus() && ui.input(|x| x.key_pressed(egui::Key::Enter)) } else { field(ui, v) };
                ui.end_row();
            }
            row_label(ui, "Active rule");
            ui.checkbox(active, "");
            ui.end_row();
        }
        Pl::Assign { rule } => {
            let mut names: Vec<String> = vec!["(no rule)".into()];
            names.extend(app.session.doc.plastic_rules().into_iter().map(|r| r.name));
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            row_label(ui, "Rule");
            combo(ui, "pl_assign", &refs, rule);
            ui.end_row();
        }
    }
    enter
}

/// The rule's value fields, in the order `PlasticRule::fields` gives them.
fn rule_keys() -> Vec<&'static str> {
    PlasticRule::library().first().map(|r| r.fields().iter().map(|(k, _, _)| *k).collect()).unwrap_or_default()
}

fn label_of(key: &str) -> String {
    key.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn pt(v: Vec3) -> Value {
    json!([v.x, v.y, v.z])
}

/// Put non-empty optional values into the parameters.
fn put(p: &mut Value, list: &[(&str, &String)]) {
    for (k, v) in list {
        if !v.trim().is_empty() {
            p[*k] = json!(v);
        }
    }
}

/// The commands the dialog's OK runs (`extra`: parameters kept from the feature being edited).
pub fn commands(app: &SolveApp, k: &Pl, inputs: &[SelInput], extra: &Map<String, Value>) -> Result<Vec<(String, Value)>, String> {
    let face = inputs
        .first()
        .and_then(|i| i.items.first())
        .and_then(|x| if let Sel::Face { body, point, .. } = x { Some((body.clone(), *point)) } else { None });
    let placed = |what: &str| -> Result<Value, String> {
        let (body, at) = face.clone().ok_or_else(|| format!("select {what} first"))?;
        Ok(json!({"position": pt(at), "body": body}))
    };
    let (cmd, mut p): (&str, Value) = match k {
        Pl::Boss { diameter, height, hole_diameter, hole_depth, draft, fillet, ribs, rib_thickness, rib_length, rib_offset } => {
            let mut p = placed("a point on a face")?;
            p["diameter"] = json!(diameter);
            p["height"] = json!(height);
            put(&mut p, &[("hole_diameter", hole_diameter), ("hole_depth", hole_depth), ("draft", draft), ("fillet", fillet)]);
            if app.session.doc.eval(ribs, ValueKind::Unitless).is_ok_and(|n| n >= 1.0) {
                put(&mut p, &[("ribs", ribs), ("rib_thickness", rib_thickness), ("rib_length", rib_length), ("rib_offset", rib_offset)]);
            }
            ("plastic.boss", p)
        }
        Pl::Lip { groove, width, height, gap, outside } => {
            let (body, at) = face.clone().ok_or("select the rim face first")?;
            let mut p = json!({"face": pt(at), "body": body, "width": width, "height": height,
                "type": if *groove { "groove" } else { "lip" }, "side": if *outside { "outside" } else { "inside" }});
            if *groove {
                put(&mut p, &[("gap", gap)]);
            }
            ("plastic.lip", p)
        }
        Pl::SnapFit { hook, length, thickness, width, catch_depth, catch_length } => {
            let mut p = placed("a point on a face")?;
            let n = extra
                .get("direction")
                .and_then(|v| Some(Vec3::new(v.get(0)?.as_f64()?, v.get(1)?.as_f64()?, v.get(2)?.as_f64()?)))
                .or(face_normal(app, inputs));
            let dir = n.map(hook_dirs).and_then(|d| d.get(*hook).map(|x| x.1)).ok_or("pick a planar face")?;
            p["hook"] = pt(dir);
            for (key, v) in
                [("length", length), ("thickness", thickness), ("width", width), ("catch_depth", catch_depth), ("catch_length", catch_length)]
            {
                p[key] = json!(v);
            }
            ("plastic.snap_fit", p)
        }
        Pl::Rest { round, width, length, height, draft, thickness } => {
            let mut p = placed("a point on a face")?;
            p["width"] = json!(width);
            p["height"] = json!(height);
            if !*round {
                p["length"] = json!(length);
            }
            put(&mut p, &[("draft", draft), ("thickness", thickness)]);
            ("plastic.rest", p)
        }
        Pl::Rules { pick, name, values, active, .. } => {
            let rules = app.session.doc.plastic_rules();
            let n = match rules.get(*pick) {
                Some(r) => r.name.clone(),
                None => name.trim().to_string(),
            };
            if n.is_empty() {
                return Err("give the rule a name".into());
            }
            let mut p = json!({"name": n, "active": active});
            if let Some(m) = values.first().filter(|m| !m.trim().is_empty()) {
                p["material"] = json!(m);
            }
            for (key, v) in rule_keys().iter().zip(values.iter().skip(1)) {
                if !v.trim().is_empty() {
                    p[*key] = json!(v);
                }
            }
            ("plastic.manage_rules", p)
        }
        Pl::Assign { rule } => {
            let bodies: Vec<String> = inputs
                .first()
                .into_iter()
                .flat_map(|i| &i.items)
                .filter_map(|x| if let Sel::Body { name } = x { Some(name.clone()) } else { None })
                .collect();
            if bodies.is_empty() {
                return Err("select bodies first".into());
            }
            let name = rule.checked_sub(1).and_then(|i| app.session.doc.plastic_rules().get(i).map(|r| r.name.clone())).unwrap_or_default();
            ("plastic.assign_rule", json!({"bodies": bodies, "rule": name}))
        }
    };
    if let Value::Object(m) = &mut p {
        for (key, v) in extra {
            m.entry(key.clone()).or_insert_with(|| v.clone());
        }
    }
    Ok(vec![(cmd.to_string(), p)])
}

fn s(o: &Option<solidcraft_engine::doc::Expr>) -> String {
    o.as_deref().map(str::to_string).unwrap_or_default()
}

/// The dialog editing a plastic feature, filled from it.
pub fn for_feature(app: &SolveApp, kind: &FeatureKind) -> Option<(Kind, Vec<SelInput>, Map<String, Value>)> {
    let ss = &app.session;
    let mut extra = Map::new();
    let (pl, id, at): (Pl, &str, Vec3) = match kind {
        FeatureKind::Boss {
            position,
            direction,
            diameter,
            height,
            hole_diameter,
            hole_depth,
            draft,
            fillet,
            ribs,
            rib_thickness,
            rib_length,
            rib_offset,
            body,
        } => {
            extra.insert("direction".into(), pt(direction.0));
            if let Some(b) = body {
                extra.insert("body".into(), json!(b));
            }
            let mut b = boss();
            if let Pl::Boss {
                diameter: d,
                height: h,
                hole_diameter: hd,
                hole_depth: hp,
                draft: dr,
                fillet: f,
                ribs: r,
                rib_thickness: rt,
                rib_length: rl,
                rib_offset: ro,
            } = &mut b
            {
                *d = shown_expr(diameter, ValueKind::Length);
                *h = shown_expr(height, ValueKind::Length);
                *hd = shown_expr(&s(hole_diameter), ValueKind::Length);
                *hp = shown_expr(&s(hole_depth), ValueKind::Length);
                *dr = shown_expr(&s(draft), ValueKind::Angle);
                *f = shown_expr(&s(fillet), ValueKind::Length);
                *r = ribs.as_deref().map(str::to_string).unwrap_or_else(|| "0".into());
                for (slot, v) in [(rt, rib_thickness), (rl, rib_length), (ro, rib_offset)] {
                    if let Some(v) = v {
                        *slot = shown_expr(v, ValueKind::Length);
                    }
                }
            }
            (b, "plastic.boss", position.0)
        }
        FeatureKind::Lip { face, width, height, groove, gap, outside, body } => {
            if let Some(b) = body {
                extra.insert("body".into(), json!(b));
            }
            (
                Pl::Lip {
                    groove: *groove,
                    width: shown_expr(width, ValueKind::Length),
                    height: shown_expr(height, ValueKind::Length),
                    gap: shown_expr(&s(gap), ValueKind::Length),
                    outside: *outside,
                },
                "plastic.lip",
                face.0,
            )
        }
        FeatureKind::SnapFit { position, direction, hook, length, thickness, width, catch_depth, catch_length, body } => {
            extra.insert("direction".into(), pt(direction.0));
            if let Some(b) = body {
                extra.insert("body".into(), json!(b));
            }
            let h = hook_dirs(direction.0).iter().position(|(_, d)| d.dist(hook.0) < 1e-6);
            if h.is_none() {
                extra.insert("hook".into(), pt(hook.0));
            }
            let f = Pl::SnapFit {
                hook: h.unwrap_or(0),
                length: shown_expr(length, ValueKind::Length),
                thickness: shown_expr(thickness, ValueKind::Length),
                width: shown_expr(width, ValueKind::Length),
                catch_depth: shown_expr(catch_depth, ValueKind::Length),
                catch_length: shown_expr(catch_length, ValueKind::Length),
            };
            (f, "plastic.snap_fit", position.0)
        }
        FeatureKind::Rest { position, direction, along, width, length, height, draft, thickness, body } => {
            extra.insert("direction".into(), pt(direction.0));
            if let Some(a) = along {
                extra.insert("along".into(), pt(a.0));
            }
            if let Some(b) = body {
                extra.insert("body".into(), json!(b));
            }
            let r = Pl::Rest {
                round: length.is_none(),
                width: shown_expr(width, ValueKind::Length),
                length: length.as_deref().map(|l| shown_expr(l, ValueKind::Length)).unwrap_or_else(|| "15 mm".into()),
                height: shown_expr(height, ValueKind::Length),
                draft: shown_expr(&s(draft), ValueKind::Angle),
                thickness: shown_expr(&s(thickness), ValueKind::Length),
            };
            (r, "plastic.rest", position.0)
        }
        _ => return None,
    };
    let (_, mut inputs) = start(app, id)?;
    if let Some(i) = inputs.first_mut() {
        i.items = face_sel(ss, at).into_iter().collect();
    }
    // A pick that no longer finds its face keeps the stored point.
    if let Some(i) = inputs.first_mut().filter(|i| i.items.is_empty()) {
        i.items = vec![Sel::Face { body: extra.get("body").and_then(Value::as_str).unwrap_or_default().to_string(), index: 0, point: at }];
    }
    Some((Kind::Plastic(pl), inputs, extra))
}

#[cfg(test)]
#[path = "dialogs_plastic_tests.rs"]
mod tests;
