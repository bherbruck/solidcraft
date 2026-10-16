//! STEP files written by other CAD systems: Y up (`up`) and the names of the bodies (`names`)
//! on `file.insert_step` and `doc.open`.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;
use solidcraft_kernel::{ExportBody, ExportProduct, StepHeader, box_solid, step_export_products};

use super::run;
use crate::*;

/// A frame as a Y-up file has it, in a STEP file in a fresh temp directory: a plate 40 wide,
/// 4 high (Y) and 20 deep, with two posts 4 wide and 30 high standing on it. The solids are
/// named after the last feature that made them (`Cut-Extrude3`), and the posts are two
/// occurrences of one part. As written the frame spans x 0..40, y 0..34, z 0..20.
fn frame_file(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("solidcraft-step-options-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let plate = box_solid(Vec3::ZERO, Vec3::new(40.0, 4.0, 20.0)).unwrap();
    let post = box_solid(Vec3::ZERO, Vec3::new(4.0, 30.0, 4.0)).unwrap();
    let at = |x: f64, y: f64, z: f64| [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, y, z, 1.0]];
    let part = |name: &str, label: &str, body| ExportProduct {
        name: name.into(),
        bodies: vec![ExportBody { name: label.into(), body, color: None }],
        children: Vec::new(),
    };
    let products = vec![
        ExportProduct {
            name: "Frame".into(),
            bodies: Vec::new(),
            children: vec![
                (1, at(0.0, 0.0, 0.0), "Plate-1".into()),
                (2, at(10.0, 4.0, 8.0), "Post-1".into()),
                (2, at(26.0, 4.0, 8.0), "Post-2".into()),
            ],
        },
        part("Plate", "Cut-Extrude3", &plate),
        part("Post", "Boss-Extrude1", &post),
    ];
    let text = step_export_products(&products, 0, &StepHeader::default()).unwrap();
    let path = dir.join("frame.step");
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().to_string()
}

/// The lowest and highest corner of everything the design shows.
fn extent(s: &Session) -> ([f64; 3], [f64; 3]) {
    let b = crate::view::bounds(s);
    ([b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z])
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-6)
}

fn body_names(s: &Session) -> Vec<String> {
    s.model.state().bodies.iter().map(|b| b.name.clone()).collect()
}

fn kinds(s: &Session) -> Vec<&'static str> {
    s.doc.features.iter().map(|f| f.kind.type_name()).collect()
}

fn total_volume(s: &mut Session) -> f64 {
    run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap()
}

