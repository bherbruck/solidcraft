//! Plastic (enclosure) features: Boss, Lip / Groove, Snap Fit and Rest, and plastic rules.

use serde_json::{Value, json};
use solidcraft_doc::BodyName;
use solidcraft_doc::FeatureKind;
use solidcraft_doc::expr::Kind;
use solidcraft_doc::plastic::PlasticRule;
use solidcraft_doc::{Dir, FacePick, Position};
use solidcraft_geom::Vec3;

use super::CommandSpec;
use super::features::{add_feature, check_expr};
use crate::params::{bad, bool_, expr, req_expr, str_, string_list, vec3};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("plastic.boss", "Boss", boss).at("PLASTIC", "CREATE").icon("boss").params(
        "position: [x,y,z] on a face; direction? (default: out of the face); diameter, height; hole_diameter?, hole_depth? (default: to the face); \
         draft? (angle), fillet? (root radius); ribs? (count), rib_thickness?, rib_length? (out from the wall), rib_offset? (below the top); body?",
    ),
    CommandSpec::new("plastic.lip", "Lip", lip).at("PLASTIC", "CREATE").icon("lip").params(
        "face: [x,y,z] on the rim face of a shelled body; width, height; type?: lip|groove; gap? (groove clearance); side?: inside|outside (which rim edge); body?",
    ),
    CommandSpec::new("plastic.snap_fit", "Snap Fit", snap_fit).at("PLASTIC", "CREATE").icon("snap_fit").params(
        "position: [x,y,z] on a face; direction? (default: out of the face); hook: [x,y,z] (the catch's direction); length, thickness, width, catch_depth, catch_length; body?",
    ),
    CommandSpec::new("plastic.rest", "Rest", rest).at("PLASTIC", "CREATE").icon("rest").params(
        "position: [x,y,z] on a face (the rest's centre); direction? (default: out of the face); width (round: the diameter); length? (a rectangle, along `along`); along?: [x,y,z]; \
         height; draft? (angle; default: the body's plastic rule); thickness? (wall of a hollow rest); body?",
    ),
    CommandSpec::new("plastic.manage_rules", "Manage Plastic Rules", manage_rules).at("PLASTIC", "SETUP").icon("plastic_rule").params(
        "name (library or new rule; a new one starts from `from`, default ABS (1.5mm)); from?; material?; thickness?, nominal_radius?, clearance?, knife_edge?, \
         reveal_height?, thickness_variation?, draft?, max_thickness?, min_thickness?, min_draft? (expressions; `Thickness` is the rule's); active?: bool (rule for bodies without one); \
         delete?: bool (a design rule; a library rule goes back to the library values)",
    ),
    CommandSpec::new("plastic.assign_rule", "Assign Plastic Rule", assign_rule).at("PLASTIC", "SETUP").icon("plastic_assign").params(
        "bodies: [names]; rule: name (empty: no rule) — plastic features on these bodies take its draft and clearance as defaults",
    ),
    CommandSpec::new("plastic.rules", "List Plastic Rules", list_rules).icon("plastic_rule").noundo().params("→ rules with their expressions and values, the active rule and the rules assigned to bodies"),
];

fn rule_json(s: &Session, r: &PlasticRule) -> Value {
    let (vals, _) = s.doc.param_values();
    let values = s.doc.plastic_values(&vals, r).map(|v| {
        json!({"thickness": v.thickness, "nominal_radius": v.nominal_radius, "clearance": v.clearance, "knife_edge": v.knife_edge,
            "reveal_height": v.reveal_height, "thickness_variation": v.thickness_variation, "draft_deg": v.draft.to_degrees(),
            "max_thickness": v.max_thickness, "min_thickness": v.min_thickness, "min_draft_deg": v.min_draft.to_degrees()})
    });
    let library = PlasticRule::library().iter().any(|l| l.name == r.name);
    json!({
        "name": r.name, "material": r.material, "library": library,
        "expressions": r.fields().iter().map(|(k, e, _)| (k.to_string(), json!(e))).collect::<serde_json::Map<_, _>>(),
        "values": values.unwrap_or_else(|e| json!({"error": e.to_string()})),
    })
}

