//! Design properties (#1015): `document.metadata` reads the design's `metadata` object, and
//! `document.metadata.set` and `document.metadata.remove` change it, each in one undo step. The
//! keys SolidCraft reads, and the checks a change gets, are `solidcraft_doc::metadata`'s; keys
//! other tools wrote are kept as they are. The UI's Design Properties dialog, scripts and MCP
//! agents all go through these commands.

use serde_json::{Map, Value, json};
use solidcraft_doc::metadata::{MAX_KEYS, WELL_KNOWN};

use super::CommandSpec;
use crate::params::{bad, str_};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("document.metadata", "Design Properties", read).icon("settings").noundo().doc_only().params(
        "→ metadata: the design's metadata object as saved (other tools' keys too); properties: project, part_number, revision, \
         author, organization, description and license, each its text or null (absent, or not text)",
    ),
    CommandSpec::new("document.metadata.set", "Change Design Properties", set).icon("settings").doc_only().params(
        "key and value, or values: {key: value, …} (all or nothing, one undo step). SolidCraft's keys: project, part_number, \
         revision, author, organization, description, license (text; \"\" or null removes); another tool's data goes under its \
         reverse-DNS key such as com.example.tool (an object; null removes) → metadata, changed: [keys]",
    ),
    CommandSpec::new("document.metadata.remove", "Remove Design Property", remove)
        .icon("delete")
        .doc_only()
        .params("key, or keys: [keys] (any key, also one another tool wrote) → metadata, removed: [keys that were there]"),
];