/// `up: "y"` turns the file's bodies a quarter turn about X, Y becoming Z: the frame's height
/// lies along Z, and what was its depth now runs towards -Y.
#[test]
fn a_y_up_step_file_comes_in_standing_up() {
    let path = frame_file("up");
    let mut s = Session::default();
    run(&mut s, "file.insert_step", json!({"path": path}));
    let (lo, hi) = extent(&s);
    assert!(near(lo, [0.0, 0.0, 0.0]) && near(hi, [40.0, 34.0, 20.0]), "as written: {lo:?} {hi:?}");
    assert_eq!(kinds(&s), ["BaseFeature"]);

    // An explicit z keeps the file's axes, in any case.
    for up in ["z", "Z"] {
        let mut d = Session::default();
        let r = run(&mut d, "file.insert_step", json!({"path": path, "up": up}));
        assert_eq!(kinds(&d), ["BaseFeature"], "{r}");
        assert!(r.get("stand_up").is_none(), "{r}");
        assert!(near(extent(&d).1, [40.0, 34.0, 20.0]));
    }

    // y: (x, y, z) goes to (x, -z, y), so the frame, 34 high in Y, stands 34 high in Z.
    let mut d = Session::default();
    let undo = d.undo.len();
    let r = run(&mut d, "file.insert_step", json!({"path": path, "up": "y"}));
    let (lo, hi) = extent(&d);
    assert!(near(lo, [0.0, -20.0, 0.0]) && near(hi, [40.0, 0.0, 34.0]), "stood up: {lo:?} {hi:?}");
    // The import's component is turned (#1386), so an assembly's instances turn with it.
    assert_eq!(kinds(&d), ["BaseFeature"]);
    assert_eq!(r["stand_up"], r["occurrence"], "{r}");
    assert_eq!(r["created_bodies"].as_array().unwrap().len(), 2, "the plate, and the post placed twice: {r}");
    assert!((total_volume(&mut d) - (40.0 * 4.0 * 20.0 + 2.0 * 4.0 * 30.0 * 4.0)).abs() < 1e-6);
    // The Import and the turn are one undo step.
    assert_eq!(d.undo.len(), undo + 1);
    run(&mut d, "edit.undo", json!({}));
    assert!(d.doc.features.is_empty());

    // Only the file's bodies turn, not the ones already in the design.
    let mut d = Session::default();
    run(&mut d, "solid.box", json!({"length": 5, "width": 6, "height": 7, "corner": [100, 100, 100]}));
    run(&mut d, "file.insert_step", json!({"path": path, "up": "Y"}));
    let (lo, hi) = extent(&d);
    assert!(near(lo, [0.0, -20.0, 0.0]) && near(hi, [105.0, 106.0, 107.0]), "{lo:?} {hi:?}");

    // The turn is the component's placement: the saved design has it.
    let design = std::path::Path::new(&path).with_file_name("frame.solidcraft");
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "file.insert_step", json!({"path": path, "up": "y"}));
    run(&mut s, "file.save_as", json!({"path": design.to_string_lossy()}));
    let mut again = Session::default();
    run(&mut again, "doc.open", json!({"path": design.to_string_lossy()}));
    assert!(near(extent(&again).1, [40.0, 0.0, 34.0]));
}

/// `doc.open` takes the same options and starts a new design that has the turn in it.
#[test]
fn opening_a_y_up_step_file_can_stand_it_up() {
    let path = frame_file("open");
    let mut s = Session::default();
    let r = run(&mut s, "doc.open", json!({"path": path, "up": "y"}));
    assert_eq!(r["features"], 1, "{r}");
    assert!(near(extent(&s).1, [40.0, 0.0, 34.0]));
    assert!(s.undo.is_empty(), "a freshly opened design has no history");
    run(&mut s, "doc.open", json!({"path": path}));
    assert_eq!(kinds(&s), ["BaseFeature"]);

    // The options belong to STEP files: a design file does not take them.
    let design = std::path::Path::new(&path).with_file_name("kept.solidcraft");
    run(&mut s, "file.save_as", json!({"path": design.to_string_lossy()}));
    let before = s.doc.clone();
    for p in [json!({"up": "y"}), json!({"names": "product"})] {
        let mut params = p.clone();
        params["path"] = json!(design.to_string_lossy());
        assert!(s.execute("doc.open", &params).is_err(), "{params}");
    }
    assert!(std::sync::Arc::ptr_eq(&s.doc, &before));
}

/// A value that is not one of the choices is refused, and the design stays as it was.
#[test]
fn wrong_up_and_names_are_refused() {
    let path = frame_file("refused");
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5}));
    let before = s.doc.clone();
    for p in [
        json!({"up": "x"}),
        json!({"up": "-y"}),
        json!({"up": 5}),
        json!({"up": ["y"]}),
        json!({"names": "label"}),
        json!({"names": 7}),
        json!({"up": "y", "names": "nope"}),
    ] {
        let mut params = p.clone();
        params["path"] = json!(path);
        for command in ["file.insert_step", "doc.open"] {
            let e = s.execute(command, &params).unwrap_err().to_string();
            assert!(e.contains("`up`") || e.contains("`names`"), "{command} {params}: {e}");
            assert!(std::sync::Arc::ptr_eq(&s.doc, &before), "{command} {params} changed the design");
        }
    }
    // Null means not given.
    run(&mut s, "file.insert_step", json!({"path": path, "up": null, "names": null}));
    assert_eq!(kinds(&s), ["BoxFeature", "BaseFeature"]);
}

