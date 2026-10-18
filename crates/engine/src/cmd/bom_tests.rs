use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// A folder of its own for one test's files.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("solidcraft-bom-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A plate placed three times, and a hinge subassembly (no bodies of its own) placed twice
/// that holds one pin: the plate has 3 occurrences, the pin 2 (one in each hinge).
fn assembly() -> Session {
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Plate"}));
    run(&mut s, "solid.box", json!({"length": 40, "width": 20, "height": 3}));
    run(&mut s, "component.activate", json!({"component": 0}));
    run(&mut s, "component.create", json!({"name": "Hinge", "activate": false}));
    run(&mut s, "component.create", json!({"name": "Pin", "parent": "Hinge"}));
    run(&mut s, "solid.cylinder", json!({"diameter": 4, "height": 30}));
    run(&mut s, "component.activate", json!({"component": 0}));
    for x in [50, 100] {
        run(&mut s, "occurrence.copy", json!({"component": "Plate", "translate": [x, 0, 0]}));
    }
    run(&mut s, "occurrence.copy", json!({"component": "Hinge", "translate": [0, 50, 0]}));
    run(&mut s, "component.metadata.set", json!({"component": "Plate", "part_number": "PL-100", "description": "Plate, 3 mm", "revision": "B"}));
    run(
        &mut s,
        "component.metadata.set",
        json!({"component": "Pin", "part_number": "PN-040", "description": "Pin 4 x 30", "properties": {"finish": "zinc"}}),
    );
    s
}

fn summary(r: &Value) -> Vec<(String, String, u64)> {
    r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["part_number"].as_str().unwrap().to_string(), r["definition"].as_str().unwrap().to_string(), r["count"].as_u64().unwrap()))
        .collect()
}

#[test]
fn flat_counts_parts_through_subassemblies_and_top_counts_children() {
    let mut s = assembly();
    let undo = s.undo.len();
    let flat = run(&mut s, "assembly.bom", json!({}));
    assert_eq!(summary(&flat), [("PL-100".into(), "Plate".into(), 3), ("PN-040".into(), "Pin".into(), 2)], "{flat}");
    assert_eq!(flat["total_occurrences"], 5);
    assert_eq!(flat["rows"][0]["item"], 1);
    assert_eq!(flat["rows"][0]["description"], "Plate, 3 mm");
    assert_eq!(flat["rows"][0]["revision"], "B");
    assert_eq!(flat["rows"][1]["properties"], json!({"finish": "zinc"}));

    let top = run(&mut s, "assembly.bom", json!({"levels": "top"}));
    assert_eq!(summary(&top), [("PL-100".into(), "Plate".into(), 3), (String::new(), "Hinge".into(), 2)], "{top}");
    assert_eq!(top["total_occurrences"], 5);

    // One subassembly's own bill of materials.
    let hinge = run(&mut s, "assembly.bom", json!({"component": "Hinge"}));
    assert_eq!(summary(&hinge), [("PN-040".into(), "Pin".into(), 1)]);
    assert_eq!(s.undo.len(), undo, "the bill of materials only reads");
}

#[test]
fn equal_part_numbers_share_a_row_unless_grouped_by_definition() {
    let mut s = assembly();
    // An independent copy keeps the part properties, so it is the same part.
    run(&mut s, "component.paste_new", json!({"component": "Plate", "translate": [0, -60, 0]}));
    let r = run(&mut s, "assembly.bom", json!({}));
    let plate = &r["rows"][0];
    assert_eq!((plate["part_number"].as_str(), plate["count"].as_u64()), (Some("PL-100"), Some(4)), "{r}");
    assert_eq!(plate["components"].as_array().map(Vec::len), Some(2));
    let by_def = run(&mut s, "assembly.bom", json!({"group_by": ["definition"]}));
    let counts: Vec<u64> =
        by_def["rows"].as_array().unwrap().iter().filter(|r| r["part_number"] == "PL-100").map(|r| r["count"].as_u64().unwrap()).collect();
    assert_eq!(counts, [3, 1], "{by_def}");
    // Without a part number every definition is its own row, after the numbered ones.
    run(&mut s, "component.create", json!({"name": "Spacer"}));
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5}));
    run(&mut s, "component.activate", json!({"component": 0}));
    let r = run(&mut s, "assembly.bom", json!({}));
    let last = r["rows"].as_array().unwrap().last().cloned().unwrap();
    assert_eq!((last["part_number"].as_str(), last["definition"].as_str()), (Some(""), Some("Spacer")));
}

