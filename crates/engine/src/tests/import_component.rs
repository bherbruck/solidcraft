//! An imported file becomes a component named after it (#1386): a STEP part is one component
//! with its body, a STEP assembly nested components that mirror the file with a repeated part's
//! placements as occurrences of one component, held together as one rigid group until it is
//! unlocked; a mesh file one component. Undo takes the whole import away, and designs whose
//! imports sit loose in a component open and rebuild as they did.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;
use solidcraft_kernel::{ExportBody, ExportProduct, StepHeader, box_solid, step_export_products};

use super::run;
use crate::*;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("solidcraft-import-component-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn at(x: f64, y: f64, z: f64) -> [[f64; 4]; 4] {
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, y, z, 1.0]]
}

/// A quarter turn about Z, then moved to (x, y, z).
fn turned(x: f64, y: f64, z: f64) -> [[f64; 4]; 4] {
    [[0.0, 1.0, 0.0, 0.0], [-1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, y, z, 1.0]]
}

/// `Rig`: a base plate, two `Arm` sub-assemblies (each a 20 × 4 × 4 beam and a `Pin`) and a third
/// `Pin` on its own; one arm is turned a quarter turn. Pins: 2 × 2 × 10. Six bodies in the
/// file: four products, one of them a sub-assembly placed twice.
fn rig_file(dir: &std::path::Path) -> String {
    let plate = box_solid(Vec3::ZERO, Vec3::new(100.0, 60.0, 5.0)).unwrap();
    let beam = box_solid(Vec3::ZERO, Vec3::new(20.0, 4.0, 4.0)).unwrap();
    let pin = box_solid(Vec3::ZERO, Vec3::new(2.0, 2.0, 10.0)).unwrap();
    let part = |name: &str, body| ExportProduct {
        name: name.into(),
        bodies: vec![ExportBody { name: name.into(), body, color: None }],
        children: Vec::new(),
    };
    let products = vec![
        ExportProduct {
            name: "Rig".into(),
            bodies: Vec::new(),
            children: vec![
                (1, at(0.0, 0.0, 0.0), "Base-1".into()),
                (2, at(10.0, 10.0, 5.0), "Arm-1".into()),
                (2, turned(60.0, 10.0, 5.0), "Arm-2".into()),
                (4, at(90.0, 50.0, 5.0), "Pin-3".into()),
            ],
        },
        part("Base", &plate),
        ExportProduct {
            name: "Arm".into(),
            bodies: Vec::new(),
            children: vec![(3, at(0.0, 0.0, 0.0), "Beam-1".into()), (4, at(18.0, 1.0, 4.0), "Pin-1".into())],
        },
        part("Beam", &beam),
        part("Pin", &pin),
    ];
    let text = step_export_products(&products, 0, &StepHeader::default()).unwrap();
    let path = dir.join("rig.step");
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().to_string()
}

/// Where the file itself puts its bodies: their bounding boxes, sorted.
fn file_boxes(path: &str) -> Vec<[i64; 6]> {
    let imp = solidcraft_kernel::step_import(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut out: Vec<[i64; 6]> = imp.bodies.iter().map(|b| key(solidcraft_kernel::measure(&b.body).unwrap().bbox)).collect();
    out.sort();
    out
}

fn key(b: solidcraft_geom::Aabb3) -> [i64; 6] {
    let r = |v: f64| (v * 1000.0).round() as i64;
    [r(b.min.x), r(b.min.y), r(b.min.z), r(b.max.x), r(b.max.y), r(b.max.z)]
}

/// Where the design shows its bodies, sorted.
fn shown_boxes(s: &Session) -> Vec<[i64; 6]> {
    let mut out: Vec<[i64; 6]> = s.world_state().bodies.iter().map(|b| key(solidcraft_kernel::measure(&b.body).unwrap().bbox)).collect();
    out.sort();
    out
}

fn components(s: &mut Session) -> Vec<Value> {
    run(s, "component.list", json!({}))["components"].as_array().unwrap().clone()
}

fn component<'a>(list: &'a [Value], name: &str) -> &'a Value {
    list.iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("no component {name}: {list:?}"))
}

fn occurrence_named(s: &Session, name: &str) -> u64 {
    s.doc.occurrences.iter().find(|o| o.name == name).unwrap_or_else(|| panic!("no occurrence {name}")).id
}

