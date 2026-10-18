//! `timeline.export_script` and `timeline.add` (#886): a design's script replays into the same
//! design, and a recorded feature that does not fit the design is refused, changing nothing.

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn script(s: &mut Session) -> Value {
    let r = run(s, "timeline.export_script", json!({}));
    // Through text, as a script file carries it.
    serde_json::from_str(&r["script"].to_string()).unwrap()
}

fn replay(script: &Value) -> Session {
    let mut t = Session::default();
    if let Err(e) = t.run_script(script) {
        panic!("replay: {e}\n{script:#}");
    }
    t
}

fn volume(s: &mut Session) -> f64 {
    run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap_or(f64::NAN)
}

fn params(s: &Session) -> Vec<solidcraft_doc::Param> {
    let mut v = s.doc.params.clone();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

fn errors(s: &Session) -> Vec<(u64, Option<String>)> {
    s.model.results.iter().map(|r| (r.id, r.error.clone())).collect()
}

fn same(a: &mut Session, b: &mut Session) {
    assert_eq!(a.doc.features, b.doc.features, "timeline");
    assert_eq!(params(a), params(b), "parameters");
    assert_eq!(a.doc.marker, b.doc.marker, "marker");
    assert_eq!(a.doc.favorites, b.doc.favorites, "favourites");
    assert_eq!(a.doc.param_comments, b.doc.param_comments, "comments");
    assert_eq!(a.doc.materials, b.doc.materials, "materials");
    assert_eq!(a.doc.user_materials, b.doc.user_materials, "user materials");
    assert_eq!(errors(a), errors(b), "feature results");
    assert_eq!(volume(a), volume(b), "volume");
}

fn commands(script: &Value) -> Vec<&str> {
    script["commands"].as_array().into_iter().flatten().filter_map(|c| c["command"].as_str()).collect()
}

/// A plate from a dimensioned sketch, with a user parameter driving a dimension.
fn plate(s: &mut Session) {
    run(s, "parameters.add", json!({"name": "w", "expression": "40 mm", "comment": "plate width", "favorite": true}));
    run(s, "parameters.add", json!({"name": "count", "expression": "3", "unit": ""}));
    run(s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    run(s, "sketch.constraint.coincident", json!({"a": "p1", "b": "origin"}));
    run(s, "sketch.dimension", json!({"entities": ["l1"], "value": "w"}));
    run(s, "sketch.dimension", json!({"entities": ["l2"], "value": "w * 0.75"}));
    run(s, "sketch.finish", json!({}));
    run(s, "solid.extrude", json!({"sketch": "Base", "distance": "10 mm"}));
}

#[test]
fn a_dimensioned_part_replays_into_the_same_design() {
    let mut s = Session::default();
    plate(&mut s);
    run(&mut s, "solid.fillet", json!({"edges": [[20, 0, 10]], "radius": "2 mm"}));
    run(&mut s, "solid.chamfer", json!({"edges": [[20, 30, 10]], "distance": "1 mm"}));
    let sc = script(&mut s);
    let cmds = commands(&sc);
    assert_eq!(cmds.iter().filter(|c| **c == "timeline.add").count(), 4, "{cmds:?}");
    assert_eq!(cmds.first(), Some(&"parameters.add"), "user parameters come first: {cmds:?}");
    let mut t = replay(&sc);
    same(&mut s, &mut t);
    // Replayed parameters still drive the part.
    run(&mut t, "parameters.change", json!({"name": "w", "expression": "20 mm"}));
    run(&mut s, "parameters.change", json!({"name": "w", "expression": "20 mm"}));
    same(&mut s, &mut t);
}

#[test]
fn ids_names_suppression_marker_and_failures_survive() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [20, 0, 0], "name": "Gone"}));
    run(&mut s, "solid.box", json!({"length": 4, "width": 4, "height": 4, "corner": [40, 0, 0], "name": "Kept box"}));
    // A gap in the ids, a renamed feature, a suppressed one.
    run(&mut s, "timeline.delete", json!({"features": ["Gone"]}));
    plate(&mut s);
    run(&mut s, "timeline.suppress", json!({"feature": "Kept box", "suppressed": true}));
    // A feature input named by a user parameter that refers back to it.
    let extrude = s.doc.features.last().unwrap().clone();
    let depth = extrude.param_names.first().cloned().unwrap();
    run(&mut s, "parameters.add", json!({"name": "twice_depth", "expression": format!("{depth} * 2")}));
    run(&mut s, "parameters.comment", json!({"name": depth, "comment": "plate depth"}));
    run(&mut s, "parameters.favorite", json!({"name": depth}));
    // A fillet that a parameter change makes too big fails, and the design keeps it.
    run(&mut s, "parameters.add", json!({"name": "r", "expression": "1 mm"}));
    run(&mut s, "solid.fillet", json!({"edges": [[5, 0, 10]], "radius": "r"}));
    let fillet = s.doc.features.last().unwrap().id;
    run(&mut s, "parameters.change", json!({"name": "r", "expression": "50 mm"}));
    assert!(s.model.result(fillet).and_then(|r| r.error.clone()).is_some(), "the fillet fails");
    run(&mut s, "timeline.rollback", json!({"position": 3}));
    let mut t = replay(&script(&mut s));
    same(&mut s, &mut t);
    // The script up to a feature is the design rolled back to just after it.
    let full = script(&mut s);
    let cmds = full["commands"].as_array().unwrap();
    let cut = cmds.iter().position(|c| c["params"]["feature"]["type"] == "extrude").unwrap();
    let mut part = replay(&json!({"commands": cmds[..=cut].to_vec()}));
    let ids: Vec<u64> = part.doc.features.iter().map(|f| f.id).collect();
    assert_eq!(ids, s.doc.features.iter().map(|f| f.id).take(ids.len()).collect::<Vec<_>>());
    run(&mut s, "timeline.rollback", json!({"position": ids.len()}));
    assert_eq!(volume(&mut part), volume(&mut s));
}