#[test]
fn part_properties_survive_save_and_open_and_undo_in_one_step() {
    let dir = scratch("roundtrip");
    let mut s = assembly();
    let undo = s.undo.len();
    let r = run(&mut s, "component.metadata.set", json!({"component": "Pin", "revision": "C", "properties": {"finish": null, "material": "1.4301"}}));
    assert_eq!(r["changed"], json!(["revision", "properties.finish", "properties.material"]));
    assert_eq!(s.undo.len(), undo + 1, "one undo step");
    // Setting the same values again changes nothing.
    let r = run(&mut s, "component.metadata.set", json!({"component": "Pin", "revision": "C"}));
    assert_eq!(r["changed"], json!([]));
    assert_eq!(s.undo.len(), undo + 1);
    for (file, format) in [("bom.json", "json"), ("bom.solidcraft", "container")] {
        let path = dir.join(file);
        run(&mut s, "file.save_as", json!({"path": path, "format": format}));
        let mut back = Session::default();
        run(&mut back, "doc.open", json!({"path": path}));
        let pin = run(&mut back, "component.metadata", json!({"component": "Pin"}));
        assert_eq!((pin["part_number"].as_str(), pin["revision"].as_str()), (Some("PN-040"), Some("C")), "{format}: {pin}");
        assert_eq!(pin["properties"], json!({"material": "1.4301"}), "{format}");
        assert_eq!(summary(&run(&mut back, "assembly.bom", json!({}))), summary(&run(&mut s, "assembly.bom", json!({}))), "{format}");
    }
    run(&mut s, "edit.undo", json!({}));
    let pin = run(&mut s, "component.metadata", json!({"component": "Pin"}));
    assert_eq!((pin["revision"].as_str(), &pin["properties"]), (None, &json!({"finish": "zinc"})));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn csv_quotes_every_field_and_json_writes_the_rows() {
    let dir = scratch("export");
    let mut s = assembly();
    run(&mut s, "component.metadata.set", json!({"component": "Pin", "description": "Pin \"short\", 4 mm"}));
    let csv = dir.join("bom.csv");
    let r = run(&mut s, "assembly.bom", json!({"path": csv}));
    assert_eq!(r["format"], "csv");
    let text = std::fs::read_to_string(&csv).unwrap();
    assert_eq!(
        text,
        "\"item\",\"part_number\",\"description\",\"revision\",\"count\",\"definition\"\n\
         \"1\",\"PL-100\",\"Plate, 3 mm\",\"B\",\"3\",\"Plate\"\n\
         \"2\",\"PN-040\",\"Pin \"\"short\"\", 4 mm\",\"\",\"2\",\"Pin\"\n"
    );
    let json_path = dir.join("bom.json");
    run(&mut s, "assembly.bom", json!({"path": json_path, "levels": "top"}));
    let back: Value = serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    assert_eq!(back["total_occurrences"], 5);
    assert_eq!(back["rows"][1]["definition"], "Hinge");
    assert!(s.execute("assembly.bom", &json!({"path": dir.join("bom.txt")})).is_err(), "unknown extension");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn standard_parts_carry_a_part_number_that_follows_a_resize() {
    let mut s = Session::default();
    let a = run(&mut s, "parts.insert", json!({"family": "socket_head_cap_screw", "size": "M6", "length": 20}));
    run(&mut s, "parts.insert", json!({"family": "socket_head_cap_screw", "size": "M6", "length": 20, "point": [30, 0, 0]}));
    run(&mut s, "parts.insert", json!({"family": "bearing", "size": "608", "point": [60, 0, 0]}));
    let r = run(&mut s, "assembly.bom", json!({}));
    let rows: Vec<(String, String, u64)> = r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["part_number"].as_str().unwrap().into(), r["description"].as_str().unwrap().into(), r["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        rows,
        [("608".into(), "Deep groove ball bearing (envelope) 608".into(), 1), ("ISO 4762 M6x20".into(), "Socket head cap screw M6x20".into(), 2)]
    );
    let screws = r["rows"].as_array().unwrap().iter().find(|r| r["count"] == 2).cloned().unwrap();
    assert_eq!(screws["definition"], "SHCS M6x20", "equal names are named once");
    assert_eq!(screws["components"].as_array().map(Vec::len), Some(2));
    // A resize renumbers the part; a description someone wrote stays theirs.
    let comp = a["component"].as_u64().unwrap();
    run(&mut s, "component.metadata.set", json!({"component": comp, "description": "Cap screw, A2"}));
    run(&mut s, "parts.resize", json!({"component": comp, "size": "M8", "length": 25}));
    let m = run(&mut s, "component.metadata", json!({"component": comp}));
    assert_eq!((m["part_number"].as_str(), m["description"].as_str()), (Some("ISO 4762 M8x25"), Some("Cap screw, A2")));
}

#[test]
fn hostile_part_properties_are_refused_without_a_change() {
    let mut s = assembly();
    let undo = s.undo.len();
    for p in [
        json!({"component": "Pin"}),
        json!({"component": 0, "part_number": "X"}),
        json!({"component": "Nope", "part_number": "X"}),
        json!({"component": "Pin", "part_number": 7}),
        json!({"component": "Pin", "part_number": "ok", "description": "bell\u{7}"}),
        json!({"component": "Pin", "properties": {"": "x"}}),
        json!({"component": "Pin", "properties": {"finish": ["x"]}}),
        json!({"component": "Pin", "properties": {"finish": "x".repeat(5000)}}),
        json!({"component": "Pin", "values": {"bogus": "x"}}),
        json!({"component": "Pin", "values": "x"}),
    ] {
        assert!(s.execute("component.metadata.set", &p).is_err(), "{p}");
    }
    assert_eq!(s.undo.len(), undo);
    let pin = run(&mut s, "component.metadata", json!({"component": "Pin"}));
    assert_eq!(pin["part_number"], "PN-040", "all or nothing");
    for p in [json!({"levels": "deep"}), json!({"group_by": ["colour"]}), json!({"group_by": 3}), json!({"component": "Nope"})] {
        assert!(s.execute("assembly.bom", &p).is_err(), "{p}");
    }
    // The root reads as the design's own properties.
    run(&mut s, "document.metadata.set", json!({"key": "part_number", "value": "ASM-1"}));
    assert_eq!(run(&mut s, "component.metadata", json!({"component": 0}))["part_number"], "ASM-1");
}

/// Two hex bolts, each with a washer and a nut placed inside it, and a third bolt alone.
fn bolts() -> Session {
    let mut s = Session::default();
    for x in [0, 30] {
        let bolt = run(&mut s, "parts.insert", json!({"family": "hex_bolt", "size": "M6", "length": 30, "point": [x, 0, 0], "direction": [0, 0, 1]}));
        run(&mut s, "component.activate", json!({"component": bolt["component"]}));
        run(&mut s, "parts.insert", json!({"family": "washer", "size": "M6", "point": [x, 0, -2], "direction": [0, 0, -1]}));
        run(&mut s, "parts.insert", json!({"family": "hex_nut", "size": "M6", "point": [x, 0, -4], "direction": [0, 0, -1]}));
        run(&mut s, "component.activate", json!({"component": 0}));
    }
    run(&mut s, "parts.insert", json!({"family": "hex_bolt", "size": "M6", "length": 30, "point": [60, 0, 0], "direction": [0, 0, 1]}));
    s
}

#[test]
fn group_children_keeps_a_bolts_washer_and_nut_in_its_row() {
    let mut s = bolts();
    let plain = run(&mut s, "assembly.bom", json!({}));
    let numbers: Vec<(String, u64)> = summary(&plain).into_iter().map(|(pn, _, n)| (pn, n)).collect();
    assert_eq!(numbers, [("ISO 4017 M6x30".into(), 3), ("ISO 4032 M6".into(), 2), ("ISO 7089 M6".into(), 2)], "{plain}");

    let r = run(&mut s, "assembly.bom", json!({"group_children": true}));
    let rows = r["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "the bolts with a nut and the bolt alone: {r}");
    let kit = &rows[0];
    assert_eq!((kit["part_number"].as_str(), kit["count"].as_u64()), (Some("ISO 4017 M6x30"), Some(2)), "{r}");
    let children: Vec<(&str, &str, u64, u64)> = kit["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["item"].as_str().unwrap(), c["part_number"].as_str().unwrap(), c["each"].as_u64().unwrap(), c["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(children, [("1.1", "ISO 4032 M6", 1, 2), ("1.2", "ISO 7089 M6", 1, 2)]);
    assert_eq!((rows[1]["count"].as_u64(), rows[1].get("children")), (Some(1), None));
    assert_eq!(r["total_occurrences"], plain["total_occurrences"], "the same parts, folded");

    let dir = scratch("kit");
    let path = dir.join("bom.csv");
    run(&mut s, "assembly.bom", json!({"group_children": true, "path": path.to_str().unwrap()}));
    let text = std::fs::read_to_string(&path).unwrap();
    let items: Vec<&str> = text.lines().skip(1).map(|l| l.split(',').next().unwrap()).collect();
    assert_eq!(items, ["\"1\"", "\"1.1\"", "\"1.2\"", "\"2\""], "{text}");
    assert!(s.execute("assembly.bom", &json!({"group_children": "yes"})).is_err());
}

#[test]
fn changing_part_properties_does_not_evaluate_the_model_again() {
    let mut s = bolts();
    s.evaluations = 0;
    run(&mut s, "component.metadata.set", json!({"component": 1, "description": "Bolt", "properties": {"finish": "zinc"}}));
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(s.evaluations, 0, "part properties change no geometry");
    assert_eq!(run(&mut s, "component.metadata", json!({"component": 1}))["description"], "Bolt");
    // A geometric edit after it finds every other feature's earlier result.
    let first = run(&mut s, "assembly.bom", json!({}))["rows"][1]["components"][0].clone();
    run(&mut s, "parts.resize", json!({"component": first, "size": "M8"}));
    run(&mut s, "parts.resize", json!({"component": first, "size": "M6"}));
    // Anything else still evaluates.
    run(&mut s, "occurrence.copy", json!({"component": 1, "translate": [0, 40, 0]}));
    assert!(s.evaluations > 0);
}

#[test]
fn body_owners_match_each_bodys_component() {
    for s in [assembly(), bolts()] {
        let st = s.model.state();
        let each: std::collections::HashSet<u64> = st.bodies.iter().map(|b| s.doc.model_body_component(b)).collect();
        assert_eq!(s.doc.body_owners(&st), each);
        assert!(each.len() > 1);
    }
}