fn manage_rules(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plastic.manage_rules";
    let name = str_(p, "name").map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128).ok_or_else(|| bad(cmd, "`name` is required"))?.to_string();
    if bool_(p, "delete").unwrap_or(false) {
        let doc = s.doc_mut();
        let before = doc.plastic.rules.len();
        doc.plastic.rules.retain(|r| r.name != name);
        if doc.plastic.rules.len() == before {
            return Err(bad(cmd, format!("no design rule `{name}`")));
        }
        if doc.plastic_rule(&name).is_none() {
            if doc.plastic.active.as_deref() == Some(name.as_str()) {
                doc.plastic.active = None;
            }
            doc.plastic.assigned.retain(|_, r| *r != name);
        }
        return Ok(json!({"deleted": name}));
    }
    let mut rule = match s.doc.plastic_rule(&name) {
        Some(r) => r,
        None => {
            let from = str_(p, "from").unwrap_or("ABS (1.5mm)");
            let base = s.doc.plastic_rule(from).ok_or_else(|| bad(cmd, format!("no plastic rule `{from}` to start from")))?;
            PlasticRule { name: name.clone(), ..base }
        }
    };
    if let Some(m) = str_(p, "material") {
        rule.material = m.chars().take(128).collect();
    }
    for (k, _, kind) in PlasticRule::library()[0].fields() {
        if let Some(e) = expr(p, k) {
            // `Thickness` is the rule's own: check the others with it standing in.
            let probe = e.replace("Thickness", "(1 mm)");
            check_expr(s, &probe, kind, cmd, k)?;
            if let Some(slot) = rule.field_mut(k) {
                *slot = e.into();
            }
        }
    }
    let (vals, _) = s.doc.param_values();
    s.doc.plastic_values(&vals, &rule).map_err(|e| bad(cmd, e.to_string()))?;
    let changed = s.doc.plastic_rule(&name).as_ref() != Some(&rule);
    let doc = s.doc_mut();
    if changed {
        match doc.plastic.rules.iter_mut().find(|r| r.name == name) {
            Some(r) => *r = rule.clone(),
            None => {
                if doc.plastic.rules.len() >= 1000 {
                    return Err(EngineError::Other("too many rules".into()));
                }
                doc.plastic.rules.push(rule.clone());
            }
        }
    }
    match bool_(p, "active") {
        Some(true) => doc.plastic.active = Some(name.clone()),
        Some(false) if doc.plastic.active.as_deref() == Some(name.as_str()) => doc.plastic.active = None,
        _ => {}
    }
    s.refresh();
    Ok(json!({"rule": rule_json(s, &rule), "active": s.doc.plastic.active}))
}

fn assign_rule(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plastic.assign_rule";
    let bodies = string_list(p, "bodies");
    if bodies.is_empty() || bodies.len() > 10_000 {
        return Err(bad(cmd, "`bodies` must list bodies"));
    }
    let st = s.model.state();
    if let Some(b) = bodies.iter().find(|b| st.body(b).is_none()) {
        return Err(bad(cmd, format!("no body `{b}`")));
    }
    let rule = str_(p, "rule").map(str::trim).unwrap_or_default().to_string();
    if !rule.is_empty() && s.doc.plastic_rule(&rule).is_none() {
        return Err(bad(cmd, format!("no plastic rule `{rule}` (plastic.rules lists them)")));
    }
    let doc = s.doc_mut();
    for b in &bodies {
        if rule.is_empty() {
            doc.plastic.assigned.remove(b);
        } else {
            doc.plastic.assigned.insert(b.clone(), rule.clone());
        }
    }
    s.refresh();
    Ok(json!({"bodies": bodies, "rule": if rule.is_empty() { Value::Null } else { json!(rule) }}))
}