#[test]
fn units_materials_and_rules_come_back() {
    let mut s = Session::default();
    run(&mut s, "document.units", json!({"units": "in"}));
    run(&mut s, "material.create", json!({"name": "Test resin", "density_g_cm3": 1.3, "color": "#336699", "opacity": 0.5}));
    run(&mut s, "solid.box", json!({"length": "1 in", "width": "1 in", "height": "1 in", "body_name": "Cube"}));
    run(&mut s, "material.assign", json!({"bodies": ["Cube"], "material": "Test resin"}));
    run(&mut s, "sheet.manage_rules", json!({"name": "Thin", "thickness": "0.8 mm", "active": true}));
    let mut t = replay(&script(&mut s));
    same(&mut s, &mut t);
    assert_eq!(t.doc.units, "in");
    assert_eq!(t.doc.sheet, s.doc.sheet);
    let a = run(&mut s, "inspect.measure", json!({}))["total"].clone();
    let b = run(&mut t, "inspect.measure", json!({}))["total"].clone();
    assert_eq!(a, b, "mass comes from the replayed material");
}

#[test]
fn design_properties_come_back_and_odd_keys_are_listed() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    run(&mut s, "document.metadata.set", json!({"values": {"author": "Jane", "description": "two\nlines", "com.acme.tool": {"id": [1, 2]}}}));
    // Keys another tool wrote that the command would refuse.
    s.doc_mut().metadata.0.insert("revision".into(), json!(3));
    s.doc_mut().metadata.0.insert("org.other".into(), json!("text"));
    let r = run(&mut s, "timeline.export_script", json!({}));
    assert!(r["omitted"].to_string().contains("(org.other, revision)"), "{r}");
    let t = replay(&script(&mut s));
    assert_eq!(
        serde_json::to_value(&t.doc.metadata).unwrap(),
        json!({"author": "Jane", "description": "two\nlines", "com.acme.tool": {"id": [1, 2]}})
    );
}

#[test]
fn what_the_script_leaves_out_is_listed_and_components_are_refused() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "B"}));
    run(&mut s, "appearance.assign", json!({"bodies": ["B"], "color": "#ff0000"}));
    let r = run(&mut s, "timeline.export_script", json!({}));
    assert!(r["omitted"].to_string().contains("appearances"), "{r}");
    run(&mut s, "component.create", json!({"name": "Sub"}));
    let e = s.execute("timeline.export_script", &json!({})).unwrap_err().to_string();
    assert!(e.contains("not supported yet"), "{e}");
}