fn read(s: &mut Session, _: &Value) -> Result<Value> {
    let m = &s.doc.metadata;
    let properties: Map<String, Value> = WELL_KNOWN.iter().map(|k| (k.to_string(), m.text(k).map_or(Value::Null, |t| json!(t)))).collect();
    Ok(json!({"metadata": m, "properties": properties}))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "document.metadata.set";
    let entries: Vec<(&str, &Value)> = match (p.get("values"), p.get("key")) {
        (Some(Value::Object(values)), None) if values.len() <= MAX_KEYS => values.iter().map(|(k, v)| (k.as_str(), v)).collect(),
        (Some(Value::Object(_)), None) => return Err(bad(cmd, format!("`values` can hold at most {MAX_KEYS} keys"))),
        (None, Some(_)) => {
            let key = str_(p, "key").ok_or_else(|| bad(cmd, "`key` must be text"))?;
            let value = p.get("value").ok_or_else(|| bad(cmd, "`value` is required (null removes the key)"))?;
            vec![(key, value)]
        }
        _ => return Err(bad(cmd, "give `key` and `value`, or `values`: {key: value, …}")),
    };
    let mut next = s.doc.metadata.clone();
    let mut changed = Vec::new();
    for (key, value) in entries {
        if next.set(key, value.clone()).map_err(|e| bad(cmd, e.to_string()))? {
            changed.push(key.to_string());
        }
    }
    // Only a real change touches the design (an undo step, unsaved changes).
    if next != s.doc.metadata {
        s.doc_mut().metadata = next;
    }
    Ok(json!({"metadata": s.doc.metadata, "changed": changed}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "document.metadata.remove";
    let keys: Vec<&str> = match (p.get("keys"), p.get("key")) {
        (Some(Value::Array(keys)), None) if keys.len() <= MAX_KEYS => {
            keys.iter().map(Value::as_str).collect::<Option<_>>().ok_or_else(|| bad(cmd, "`keys` must be a list of keys"))?
        }
        (None, Some(_)) => vec![str_(p, "key").ok_or_else(|| bad(cmd, "`key` must be text"))?],
        _ => return Err(bad(cmd, format!("give `key`, or `keys`: a list of at most {MAX_KEYS} keys"))),
    };
    let mut removed: Vec<&str> = Vec::new();
    for k in keys {
        if s.doc.metadata.get(k).is_some() && !removed.contains(&k) {
            removed.push(k);
        }
    }
    if !removed.is_empty() {
        let m = &mut s.doc_mut().metadata;
        for k in &removed {
            m.remove(k);
        }
    }
    Ok(json!({"metadata": s.doc.metadata, "removed": removed}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Session, id: &str, p: Value) -> Value {
        match s.execute(id, &p) {
            Ok(v) => v,
            Err(e) => panic!("{id} {p}: {e}"),
        }
    }

    /// A folder of its own for one test's files.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("solidcraft-metadata-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn properties_set_read_and_remove_in_one_undo_step_each() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        s.mark_saved();
        let undo = s.undo.len();
        let r = run(
            &mut s,
            "document.metadata.set",
            json!({"values": {"author": "Jane Doe", "part_number": "ACME-1042", "com.acme.shop": {"shop_id": 88123}}}),
        );
        assert_eq!(r["changed"], json!(["author", "com.acme.shop", "part_number"]));
        assert_eq!(s.undo.len(), undo + 1, "several keys, one undo step");
        assert!(s.is_dirty());
        let r = run(&mut s, "document.metadata", json!({}));
        assert_eq!(r["properties"]["author"], "Jane Doe");
        assert_eq!(r["properties"]["project"], Value::Null);
        assert_eq!(r["metadata"]["com.acme.shop"]["shop_id"], 88123);

        // Setting what is already there changes nothing: no undo step.
        let r = run(&mut s, "document.metadata.set", json!({"key": "author", "value": "Jane Doe"}));
        assert_eq!(r["changed"], json!([]));
        assert_eq!(s.undo.len(), undo + 1);

        let r = run(&mut s, "document.metadata.remove", json!({"keys": ["author", "author", "not.there"]}));
        assert_eq!(r["removed"], json!(["author"]));
        assert_eq!(s.doc.metadata.author(), None);
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.metadata.author(), Some("Jane Doe"));
        run(&mut s, "edit.undo", json!({}));
        assert!(s.doc.metadata.is_empty());
        assert!(!s.is_dirty(), "undone back to the saved design");
        run(&mut s, "edit.redo", json!({}));
        assert_eq!(s.doc.metadata.part_number(), Some("ACME-1042"));
    }

    #[test]
    fn a_refused_change_leaves_the_design_as_it_was() {
        let mut s = Session::default();
        run(&mut s, "document.metadata.set", json!({"key": "author", "value": "Jane"}));
        let undo = s.undo.len();
        for p in [
            json!({"values": {"project": "P", "author": 42}}),
            json!({"values": {"project": "P", "colour": "red"}}),
            json!({"values": {"project": "P", "com.acme": "not an object"}}),
            json!({"key": "author"}),
            json!({"key": 7, "value": "x"}),
            json!({"key": "author", "value": "x", "values": {}}),
            json!({"values": [1, 2]}),
            json!({}),
        ] {
            assert!(s.execute("document.metadata.set", &p).is_err(), "{p}");
        }
        for p in [json!({}), json!({"keys": "author"}), json!({"keys": [1]}), json!({"key": ["author"]})] {
            assert!(s.execute("document.metadata.remove", &p).is_err(), "{p}");
        }
        assert_eq!(s.undo.len(), undo);
        assert_eq!(s.doc.metadata.project(), None, "all or nothing");
        assert_eq!(s.doc.metadata.author(), Some("Jane"));
    }

    #[test]
    fn other_tools_keys_and_odd_values_survive_open_edit_and_save() {
        let dir = scratch("roundtrip");
        // A design another tool added metadata to by hand: a namespaced object with every kind of
        // JSON value, a well-known key of the wrong type, a key a newer SolidCraft might read, and
        // numbers no feature input could hold.
        let foreign = json!({
            "author": 42,
            "revision": "B",
            "com.acme.description": {"shop_id": 88123, "nested": {"list": [1, 2.5, null, true, "x", {"deep": []}]}, "big": 1.5e300},
            "org.printfarm": {"slicer_profile": "PETG-0.2"},
            "future_key": ["kept"],
        });
        let mut design: Value = serde_json::from_str(&solidcraft_doc::Document::new("Foreign").to_json()).unwrap();
        design["metadata"] = foreign.clone();
        let plain = dir.join("foreign.json");
        std::fs::write(&plain, serde_json::to_string_pretty(&design).unwrap()).unwrap();

        let mut s = Session::default();
        run(&mut s, "doc.open", json!({"path": plain}));
        assert_eq!(serde_json::to_value(&s.doc.metadata).unwrap(), foreign, "opened as it was");
        assert_eq!(s.doc.metadata.author(), None, "a number is not an author");
        let r = run(&mut s, "document.metadata", json!({}));
        assert_eq!(r["properties"]["author"], Value::Null);
        assert_eq!(r["properties"]["revision"], "B");

        run(&mut s, "document.metadata.set", json!({"key": "project", "value": "ACME enclosures"}));
        let mut expected = foreign.clone();
        expected["project"] = json!("ACME enclosures");
        for (file, format) in [("edited.json", "json"), ("edited.solidcraft", "container")] {
            let path = dir.join(file);
            run(&mut s, "file.save_as", json!({"path": path, "format": format}));
            let mut reopened = Session::default();
            run(&mut reopened, "doc.open", json!({"path": path}));
            assert_eq!(serde_json::to_value(&reopened.doc.metadata).unwrap(), expected, "{format}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_design_without_properties_saves_as_before() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        let before = solidcraft_io::write_design(&s.doc);
        run(&mut s, "document.metadata.set", json!({"key": "author", "value": "Jane"}));
        run(&mut s, "document.metadata.remove", json!({"key": "author"}));
        assert_eq!(solidcraft_io::write_design(&s.doc), before, "byte for byte");
        assert!(!String::from_utf8_lossy(&before).contains("\"metadata\""));
    }

    #[test]
    fn exports_carry_the_properties_and_never_other_tools_keys() {
        // `file.export` hands the design's properties to the writers; the mapping itself is
        // tested in solidcraft-io.
        let dir = scratch("export");
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        run(
            &mut s,
            "document.metadata.set",
            json!({"values": {
                "project": "ACME enclosures", "author": "Jane Doe", "organization": "ACME", "license": "CC-BY-4.0",
                "description": "Vented case\nfor a Pi", "part_number": "ACME-1042", "com.acme.secret": {"token": "SECRET-VALUE"},
            }}),
        );
        let text = |s: &mut Session, name: &str| {
            let path = dir.join(name);
            run(s, "file.export", json!({"path": path}));
            std::fs::read(&path).unwrap()
        };
        let step = String::from_utf8(text(&mut s, "part.step")).unwrap();
        assert!(step.contains("FILE_DESCRIPTION(('Vented case for a Pi'),'2;1');"), "{}", &step[..400]);
        assert!(step.contains("('Jane Doe'),('ACME')"), "{}", &step[..400]);
        let iges = String::from_utf8(text(&mut s, "part.igs")).unwrap();
        assert!(iges.contains("8HJane Doe") && iges.contains("4HACME"), "{}", &iges[..600]);
        // 3MF model metadata is checked where it is written (solidcraft-io); here: it exports.
        assert!(!text(&mut s, "part.3mf").is_empty());
        for out in [step.as_bytes(), iges.as_bytes()] {
            let found = |needle: &str| out.windows(needle.len()).any(|w| w == needle.as_bytes());
            assert!(!found("SECRET-VALUE") && !found("com.acme"), "another tool's data is never exported");
        }
        // An assembly's STEP gets the same header.
        run(&mut s, "component.create", json!({}));
        assert!(!s.doc.occurrences.is_empty());
        let assembly = String::from_utf8(text(&mut s, "assembly.step")).unwrap();
        assert!(assembly.contains("('Jane Doe'),('ACME')"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspect_lists_the_metadata() {
        let mut s = Session::default();
        assert_eq!(run(&mut s, "document.inspect", json!({}))["metadata"], json!({}));
        run(&mut s, "document.metadata.set", json!({"key": "revision", "value": "C"}));
        assert_eq!(run(&mut s, "document.inspect", json!({}))["metadata"], json!({"revision": "C"}));
    }
}