/// The world corner of the bodies of one occurrence (by name) nearest -∞.
fn corner_of(s: &Session, occurrence: &str) -> Vec3 {
    let occ = occurrence_named(s, occurrence);
    let path = {
        let mut path = vec![occ];
        let mut parent = s.doc.occurrences.iter().find(|o| o.id == occ).unwrap().parent;
        while parent != 0 {
            let o = s.doc.occurrence_of(parent).unwrap();
            path.insert(0, o.id);
            parent = o.parent;
        }
        path
    };
    let names = s.occurrence_bodies(&path, true).unwrap();
    assert!(!names.is_empty());
    let world = s.world_state();
    let mut lo = Vec3::new(f64::MAX, f64::MAX, f64::MAX);
    for b in world.bodies.iter().filter(|b| names.contains(&b.name)) {
        let bb = solidcraft_kernel::measure(&b.body).unwrap().bbox;
        lo = Vec3::new(lo.x.min(bb.min.x), lo.y.min(bb.min.y), lo.z.min(bb.min.z));
    }
    lo
}

fn close(a: Vec3, b: Vec3) -> bool {
    a.dist(b) < 1e-6
}

/// A box exported as `name.step` in `dir`.
fn part_file(dir: &std::path::Path, name: &str) -> String {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 30, "width": 20, "height": 10}));
    let path = dir.join(format!("{name}.step")).to_string_lossy().to_string();
    run(&mut s, "file.export", json!({"path": path}));
    path
}