fn list_rules(s: &mut Session, _p: &Value) -> Result<Value> {
    let rules: Vec<Value> = s.doc.plastic_rules().iter().map(|r| rule_json(s, r)).collect();
    Ok(json!({"rules": rules, "active": s.doc.plastic.active, "assigned": s.doc.plastic.assigned}))
}

fn rest(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plastic.rest";
    let (position, direction) = placed(s, p, cmd)?;
    let along = match p.get("along") {
        Some(v) => Some(
            vec3(v)
                .and_then(|a| a.normalized())
                .filter(|a| a.cross(direction).len() > 1e-6)
                .ok_or_else(|| bad(cmd, "`along` must be a direction across the face"))?,
        ),
        None => None,
    };
    let kind = FeatureKind::Rest {
        position: FacePick(position),
        direction: Dir(direction),
        along: along.map(Dir),
        width: required(s, p, "width", Kind::Length, cmd)?,
        length: checked(s, p, "length", Kind::Length, cmd)?,
        height: required(s, p, "height", Kind::Length, cmd)?,
        draft: checked(s, p, "draft", Kind::Angle, cmd)?,
        thickness: checked(s, p, "thickness", Kind::Length, cmd)?,
        body: str_(p, "body").map(BodyName::from),
    };
    add_feature(s, p, kind)
}

fn checked(s: &Session, p: &Value, k: &str, kind: Kind, cmd: &str) -> Result<Option<solidcraft_doc::Expr>> {
    match expr(p, k) {
        Some(e) => {
            check_expr(s, &e, kind, cmd, k)?;
            Ok(Some(e))
        }
        None => Ok(None),
    }
}

fn required(s: &Session, p: &Value, k: &str, kind: Kind, cmd: &str) -> Result<solidcraft_doc::Expr> {
    let e = req_expr(cmd, p, k)?;
    check_expr(s, &e, kind, cmd, k)?;
    Ok(e)
}

/// A point on a face and the direction out of it.
fn placed(s: &Session, p: &Value, cmd: &str) -> Result<(Vec3, Vec3)> {
    let at = p.get("position").and_then(vec3).ok_or_else(|| bad(cmd, "`position` must be a point [x, y, z] on a face"))?;
    let dir = match p.get("direction") {
        Some(v) => vec3(v).and_then(|d| d.normalized()).ok_or_else(|| bad(cmd, "`direction` must be a non-zero [x, y, z]"))?,
        None => super::face::face_normal(s, at).ok_or_else(|| bad(cmd, "no planar face there (give `direction`)"))?,
    };
    Ok((at, dir))
}

fn boss(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plastic.boss";
    let (position, direction) = placed(s, p, cmd)?;
    let kind = FeatureKind::Boss {
        position: FacePick(position),
        direction: Dir(direction),
        diameter: required(s, p, "diameter", Kind::Length, cmd)?,
        height: required(s, p, "height", Kind::Length, cmd)?,
        hole_diameter: checked(s, p, "hole_diameter", Kind::Length, cmd)?,
        hole_depth: checked(s, p, "hole_depth", Kind::Length, cmd)?,
        draft: checked(s, p, "draft", Kind::Angle, cmd)?,
        fillet: checked(s, p, "fillet", Kind::Length, cmd)?,
        ribs: checked(s, p, "ribs", Kind::Unitless, cmd)?,
        rib_thickness: checked(s, p, "rib_thickness", Kind::Length, cmd)?,
        rib_length: checked(s, p, "rib_length", Kind::Length, cmd)?,
        rib_offset: checked(s, p, "rib_offset", Kind::Length, cmd)?,
        body: str_(p, "body").map(BodyName::from),
    };
    add_feature(s, p, kind)
}

