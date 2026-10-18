//! A design's command script (`timeline.export_script`, #886) replays into the same design:
//! every corpus design that the script supports comes back with the same timeline, parameters,
//! settings and bodies.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solidcraft_engine::Session;

fn corpus() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut v: Vec<PathBuf> =
        std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "solidcraft")).collect();
    v.sort();
    assert!(v.len() >= 7, "corpus files in {}", dir.display());
    v
}

/// Designs with components or joints, which scripts do not support yet.
const ASSEMBLIES: [&str; 2] = ["components-arm", "joints-hinge"];

fn bodies(s: &mut Session) -> Vec<(String, f64, f64, Value)> {
    let m = s.execute("inspect.measure", &json!({})).unwrap();
    let mut v: Vec<_> = m["bodies"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| {
            (
                b["name"].as_str().unwrap_or_default().to_string(),
                b["volume_mm3"].as_f64().unwrap(),
                b["area_mm2"].as_f64().unwrap(),
                b["faces"].clone(),
            )
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn errors(s: &Session) -> Vec<(u64, Option<String>)> {
    s.model.results.iter().map(|r| (r.id, r.error.clone())).collect()
}

fn params(s: &Session) -> Vec<solidcraft_engine::doc::Param> {
    let mut v = s.doc.params.clone();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

#[test]
fn corpus_designs_replay_from_their_scripts() {
    let mut replayed = 0;
    for p in corpus() {
        let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let mut s = Session::default();
        s.execute("doc.open", &json!({"path": p.to_string_lossy()})).unwrap();
        let r = match s.execute("timeline.export_script", &json!({})) {
            Ok(r) => r,
            Err(e) => {
                assert!(ASSEMBLIES.contains(&name.as_str()), "{name}: {e}");
                assert!(e.to_string().contains("not supported yet"), "{name}: {e}");
                continue;
            }
        };
        assert!(!ASSEMBLIES.contains(&name.as_str()), "{name} has components: the script should refuse it");
        // Through text, as a script file would carry it.
        let script: Value = serde_json::from_str(&r["script"].to_string()).unwrap();
        let mut t = Session::default();
        t.run_script(&script).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(t.doc.features, s.doc.features, "{name}: timeline");
        assert_eq!(params(&t), params(&s), "{name}: parameters");
        assert_eq!(t.doc.marker, s.doc.marker, "{name}: marker");
        assert_eq!(t.doc.units, s.doc.units, "{name}: units");
        assert_eq!(t.doc.sheet, s.doc.sheet, "{name}: sheet metal rules");
        assert_eq!(t.doc.plastic, s.doc.plastic, "{name}: plastic rules");
        assert_eq!(t.doc.materials, s.doc.materials, "{name}: materials");
        assert_eq!(t.doc.user_materials, s.doc.user_materials, "{name}: user materials");
        assert_eq!(t.doc.favorites, s.doc.favorites, "{name}: favourites");
        assert_eq!(t.doc.param_comments, s.doc.param_comments, "{name}: comments");
        assert_eq!(errors(&t), errors(&s), "{name}: feature results");
        assert_eq!(bodies(&mut t), bodies(&mut s), "{name}: bodies");
        replayed += 1;
    }
    assert!(replayed >= 7, "replayed {replayed} corpus designs");
}

/// #1990: a sketch and its extrude may share a name, and the design's script still replays,
/// keeping the names: the replayed design's own script has the same steps.
#[test]
fn a_design_naming_a_sketch_and_its_extrude_alike_replays_from_its_script() {
    let dir = std::env::temp_dir().join(format!("solidcraft-shared-names-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cli = |args: &[&std::ffi::OsStr]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_solidcraft-cli")).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{}: {}",
            args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let script = dir.join("mini.json");
    let saved = dir.join("mini.solidcraft");
    let exported = dir.join("rt.json");
    let replayed = dir.join("rt.solidcraft");
    let again = dir.join("rt2.json");
    let commands = json!({"commands": [
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Block"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [20, 10]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": "5", "name": "Block"}},
    ]});
    std::fs::write(&script, commands.to_string()).unwrap();
    cli(&["run".as_ref(), script.as_ref(), "--save".as_ref(), saved.as_ref()]);
    cli(&["script".as_ref(), saved.as_ref(), "--out".as_ref(), exported.as_ref()]);
    cli(&["run".as_ref(), exported.as_ref(), "--save".as_ref(), replayed.as_ref()]);
    cli(&["script".as_ref(), replayed.as_ref(), "--out".as_ref(), again.as_ref()]);
    let first: Value = serde_json::from_str(&std::fs::read_to_string(&exported).unwrap()).unwrap();
    let second: Value = serde_json::from_str(&std::fs::read_to_string(&again).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let names: Vec<&str> = first["commands"].as_array().unwrap().iter().filter_map(|c| c["params"]["feature"]["name"].as_str()).collect();
    assert_eq!(names, ["Block", "Block"], "{first:#}");
    assert_eq!(second["commands"], first["commands"], "the replayed design's script has the same steps");
}
