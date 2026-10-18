use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// A part library written to a fresh folder (removed when dropped): `library.json` in it.
struct Folder(std::path::PathBuf);

impl Folder {
    fn new(library: &Value) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = solidcraft_io::vfs::temp_dir().join(format!("solidcraft-parts-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join("library.json"), library.to_string()).unwrap_or_else(|e| panic!("{e}"));
        Folder(dir)
    }
    fn dir(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
    fn file(&self) -> String {
        self.0.join("library.json").to_string_lossy().into_owned()
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Two sizes of a generic dome-head blind rivet, each length with the grip it holds.
fn rivets() -> Value {
    json!({
        "format": "solidcraft-parts/1",
        "id": "example.rivets",
        "name": "Example blind rivets",
        "provenance": {"license": "CC0-1.0"},
        "families": [{
            "kind": "fastener_family",
            "name": "rivet_dome",
            "label": "Rivet",
            "shape": "blind_rivet",
            "sizes": [
                {"size": "4.0", "d": 4.0, "head": {"d": 8.0, "h": 1.3, "style": "dome"},
                 "lengths": [{"length": 6, "grip": [1.0, 3.0]}, {"length": 8, "grip": [3.0, 5.0]}, {"length": 10, "grip": [5.0, 6.5]}]},
                {"size": "4.8", "d": 4.8, "head": {"d": 9.5, "h": 1.6, "style": "dome"},
                 "lengths": [{"length": 8, "grip": [1.5, 4.0]}, {"length": 10, "grip": [4.0, 6.0]},
                             {"length": 12, "grip": [6.0, 8.0], "article": "EX-48-12"}, {"length": 14, "grip": [8.0, 10.0]}]}
            ]
        }]
    })
}

fn bbox(s: &Session, prefix: &str) -> (solidcraft_geom::Vec3, solidcraft_geom::Vec3) {
    let st = s.world_state();
    let comp = s.doc.components.iter().find(|c| c.name.starts_with(prefix)).map(|c| c.id).unwrap_or(0);
    let (mut lo, mut hi) = (solidcraft_geom::Vec3::new(f64::MAX, f64::MAX, f64::MAX), solidcraft_geom::Vec3::new(f64::MIN, f64::MIN, f64::MIN));
    for b in st.bodies.iter().filter(|b| s.doc.model_body_component(b) == comp) {
        let m = b.mesh().bounds();
        lo = solidcraft_geom::Vec3::new(lo.x.min(m.min.x), lo.y.min(m.min.y), lo.z.min(m.min.z));
        hi = solidcraft_geom::Vec3::new(hi.x.max(m.max.x), hi.y.max(m.max.y), hi.z.max(m.max.z));
    }
    (lo, hi)
}

#[test]
fn a_library_rivet_is_chosen_by_grip_and_kept_in_the_design() {
    let lib = Folder::new(&rivets());
    let mut s = Session::default();
    let opened = run(&mut s, "parts.library.load", json!({"path": lib.dir()}));
    assert_eq!(
        (opened["library"].as_str(), opened["families"].clone(), opened["problems"].clone()),
        (Some("example.rivets"), json!(["rivet_dome"]), json!([]))
    );
    // Listed beside the built-in families, with where it comes from.
    let all = run(&mut s, "parts.library", json!({}));
    let families = all["families"].as_array().cloned().unwrap_or_default();
    assert_eq!(families.iter().filter(|f| f["source"] == "built-in").count(), 7, "{all}");
    let rivet = families.iter().find(|f| f["family"] == "rivet_dome").cloned().unwrap_or_default();
    assert_eq!((rivet["source"].as_str(), rivet["shape"].as_str()), (Some("example.rivets"), Some("blind_rivet")), "{rivet}");
    assert_eq!(rivet["lengths"], json!([6.0, 8.0, 10.0, 12.0, 14.0]));
    // A 6.5 mm grip in size 4.8: the shortest length whose grip holds it is 12 (6–8).
    let r = run(&mut s, "parts.insert", json!({"family": "rivet_dome", "size": "4.8", "grip": 6.5, "point": [0, 0, 0]}));
    assert_eq!((r["length"].as_f64(), r["grip"].clone(), r["article"].as_str()), (Some(12.0), json!([6.0, 8.0]), Some("EX-48-12")), "{r}");
    // Named with its article, which the design keeps.
    assert_eq!(r["name"], "Rivet 4.8x12 EX-48-12");
    let comp = r["component"].as_u64().unwrap_or(0);
    assert_eq!(s.doc.parts.get(&comp).and_then(|p| p.article.as_deref()), Some("EX-48-12"));
    let (lo, hi) = bbox(&s, "Rivet 4.8x12 EX-48-12");
    assert!((hi.x - lo.x - 9.5).abs() < 1e-3 && (lo.z + 12.0).abs() < 1e-6 && (hi.z - 1.6).abs() < 1e-3, "{lo:?} {hi:?}");
    // Size 4.0 holds 6.5 with its longest length; nothing holds 11.
    let r40 = run(&mut s, "parts.insert", json!({"family": "example.rivets/rivet_dome", "size": "4.0", "grip": 6.5}));
    assert_eq!(r40["length"].as_f64(), Some(10.0), "{r40}");
    let err = s.execute("parts.insert", &json!({"family": "rivet_dome", "size": "4.8", "grip": 11})).unwrap_err().to_string();
    assert!(err.contains("grip of 11"), "{err}");
    // The design keeps the family: reopened without the library, the part still resizes.
    let doc = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap_or_else(|e| panic!("{e}"));
    let mut reopened = Session::new(doc);
    assert!(reopened.part_libraries.is_empty());
    let listed = run(&mut reopened, "parts.library", json!({"family": "rivet_dome"}));
    assert_eq!(listed["source"], "design", "{listed}");
    let r = run(&mut reopened, "parts.resize", json!({"component": comp, "grip": 9}));
    assert_eq!((r["length"].as_f64(), r["name"].as_str()), (Some(14.0), Some("Rivet 4.8x14")), "{r}");
    let (lo, _) = bbox(&reopened, "Rivet 4.8x14");
    assert!((lo.z + 14.0).abs() < 1e-6, "{lo:?}");
}

#[test]
fn a_malformed_library_is_refused_with_its_json_path() {
    let mut bad = rivets();
    bad["families"][0]["sizes"][1]["d"] = json!("4.8");
    let lib = Folder::new(&bad);
    let mut s = Session::default();
    let err = s.execute("parts.library.load", &json!({"path": lib.file()})).unwrap_err().to_string();
    assert!(err.contains("families[0].sizes[1].d") && err.contains("number"), "{err}");
    let checked = run(&mut s, "parts.library", json!({"path": lib.file(), "check": true}));
    assert_eq!((checked["ok"].as_bool(), checked["problems"][0]["path"].as_str()), (Some(false), Some("families[0].sizes[1].d")), "{checked}");
    // Unknown fields and values out of range are refused the same way.
    for (path, value, at) in [
        ("/families/0/sizes/0/colour", json!("red"), "families[0].sizes[0].colour"),
        ("/families/0/sizes/0/head/d", json!(3.0), "families[0].sizes[0].head.d"),
        ("/families/0/shape", json!("rocket"), "families[0].shape"),
        ("/format", json!("solidcraft-parts/9"), "format"),
    ] {
        let mut bad = rivets();
        let (parent, key) = path.rsplit_once('/').unwrap_or(("", path));
        if let Some(Value::Object(m)) = bad.pointer_mut(parent) {
            m.insert(key.to_string(), value);
        }
        let lib = Folder::new(&bad);
        let err = s.execute("parts.library.load", &json!({"path": lib.dir()})).unwrap_err().to_string();
        assert!(err.contains(at), "{path}: {err}");
    }
    assert!(s.part_libraries.is_empty());
    assert!(s.execute("parts.library.load", &json!({"path": "/nowhere/at/all"})).is_err());
}

#[test]
fn a_family_named_like_a_built_in_is_refused() {
    let mut lib = rivets();
    lib["families"][0]["name"] = json!("hex_nut");
    let f = Folder::new(&lib);
    let mut s = Session::default();
    let err = s.execute("parts.library.load", &json!({"path": f.dir()})).unwrap_err().to_string();
    assert!(err.contains("families[0].name") && err.contains("built-in"), "{err}");
    // Two libraries may not both have a family of one name; reopening one reloads it.
    let a = Folder::new(&rivets());
    run(&mut s, "parts.library.load", json!({"path": a.dir()}));
    run(&mut s, "parts.library.load", json!({"path": a.dir()}));
    assert_eq!(s.part_libraries.len(), 1);
    let mut other = rivets();
    other["id"] = json!("example.other");
    let b = Folder::new(&other);
    let err = s.execute("parts.library.load", &json!({"path": b.dir()})).unwrap_err().to_string();
    assert!(err.contains("already a family") && err.contains("example.rivets"), "{err}");
}

#[test]
fn check_reports_overlapping_grips_and_sizes_without_lengths() {
    let mut lib = rivets();
    lib["families"][0]["sizes"][0]["lengths"][1]["grip"] = json!([2.5, 5.0]);
    lib["families"][0]["sizes"][1]["lengths"] = json!([]);
    let f = Folder::new(&lib);
    let mut s = Session::default();
    // Usable, so it opens, but it says what is wrong.
    let opened = run(&mut s, "parts.library.load", json!({"path": f.dir()}));
    assert_eq!(opened["problems"].as_array().map(Vec::len), Some(2), "{opened}");
    let c = run(&mut s, "parts.library", json!({"family": "rivet_dome", "check": true}));
    assert_eq!(c["ok"], false);
    let paths: Vec<&str> = c["problems"].as_array().into_iter().flatten().filter_map(|p| p["path"].as_str()).collect();
    assert_eq!(paths, ["families[0].sizes[0].lengths[1].grip", "families[0].sizes[1].lengths"], "{c}");
    let good = Folder::new(&rivets());
    let c = run(&mut s, "parts.library", json!({"path": good.file(), "check": true}));
    assert_eq!((c["ok"].as_bool(), c["library"].as_str()), (Some(true), Some("example.rivets")), "{c}");
    assert_eq!(run(&mut s, "parts.library", json!({"family": "hex_nut", "check": true}))["ok"], true);
    assert!(s.execute("parts.library", &json!({"check": true})).is_err());
}

#[test]
fn library_bolts_nuts_and_washers_are_built_from_their_tables() {
    let lib = json!({
        "format": "solidcraft-parts/1", "id": "example.hardware", "name": "Example hardware",
        "families": [
            {"kind": "fastener_family", "name": "flange_bolt", "label": "Bolt", "standard": "example", "shape": "bolt",
             "sizes": [{"size": "M5", "d": 5.0, "pitch": 0.8, "head": {"d": 8.0, "h": 3.5, "style": "hex"},
                        "lengths": [{"length": 12, "grip": [0, 7]}, {"length": 16, "grip": [7, 11]}]}]},
            {"kind": "fastener_family", "name": "plain_nut", "label": "Nut", "shape": "nut",
             "sizes": [{"size": "M5", "d": 5.0, "pitch": 0.8, "head": {"d": 8.0, "h": 4.0}}]},
            {"kind": "fastener_family", "name": "flat_washer", "label": "Washer", "shape": "washer",
             "sizes": [{"size": "M5", "d": 5.3, "head": {"d": 10.0, "h": 1.0}}]}
        ]
    });
    let f = Folder::new(&lib);
    let mut s = Session::default();
    run(&mut s, "parts.library.load", json!({"path": f.file()}));
    let b = run(&mut s, "parts.insert", json!({"family": "flange_bolt", "size": "M5", "grip": 9}));
    assert_eq!(b["length"].as_f64(), Some(16.0), "{b}");
    let (lo, hi) = bbox(&s, "Bolt M5x16");
    assert!((lo.z + 16.0).abs() < 1e-6 && (hi.z - 3.5).abs() < 1e-6, "{lo:?} {hi:?}");
    assert!(s.model.state().threads.iter().any(|t| t.designation.starts_with("M5")), "{:?}", s.model.state().threads);
    run(&mut s, "parts.insert", json!({"family": "plain_nut", "size": "M5"}));
    let (lo, hi) = bbox(&s, "Nut M5");
    assert!((hi.z - lo.z - 4.0).abs() < 1e-6, "{lo:?} {hi:?}");
    run(&mut s, "parts.insert", json!({"family": "flat_washer", "size": "M5"}));
    let (lo, hi) = bbox(&s, "Washer M5");
    assert!((hi.x - lo.x - 10.0).abs() < 1e-3 && (hi.z - lo.z - 1.0).abs() < 1e-6, "{lo:?} {hi:?}");
    assert!(s.execute("parts.insert", &json!({"family": "hex_bolt", "size": "M5", "grip": 4})).is_err(), "built-ins list no grips");
}