fn lip(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plastic.lip";
    let face = p.get("face").and_then(vec3).ok_or_else(|| bad(cmd, "`face` must be a point [x, y, z] on the rim face"))?;
    let groove = match str_(p, "type").map(str::to_ascii_lowercase).as_deref() {
        None | Some("lip") => false,
        Some("groove") => true,
        Some(o) => return Err(bad(cmd, format!("unknown type `{o}` (lip or groove)"))),
    };
    let outside = match str_(p, "side").map(str::to_ascii_lowercase).as_deref() {
        None | Some("inside") => false,
        Some("outside") => true,
        Some(o) => return Err(bad(cmd, format!("unknown side `{o}` (inside or outside)"))),
    };
    let kind = FeatureKind::Lip {
        face: Position(face),
        width: required(s, p, "width", Kind::Length, cmd)?,
        height: required(s, p, "height", Kind::Length, cmd)?,
        groove,
        gap: checked(s, p, "gap", Kind::Length, cmd)?,
        outside: outside || bool_(p, "outside").unwrap_or(false),
        body: str_(p, "body").map(BodyName::from),
    };
    add_feature(s, p, kind)
}

fn snap_fit(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plastic.snap_fit";
    let (position, direction) = placed(s, p, cmd)?;
    let hook = p.get("hook").and_then(vec3).and_then(|h| h.normalized()).ok_or_else(|| bad(cmd, "`hook` must be the catch's direction [x, y, z]"))?;
    if hook.cross(direction).len() < 1e-6 {
        return Err(bad(cmd, "`hook` must point across the arm"));
    }
    let kind = FeatureKind::SnapFit {
        position: FacePick(position),
        direction: Dir(direction),
        hook: Dir(hook),
        length: required(s, p, "length", Kind::Length, cmd)?,
        thickness: required(s, p, "thickness", Kind::Length, cmd)?,
        width: required(s, p, "width", Kind::Length, cmd)?,
        catch_depth: required(s, p, "catch_depth", Kind::Length, cmd)?,
        catch_length: required(s, p, "catch_length", Kind::Length, cmd)?,
        body: str_(p, "body").map(BodyName::from),
    };
    add_feature(s, p, kind)
}