#[test]
fn a_step_part_comes_in_as_one_component_named_after_the_file() {
    let dir = temp_dir("part");
    let path = part_file(&dir, "bracket");
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [100, 0, 0]}));
    let r = run(&mut s, "file.insert_step", json!({"path": path}));
    let list = components(&mut s);
    assert_eq!(list.len(), 2, "the design and the import: {list:?}");
    let bracket = component(&list, "bracket");
    assert_eq!((r["component"].clone(), r["component_name"].clone()), (bracket["id"].clone(), json!("bracket")), "{r}");
    assert_eq!(bracket["bodies"].as_array().unwrap().len(), 1, "{bracket}");
    assert_eq!(component(&list, &s.doc.name.clone())["bodies"].as_array().unwrap().len(), 1, "my own box stays where it was");
    let feature = r["feature"].as_u64().unwrap();
    assert_eq!(s.doc.feature(feature).unwrap().component, r["component"].as_u64().unwrap());
    // One part: nothing to hold together.
    assert!(s.doc.assembly.joints.is_empty() && r.get("subcomponents").is_none(), "{r}");
    // The active component is still the one I was working in.
    assert_eq!(s.active_component, 0);

    // Opening the file as a design makes the component too, so the browser shows its name.
    let r = run(&mut s, "doc.open", json!({"path": path}));
    assert_eq!(r["component_name"], "bracket", "{r}");
    assert_eq!(s.doc.components.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["bracket"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_step_assembly_comes_in_as_nested_components_with_its_instances() {
    let dir = temp_dir("assembly");
    let path = rig_file(&dir);
    let mut s = Session::default();
    let r = run(&mut s, "file.insert_step", json!({"path": path}));
    // rig ⊃ Base, Arm (twice; Beam, Pin inside), Pin (once more, directly).
    let list = components(&mut s);
    let names: Vec<&str> = list.iter().filter_map(|c| c["name"].as_str()).collect();
    assert_eq!(names.len(), 6, "{names:?}");
    for n in ["rig", "Base", "Arm", "Beam", "Pin"] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
    let id = |n: &str| component(&list, n)["id"].as_u64().unwrap();
    let parent = |n: &str| component(&list, n)["parent"].as_u64().unwrap();
    assert_eq!(parent("rig"), 0);
    assert_eq!((parent("Base"), parent("Arm")), (id("rig"), id("rig")));
    assert_eq!((parent("Beam"), parent("Pin")), (id("Arm"), id("Arm")));
    assert_eq!(r["component"], id("rig"), "{r}");
    assert_eq!(r["subcomponents"], 4, "{r}");
    // A repeated part is one component placed again: Arm twice, Pin in the arm and on its own.
    let placements = |c: u64| s.doc.occurrences.iter().filter(|o| o.component == c).count();
    assert_eq!((placements(id("Arm")), placements(id("Pin")), placements(id("Beam"))), (2, 2, 1));
    // Each component holds its own part's body; the file's later instances are not built again.
    let own = |n: &str| component(&list, n)["bodies"].as_array().unwrap().len();
    assert_eq!((own("rig"), own("Base"), own("Arm"), own("Beam"), own("Pin")), (0, 1, 0, 1, 1));
    assert_eq!(s.model.state().bodies.len(), 3, "base, beam and pin, once each");
    // And every body shows where the file puts it.
    assert_eq!(shown_boxes(&s), file_boxes(&path));

    // Saved and opened again, it is the same.
    let design = dir.join("rig.solidcraft").to_string_lossy().to_string();
    run(&mut s, "file.save_as", json!({"path": design}));
    let mut again = Session::default();
    run(&mut again, "doc.open", json!({"path": design}));
    assert_eq!(shown_boxes(&again), file_boxes(&path));
    assert_eq!(again.doc.components.len(), 5);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Each occurrence's degrees of freedom, as `joint.list` gives them.
fn dofs(s: &mut Session) -> (std::collections::BTreeMap<u64, u64>, Value) {
    let r = run(s, "joint.list", json!({}));
    let dof = r["dof"].as_array().unwrap().iter().map(|d| (d["occurrence"].as_u64().unwrap(), d["dof"].as_u64().unwrap())).collect();
    (dof, r)
}

#[test]
fn an_imported_assembly_moves_as_one_until_it_is_unlocked() {
    let dir = temp_dir("rigid");
    let path = rig_file(&dir);
    let mut s = Session::default();
    run(&mut s, "file.insert_step", json!({"path": path}));
    let rig = occurrence_named(&s, "rig:1");
    let inside: Vec<u64> = s.doc.occurrences.iter().filter(|o| o.id != rig).map(|o| o.id).collect();
    assert_eq!(inside.len(), 6);
    let pin = corner_of(&s, "Pin:2");
    let arm = corner_of(&s, "Arm:1");

    // The whole import is one rigid body: no part of it moves against the others.
    let (dof, r) = dofs(&mut s);
    assert!(r["conflicts"].as_array().unwrap().is_empty(), "{r}");
    for o in &inside {
        assert_eq!(dof.get(o), dof.get(&rig), "occurrence {o} moves only with the import: {r}");
    }

    // Moving the import's component moves everything in it.
    run(&mut s, "occurrence.move", json!({"occurrence": "rig:1", "translate": [0, 0, 50]}));
    assert!(close(corner_of(&s, "Pin:2"), pin + Vec3::new(0.0, 0.0, 50.0)), "{:?}", corner_of(&s, "Pin:2"));
    assert!(close(corner_of(&s, "Arm:1"), arm + Vec3::new(0.0, 0.0, 50.0)));
    // A part inside doesn't move on its own; the refusal says what to move instead.
    for part in ["Pin:2", "Pin:1"] {
        let e = s.execute("occurrence.move", &json!({"occurrence": part, "translate": [30, 0, 0]})).unwrap_err().to_string();
        assert!(e.contains("move rig"), "{e}");
    }
    assert!(close(corner_of(&s, "Pin:2"), pin + Vec3::new(0.0, 0.0, 50.0)));

    // Next to a grounded part of mine the import is still one rigid body.
    run(&mut s, "component.create", json!({"name": "Mine"}));
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [-50, 0, 0]}));
    run(&mut s, "component.activate", json!({"component": "root"}));
    run(&mut s, "occurrence.ground", json!({"occurrence": "Mine:1", "grounded": true}));
    let (dof, r) = dofs(&mut s);
    for o in &inside {
        assert_eq!(dof.get(o), dof.get(&rig), "{r}");
    }
    assert!(r["conflicts"].as_array().unwrap().is_empty(), "{r}");

    // Unlocked (one undo step), the parts move one by one.
    let undo = s.undo.len();
    let r = run(&mut s, "component.unlock", json!({"component": "rig"}));
    assert_eq!(r["joints"].as_array().unwrap().len(), 6, "{r}");
    assert_eq!(s.undo.len(), undo + 1);
    run(&mut s, "occurrence.move", json!({"occurrence": "Pin:2", "translate": [30, 0, 0]}));
    assert!(close(corner_of(&s, "Pin:2"), pin + Vec3::new(30.0, 0.0, 50.0)), "{:?}", corner_of(&s, "Pin:2"));
    assert!(close(corner_of(&s, "Arm:1"), arm + Vec3::new(0.0, 0.0, 50.0)), "the others stay");
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.assembly.joints.len(), 6, "undo locks it again");
    assert!(s.execute("component.unlock", &json!({"component": "Mine"})).is_err(), "nothing holds my own component");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mesh_file_comes_in_as_one_component() {
    let dir = temp_dir("stl");
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    let stl = dir.join("cube.stl").to_string_lossy().to_string();
    run(&mut s, "file.save_mesh", json!({"path": stl}));
    let mut s = Session::default();
    let r = run(&mut s, "file.insert_mesh", json!({"path": stl}));
    assert_eq!(r["component_name"], "cube", "{r}");
    let list = components(&mut s);
    assert_eq!(list.len(), 2);
    assert_eq!(component(&list, "cube")["bodies"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undo_takes_the_whole_import_away() {
    let dir = temp_dir("undo");
    let path = rig_file(&dir);
    let mut s = Session::default();
    let undo = s.undo.len();
    run(&mut s, "file.insert_step", json!({"path": path}));
    assert_eq!(s.undo.len(), undo + 1, "one step");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.doc.features.is_empty() && s.doc.components.is_empty() && s.doc.occurrences.is_empty(), "nothing left");
    assert!(s.doc.assembly.joints.is_empty() && s.doc.body_components.is_empty() && s.doc.body_offsets.is_empty());
    assert!(s.world_state().bodies.is_empty());
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(shown_boxes(&s), file_boxes(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A design saved before imports made components has its Import loose in the design: it opens
/// as it is (no component is made for it) and builds every body of the file where it was.
#[test]
fn a_design_with_a_loose_import_rebuilds_unchanged() {
    let dir = temp_dir("loose");
    let path = rig_file(&dir);
    let sf = solidcraft_io::step_import_feature(&std::fs::read(&path).unwrap(), &path).unwrap();
    let mut doc = solidcraft_doc::Document::new("Old");
    let id = doc.add_feature(sf.kind, Some(&sf.name)).unwrap();
    doc.feature_mut(id).unwrap().body_names = sf.body_names;
    // As a format 21 file (0.12.0) has it.
    let mut json: Value = serde_json::from_str(&doc.to_json()).unwrap();
    json["format"] = json!("solidcraft/21");
    assert!(json["features"][0]["kind"].get("skip").is_none());
    let file = dir.join("old.solidcraft");
    std::fs::write(&file, json.to_string()).unwrap();
    let mut s = Session::default();
    run(&mut s, "doc.open", json!({"path": file.to_string_lossy()}));
    assert!(s.doc.components.is_empty() && s.doc.occurrences.is_empty(), "not migrated");
    assert_eq!(s.doc.features[0].component, 0);
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, ["Base", "Beam", "Pin", "Beam (2)", "Pin (2)", "Pin (3)"]);
    assert_eq!(shown_boxes(&s), file_boxes(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A design whose `Outer` component holds a plate and an `Inner` component placed twice (the
/// first placement turned and moved), with `Outer` itself turned about X and lifted.
fn nested_file(dir: &std::path::Path) -> String {
    let mut s = Session::default();
    let outer = run(&mut s, "component.create", json!({"name": "Outer"}))["occurrence"].clone();
    run(&mut s, "solid.box", json!({"length": 30, "width": 30, "height": 5, "body_name": "Plate"}));
    let inner = run(&mut s, "component.create", json!({"name": "Inner"}))["occurrence"].clone();
    run(&mut s, "solid.box", json!({"length": 40, "width": 20, "height": 10, "body_name": "Block"}));
    run(&mut s, "component.activate", json!({"component": "Outer"}));
    run(&mut s, "occurrence.move", json!({"occurrence": inner, "axis": [0, 0, 1], "angle": "90 deg", "translate": [100, 0, 0]}));
    run(&mut s, "occurrence.copy", json!({"component": "Inner", "translate": [0, 0, 30]}));
    run(&mut s, "component.activate", json!({"component": "root"}));
    run(&mut s, "occurrence.move", json!({"occurrence": outer, "space": "world", "axis": [1, 0, 0], "angle": "90 deg", "translate": [0, 0, 50]}));
    let path = dir.join("nested.step").to_string_lossy().to_string();
    run(&mut s, "file.export", json!({"path": path}));
    path
}

#[test]
fn an_inserted_assembly_exports_its_nested_parts_where_they_are() {
    let dir = temp_dir("reexport");
    let path = nested_file(&dir);
    let placed = file_boxes(&path);
    assert_eq!(placed.len(), 3, "a plate and two blocks: {placed:?}");
    let mut s = Session::default();
    run(&mut s, "file.insert_step", json!({"path": path}));
    assert_eq!(shown_boxes(&s), placed);
    // The whole design as an assembly: every part is placed once, by its occurrences.
    let again = dir.join("again.step").to_string_lossy().to_string();
    run(&mut s, "file.export", json!({"path": again}));
    assert_eq!(file_boxes(&again), placed);
    // And inserted again, it shows as the first file did.
    let mut back = Session::default();
    run(&mut back, "file.insert_step", json!({"path": again}));
    assert_eq!(shown_boxes(&back), placed);
    let _ = std::fs::remove_dir_all(&dir);
}