/// Explicit naming choices select the solid, part or occurrence; automatic naming keeps
/// meaningful solid names.
#[test]
fn the_bodies_are_named_after_the_solid_the_product_or_the_instance() {
    let path = frame_file("names");
    let named = |params: Value| {
        let mut s = Session::default();
        let r = run(&mut s, "file.insert_step", params);
        assert_eq!(r["bodies"], json!(body_names(&s)), "{r}");
        assert_eq!(r["name"], "Frame", "the timeline entry is the root product's");
        body_names(&s)
    };
    // The post is placed twice: one component, two occurrences, its body built once (#1386).
    let solids = ["Cut-Extrude3", "Boss-Extrude1"];
    assert_eq!(named(json!({"path": path})), solids);
    assert_eq!(named(json!({"path": path, "names": "auto"})), solids);
    assert_eq!(named(json!({"path": path, "names": "solid"})), solids);
    assert_eq!(named(json!({"path": path, "names": "product"})), ["Plate", "Post"]);
    assert_eq!(named(json!({"path": path, "names": "Instance"})), ["Plate-1", "Post-1"]);

    // The names are stored with the import: a saved design keeps them, and `up` does not change them.
    let mut s = Session::default();
    run(&mut s, "doc.open", json!({"path": path, "names": "instance", "up": "y"}));
    assert_eq!(body_names(&s), ["Plate-1", "Post-1"]);
    let design = std::path::Path::new(&path).with_file_name("frame-named.solidcraft");
    run(&mut s, "file.save_as", json!({"path": design.to_string_lossy()}));
    let mut again = Session::default();
    run(&mut again, "doc.open", json!({"path": design.to_string_lossy()}));
    assert_eq!(body_names(&again), ["Plate-1", "Post-1"]);
    // A feature finds them by those names.
    run(&mut again, "solid.move", json!({"bodies": ["Post-1"], "translate": [0, 0, 5]}));
    assert_eq!(body_names(&again), ["Plate-1", "Post-1"]);
}

/// Generated body labels become part names, and the saved assembly retains its components
/// and repeated placements without needing the source file.
#[test]
fn automatic_part_names_and_assembly_survive_reopening() {
    let path = frame_file("automatic");
    let text = std::fs::read_to_string(&path).unwrap().replace("'Cut-Extrude3'", "'Body1'").replace("'Boss-Extrude1'", "'Body 2 (3)'");
    std::fs::write(&path, text).unwrap();
    let mut s = Session::default();
    let r = run(&mut s, "file.insert_step", json!({"path": path}));
    assert_eq!(body_names(&s), ["Plate", "Post"]);
    let top = r["component"].as_u64().unwrap();
    let parts: Vec<_> = s.doc.components.iter().filter(|c| c.parent == top).collect();
    assert_eq!(parts.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Plate", "Post"]);
    assert_eq!(s.doc.occurrences.iter().filter(|o| o.component == parts[1].id && o.parent == top).count(), 2);
    let design = std::path::Path::new(&path).with_file_name("automatic.solidcraft");
    run(&mut s, "file.save_as", json!({"path": design.to_string_lossy()}));
    std::fs::remove_file(&path).unwrap();
    let mut again = Session::default();
    run(&mut again, "doc.open", json!({"path": design.to_string_lossy()}));
    assert_eq!(body_names(&again), ["Plate", "Post"]);
    assert_eq!(again.doc.components, s.doc.components);
    assert_eq!(again.doc.occurrences, s.doc.occurrences);
    assert!(near(extent(&again).0, [0.0, 0.0, 0.0]) && near(extent(&again).1, [40.0, 34.0, 20.0]));
    assert!((total_volume(&mut again) - (40.0 * 4.0 * 20.0 + 2.0 * 4.0 * 30.0 * 4.0)).abs() < 1e-6);
}