#[test]
fn a_recorded_feature_that_does_not_fit_is_refused_and_changes_nothing() {
    let mut s = Session::default();
    plate(&mut s);
    let sc = script(&mut s);
    let sketch = sc["commands"].as_array().unwrap().iter().find(|c| c["params"]["feature"]["type"] == "sketch").unwrap()["params"].clone();
    let extrude = sc["commands"].as_array().unwrap().iter().find(|c| c["params"]["feature"]["type"] == "extrude").unwrap()["params"].clone();
    let before = (*s.doc).clone();
    let with = |base: &Value, path: &[&str], v: Value| {
        let mut p = base.clone();
        let mut at = &mut p;
        for k in &path[..path.len() - 1] {
            at = &mut at[*k];
        }
        at[path[path.len() - 1]] = v;
        p
    };
    let refused = [
        ("id taken", with(&extrude, &["feature", "name"], json!("Other"))),
        ("id out of range", with(&with(&extrude, &["feature", "id"], json!(1u64 << 60)), &["feature", "name"], json!("X"))),
        ("parameter taken", with(&with(&sketch, &["feature", "id"], json!(0)), &["feature", "name"], json!("S2"))),
        (
            "bad parameter name",
            with(
                &json!({"feature": {"type": "box", "length": "1", "width": "1", "height": "1"}}),
                &["params"],
                json!([{"name": "1x", "expression": "1"}]),
            ),
        ),
        (
            "unknown unit",
            with(
                &json!({"feature": {"type": "box", "length": "1", "width": "1", "height": "1"}}),
                &["params"],
                json!([{"name": "q", "expression": "1", "unit": "parsec"}]),
            ),
        ),
        ("bad input names", with(&extrude, &["feature", "param_names"], json!([1, 2]))),
        ("unknown type", json!({"feature": {"type": "warp_drive"}})),
        ("huge number", json!({"feature": {"type": "box", "length": "1", "width": "1", "height": "1", "corner": [1e300, 0, 0]}})),
        ("unknown component", json!({"feature": {"type": "box", "length": "1", "width": "1", "height": "1", "component": 9}})),
        ("not an object", json!({"feature": [1, 2, 3]})),
    ];
    for (what, p) in refused {
        assert!(s.execute("timeline.add", &p).is_err(), "{what}: {p}");
        assert_eq!(*s.doc, before, "{what}: nothing changed");
    }
    // A hand-written step needs only the feature's own fields.
    let r = run(&mut s, "timeline.add", json!({"feature": {"type": "box", "length": "5", "width": "5", "height": "5", "corner": [100, 0, 0]}}));
    assert!(r["error"].is_null(), "{r}");
    assert_eq!(r["name"], "Box1");
    assert!(s.doc.feature(r["feature"].as_u64().unwrap()).is_some_and(|f| !f.param_names.is_empty()), "its inputs get parameter names");
}

#[test]
fn the_script_is_written_to_a_file_that_replays() {
    let mut s = Session::default();
    plate(&mut s);
    let dir = std::env::temp_dir().join(format!("solidcraft-script-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("repro.json");
    let r = run(&mut s, "timeline.export_script", json!({"path": path.to_string_lossy()}));
    let text = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let written: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(written, r["script"]);
    let mut t = replay(&written);
    same(&mut s, &mut t);
    assert!(s.execute("timeline.export_script", &json!({"path": 7})).is_err(), "a path is text");
}

#[test]
fn replayed_features_claiming_the_same_body_name_do_not_crash() {
    let mut s = Session::default();
    let boxed = |name: &str, x: f64| json!({"feature": {"type": "box", "name": name, "length": "5", "width": "5", "height": "5", "corner": [x, 0, 0], "body_names": ["Twin"]}});
    run(&mut s, "timeline.add", boxed("A", 0.0));
    let r = s.execute("timeline.add", &boxed("B", 20.0));
    assert!(!matches!(r, Err(crate::EngineError::Internal(..))), "{r:?}");
    let m = run(&mut s, "inspect.measure", json!({}));
    assert!(m["body_count"].as_u64().is_some_and(|n| n >= 1), "{m}");
}

/// #1990: a sketch and the extrude built from it may share a name, as the sample and oracle
/// designs do; the design's script replays into the same design, names included.
#[test]
fn a_design_naming_a_sketch_and_its_extrude_alike_replays() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Block"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [20, 10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": "5", "name": "Block"}));
    let names = |t: &Session| t.doc.features.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s), ["Block", "Block"]);
    let written = script(&mut s);
    assert_eq!(commands(&written), ["timeline.add", "timeline.add"]);
    let mut t = replay(&written);
    same(&mut s, &mut t);
    assert_eq!(names(&t), ["Block", "Block"], "the replay keeps both names");
    assert_eq!(volume(&mut t), 1000.0);
    // The replay's own script is the same script.
    assert_eq!(script(&mut t), written);
}

/// #1990: `timeline.add` keeps a recorded name another feature has, as `solid.extrude` does.
#[test]
fn a_replayed_feature_keeps_a_name_another_feature_has() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": "5", "width": "5", "height": "5", "name": "Twin"}));
    let r = run(
        &mut s,
        "timeline.add",
        json!({"feature": {"type": "box", "name": "Twin", "length": "5", "width": "5", "height": "5", "corner": [20, 0, 0]}}),
    );
    assert_eq!(r["name"], "Twin");
    assert_eq!(s.doc.features.iter().filter(|f| f.name == "Twin").count(), 2);
    // A blank recorded name still gets a fresh one.
    let r = run(
        &mut s,
        "timeline.add",
        json!({"feature": {"type": "box", "name": " ", "length": "5", "width": "5", "height": "5", "corner": [40, 0, 0]}}),
    );
    assert_eq!(r["name"], "Box1");
}