#[cfg(test)]
mod tests {
    use solidcraft_geom::Vec3;
    use std::f64::consts::PI;

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

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs().max(1e-12)
    }

    #[test]
    fn boss_with_hole_fillet_and_ribs() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 60, "width": 60, "height": 3, "body_name": "Plate"}));
        run(&mut s, "plastic.boss", json!({"position": [30, 30, 3], "diameter": 10, "height": 12, "hole_diameter": 4}));
        let want = 10800.0 + PI * 25.0 * 12.0 - PI * 4.0 * 12.0;
        assert!(rel(volume(&mut s), want) < 1e-3, "{} {want}", volume(&mut s));
        assert_eq!(run(&mut s, "inspect.measure", json!({}))["body_count"], 1);
        // Ribs add t·l·h each.
        let mut t = Session::default();
        run(&mut t, "solid.box", json!({"length": 60, "width": 60, "height": 3}));
        run(
            &mut t,
            "plastic.boss",
            json!({"position": [30, 30, 3], "diameter": 10, "height": 12, "ribs": 4, "rib_thickness": 1.5, "rib_length": 6, "rib_offset": 2}),
        );
        let ribs = 4.0 * 1.5 * 6.0 * 10.0;
        // Each rib also fills a sliver between its flat inner end and the round wall.
        let v = volume(&mut t) - 10800.0 - PI * 25.0 * 12.0;
        assert!(v > ribs && v < ribs * 1.05, "{v} {ribs}");
        let mut u = Session::default();
        run(&mut u, "solid.box", json!({"length": 60, "width": 60, "height": 3}));
        run(&mut u, "plastic.boss", json!({"position": [30, 30, 3], "diameter": 10, "height": 12, "fillet": 2, "draft": 1}));
        assert!(volume(&mut u) > 10800.0 + PI * 25.0 * 10.0);
        assert!(u.execute("plastic.boss", &json!({"position": [30, 30, 3], "diameter": 10, "height": 12, "hole_diameter": 20})).is_err());
    }

    #[test]
    fn lip_and_groove_on_a_shelled_box() {
        let shelled = || {
            let mut s = Session::default();
            run(&mut s, "solid.box", json!({"length": 80, "width": 50, "height": 30}));
            run(&mut s, "solid.shell", json!({"faces": [[40, 25, 30]], "thickness": 2}));
            s
        };
        let mut s = shelled();
        let v0 = volume(&mut s);
        run(&mut s, "plastic.lip", json!({"face": [1, 25, 30], "width": 1, "height": 2}));
        // Inside band 1 wide along the inner edge (76 x 46): (76+2)(46+2) − 76·46 = 248, 2 high.
        assert!(rel(volume(&mut s) - v0, 248.0 * 2.0) < 1e-6, "{}", volume(&mut s) - v0);
        let mut g = shelled();
        run(&mut g, "plastic.lip", json!({"face": [1, 25, 30], "width": 1, "height": 2, "type": "groove", "side": "outside"}));
        // Outside band along the outer edge (80 x 50): 80·50 − 78·48 = 256, 2 deep.
        assert!(rel(v0 - volume(&mut g), 256.0 * 2.0) < 1e-6, "{}", v0 - volume(&mut g));
        assert!(g.execute("plastic.lip", &json!({"face": [40, 25, 0], "width": 1, "height": 2})).is_err(), "a face without an inner edge");
    }

    #[test]
    fn lip_on_a_face_that_is_not_a_rim_says_so() {
        // The Sample Plate's top face (#892): two holes through it and the boss standing on it.
        // The lip used to run round the boss's foot, against its wall, and the join failed in
        // the boolean; it is refused as no rim instead.
        let mut s = Session::default();
        s.run_script(&crate::sample::script()).unwrap();
        let lip = json!({"body": "Plate", "face": [51.89, 45.2, 8.0], "width": "1.5 mm", "height": "3.5 mm", "side": "inside", "type": "lip"});
        let e = s.execute("plastic.lip", &lip).unwrap_err().to_string();
        assert!(e.contains("rim face") && e.contains("2 openings alike"), "{e}");
        // The boss alone, no holes: its foot is not an opening either.
        let mut b = Session::default();
        run(&mut b, "solid.box", json!({"length": 80, "width": 50, "height": 8, "body_name": "Plate"}));
        run(&mut b, "solid.cylinder", json!({"base": [40, 25, 8], "axis": [0, 0, 1], "radius": 12, "height": 14, "operation": "join"}));
        let e = b.execute("plastic.lip", &lip).unwrap_err().to_string();
        assert!(e.contains("stand on it"), "{e}");
    }

    #[test]
    fn snap_fit_arm_and_catch() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 40, "width": 40, "height": 4}));
        let v0 = volume(&mut s);
        run(
            &mut s,
            "plastic.snap_fit",
            json!({"position": [20, 20, 4], "hook": [1, 0, 0], "length": 15, "thickness": 1.5, "width": 6, "catch_depth": 1, "catch_length": 3}),
        );
        let want = 1.5 * 15.0 * 6.0 + 0.5 * 1.0 * 3.0 * 6.0;
        assert!(rel(volume(&mut s) - v0, want) < 1e-6, "{}", volume(&mut s) - v0);
        let m = run(&mut s, "inspect.measure", json!({}));
        assert!((m["bodies"][0]["bbox"]["max"][0].as_f64().unwrap_or(0.0) - 40.0).abs() < 1e-9);
        assert!(s.execute("plastic.snap_fit", &json!({"position": [5, 5, 4], "direction": [0, 0, 1], "hook": [0, 0, 1], "length": 15, "thickness": 1.5, "width": 6, "catch_depth": 1, "catch_length": 3})).is_err());
    }

    #[test]
    fn enclosure_snap_fit_follows_resized_front_wall() {
        let mut s = Session::default();
        let script: Value = serde_json::from_str(include_str!("../../../../examples/acceptance/sensor_enclosure.json")).unwrap();
        s.run_script(&script).unwrap();
        let feature = s.doc.find_feature("SnapFit1").unwrap().clone();
        run(&mut s, "parameters.change", json!({"name":"board_d","expression":"50 mm"}));
        let before = s.model.state_before(feature.id);
        let name = feature.face_names.first().unwrap();
        let point = solidcraft_doc::naming::point_on_face(&before, name, Vec3::new(0., -28., 20.))
            .expect("the original wall identity survives merged pieces")
            .0;
        assert!((point - Vec3::new(0., -33., 20.)).len() < 1e-8, "{point:?}");
        assert!(s.model.results.iter().all(|result| result.error.is_none()));
        let check = |session: &Session| {
            let before = session.model.state_before(feature.id);
            let after = &session.model.result(feature.id).unwrap().state;
            let old = before.body("Case").unwrap();
            let body = after.body("Case").unwrap();
            let added = solidcraft_kernel::measure(&body.body).unwrap().volume - solidcraft_kernel::measure(&old.body).unwrap().volume;
            // Rectangular arm + triangular catch; the small attachment overlap lies inside the wall.
            assert!((added - (8. * 1.2 + 0.8 * 1.5 / 2.) * 6.).abs() < 0.05, "{added}");
            let mesh = body.mesh();
            assert!(mesh.contains(Vec3::new(0., -29., 19.4)), "the arm follows the inner wall");
            assert!(mesh.contains(Vec3::new(0., -25.9, 20.2)), "the catch follows the arm");
            assert!(!mesh.contains(Vec3::new(0., -22., 19.4)), "the old arm placement is empty");
            assert!(!mesh.contains(Vec3::new(0., -24., 19.4)), "the arm ends at its specified length");
        };
        check(&s);
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!(cold.model.results.iter().all(|result| result.error.is_none()));
        check(&cold);
    }

    fn plate() -> (Session, f64) {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 60, "width": 60, "height": 3}));
        let v0 = volume(&mut s);
        (s, v0)
    }

    #[test]
    fn rest_pads() {
        // Rectangle 20 x 10, 4 high: 800.
        let (mut s, v0) = plate();
        run(&mut s, "plastic.rest", json!({"position": [30, 30, 3], "width": 10, "length": 20, "along": [1, 0, 0], "height": 4}));
        assert!(rel(volume(&mut s) - v0, 800.0) < 1e-6, "{}", volume(&mut s) - v0);
        let m = run(&mut s, "inspect.measure", json!({}));
        assert!((m["bodies"][0]["bbox"]["max"][2].as_f64().unwrap_or(0.0) - 7.0).abs() < 1e-9);
        // Round, diameter 10: π·25·4.
        let (mut r, v0) = plate();
        run(&mut r, "plastic.rest", json!({"position": [30, 30, 3], "width": 10, "height": 4}));
        assert!(rel(volume(&mut r) - v0, PI * 25.0 * 4.0) < 1e-3, "{}", volume(&mut r) - v0);
        // Hollow, wall 1: (20·10 − 18·8)·4 = 224.
        let (mut h, v0) = plate();
        run(&mut h, "plastic.rest", json!({"position": [30, 30, 3], "width": 10, "length": 20, "height": 4, "thickness": 1}));
        assert!(rel(volume(&mut h) - v0, 224.0) < 1e-6, "{}", volume(&mut h) - v0);
        // Drafted 2°: the section shrinks by 2·s·tan 2° each way at height s.
        let (mut d, v0) = plate();
        run(&mut d, "plastic.rest", json!({"position": [30, 30, 3], "width": 10, "length": 20, "height": 4, "draft": "2 deg"}));
        let a = 2f64.to_radians().tan();
        let want = 800.0 - a * 30.0 * 16.0 + 4.0 / 3.0 * a * a * 64.0;
        assert!(rel(volume(&mut d) - v0, want) < 1e-6, "{} {want}", volume(&mut d) - v0);
        // A wall thicker than half the width, or a draft that closes the top, is refused.
        assert!(d.execute("plastic.rest", &json!({"position": [10, 10, 3], "width": 4, "height": 4, "thickness": 2})).is_err());
        assert!(d.execute("plastic.rest", &json!({"position": [10, 10, 3], "width": 4, "height": 40, "draft": 10})).is_err());
    }

    #[test]
    fn plastic_rules_library_edit_and_assign() {
        let (mut s, v0) = plate();
        let list = run(&mut s, "plastic.rules", json!({}));
        assert_eq!(list["rules"].as_array().map(Vec::len), Some(6));
        assert_eq!(list["rules"][0]["name"], "ABS (1.5mm)");
        assert_eq!(list["rules"][0]["values"]["nominal_radius"], 0.75);
        assert_eq!(list["rules"][3]["values"]["thickness"].as_f64().map(|t| (t - 2.54).abs() < 1e-9), Some(true));
        // A new rule from ABS: Thickness-based values follow its thickness.
        let r = run(&mut s, "plastic.manage_rules", json!({"name": "Thick ABS", "thickness": 3, "draft": "1.5 deg"}));
        assert_eq!(r["rule"]["values"]["nominal_radius"], 1.5);
        assert!(s.execute("plastic.manage_rules", &json!({"name": "Bad", "draft": "3 mm"})).is_err());
        // Assigned to the plate: a boss without a draft takes the rule's 1.5°.
        let body = run(&mut s, "inspect.measure", json!({}))["bodies"][0]["name"].as_str().unwrap_or_default().to_string();
        run(&mut s, "plastic.assign_rule", json!({"bodies": [body], "rule": "Thick ABS"}));
        run(&mut s, "plastic.boss", json!({"position": [30, 30, 3], "diameter": 10, "height": 12}));
        let rt = 5.0 - 12.0 * 1.5f64.to_radians().tan();
        let want = PI * 12.0 / 3.0 * (25.0 + 5.0 * rt + rt * rt);
        assert!(rel(volume(&mut s) - v0, want) < 1e-3, "{} {want}", volume(&mut s) - v0);
        // Editing the rule rebuilds the boss.
        run(&mut s, "plastic.manage_rules", json!({"name": "Thick ABS", "draft": "0 deg"}));
        assert!(rel(volume(&mut s) - v0, PI * 25.0 * 12.0) < 1e-3, "{}", volume(&mut s) - v0);
        assert!(s.execute("plastic.assign_rule", &json!({"bodies": [body], "rule": "Nope"})).is_err());
        assert!(s.execute("plastic.assign_rule", &json!({"bodies": ["Nope"], "rule": "Thick ABS"})).is_err());
    }

    #[test]
    fn groove_takes_the_rule_clearance() {
        let mut g = Session::default();
        run(&mut g, "solid.box", json!({"length": 80, "width": 50, "height": 30}));
        run(&mut g, "solid.shell", json!({"faces": [[40, 25, 30]], "thickness": 2}));
        let v0 = volume(&mut g);
        run(&mut g, "plastic.manage_rules", json!({"name": "ABS (1.5mm)", "active": true}));
        run(&mut g, "plastic.lip", json!({"face": [1, 25, 30], "width": 1, "height": 2, "type": "groove", "side": "outside"}));
        // Width and depth grow by the 0.1 clearance: (80·50 − 77.8·47.8)·2.1.
        let want = (4000.0 - 77.8 * 47.8) * 2.1;
        assert!(rel(v0 - volume(&mut g), want) < 1e-6, "{} {want}", v0 - volume(&mut g));
    }
}
