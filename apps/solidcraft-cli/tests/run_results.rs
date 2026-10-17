//! Result reporting must not skip requested files or execute a script twice.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("solidcraft-run-results-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn script(&self) -> PathBuf {
        let path = self.0.join("box.json");
        std::fs::write(&path, json!({"commands": [{"command": "solid.box", "params": {"length": 2, "width": 3, "height": 4}}]}).to_string()).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_solidcraft-cli"))
}

fn json_output(output: Output) -> Value {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn body_count(path: &Path) -> Value {
    json_output(cli().arg("eval").arg(path).output().unwrap())["body_count"].clone()
}

#[test]
fn cli_loads_both_design_encodings_by_contents() {
    let dir = Scratch::new();
    for extension in ["solidcraft", "solvecraft", "json"] {
        let saved = dir.0.join(format!("box.{extension}"));
        json_output(cli().arg("run").arg(dir.script()).arg("--save").arg(&saved).output().unwrap());
        let bytes = std::fs::read(&saved).unwrap();
        assert_eq!(solidcraft_engine::io::is_design_container(&bytes), extension != "json");
        let renamed = dir.0.join(format!("{extension}.unusual"));
        std::fs::write(&renamed, bytes).unwrap();
        let measured = json_output(cli().arg("eval").arg(&renamed).output().unwrap());
        assert_eq!(measured["body_count"], 1);
        assert!((measured["bodies"][0]["volume_mm3"].as_f64().unwrap() - 24.0).abs() < 1e-6);
    }
}

#[test]
fn results_still_save_and_export_the_script_once() {
    let dir = Scratch::new();
    // JSON is explicit here because the fixture edits its saved format tag.
    let saved = dir.0.join("box.json");
    let stl = dir.0.join("box.stl");
    let obj = dir.0.join("box.obj");
    let result = json_output(
        cli().arg("run").arg(dir.script()).arg("--results").arg("--save").arg(&saved).arg("--out").arg(&stl).arg("--out").arg(&obj).output().unwrap(),
    );
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(body_count(&saved), 1);
    for exported in [&stl, &obj] {
        assert!(std::fs::metadata(exported).unwrap().len() > 0);
        assert_eq!(body_count(exported), 1);
    }
}

#[test]
fn results_with_base_design_and_quiet_still_report_and_save() {
    let dir = Scratch::new();
    let script = dir.script();
    let base = dir.0.join("base.solidcraft");
    let saved = dir.0.join("combined.solidcraft");
    json_output(cli().arg("run").arg(&script).arg("--save").arg(&base).output().unwrap());
    let result = json_output(cli().args(["run", "--results", "--quiet", "--in"]).arg(&base).arg(&script).arg("--save").arg(&saved).output().unwrap());
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(body_count(&saved), 2);
    assert_eq!(body_count(&base), 1);
}

#[test]
fn results_propagate_output_errors() {
    let dir = Scratch::new();
    for flag in ["--save", "--out"] {
        let path = dir.0.join("missing").join(if flag == "--save" { "box.solidcraft" } else { "box.stl" });
        let output = cli().arg("run").arg(dir.script()).arg("--results").arg(flag).arg(&path).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(!output.stderr.is_empty());
        assert!(output.stdout.is_empty());
        assert!(!path.exists());
    }
}

#[test]
fn results_without_script_still_fail() {
    let dir = Scratch::new();
    let base = dir.0.join("base.solidcraft");
    json_output(cli().arg("run").arg(dir.script()).arg("--save").arg(&base).output().unwrap());
    let output = cli().args(["run", "--results", "--in"]).arg(&base).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--results needs a script"));
}

/// Compatibility shim (docs/compatibility.md): a design saved before the rename, with the
/// legacy extension and format tag, still opens; re-saving writes the current tag.
#[test]
fn legacy_extension_and_format_tag_still_open() {
    let dir = Scratch::new();
    // JSON is explicit here because the fixture edits its saved format tag.
    let saved = dir.0.join("box.json");
    json_output(cli().arg("run").arg(dir.script()).arg("--save").arg(&saved).output().unwrap());
    let text = std::fs::read_to_string(&saved).unwrap();
    assert!(text.contains("\"solidcraft/"), "new designs carry the current tag");
    let legacy = dir.0.join("old.solvecraft");
    std::fs::write(&legacy, text.replace("\"solidcraft/", "\"solvecraft/")).unwrap();
    assert_eq!(body_count(&legacy), 1);
    let resaved = dir.0.join("resaved.solidcraft");
    json_output(cli().args(["run", "--in"]).arg(&legacy).arg(dir.script()).arg("--save").arg(&resaved).output().unwrap());
    assert_eq!(body_count(&resaved), 2);
    let reopened = solidcraft_engine::io::read_design(&std::fs::read(&resaved).unwrap()).unwrap();
    assert!(reopened.to_json().contains("\"solidcraft/"));
}

/// `snapshot` starts from `--in` as `run` does (its script runs on that design), and refuses an
/// option it doesn't take rather than ignoring it.
#[test]
fn snapshot_runs_its_script_on_the_in_design() {
    let dir = Scratch::new();
    let base = dir.0.join("base.solidcraft");
    json_output(cli().arg("run").arg(dir.script()).arg("--save").arg(&base).output().unwrap());
    let shifted = dir.0.join("shifted.json");
    std::fs::write(
        &shifted,
        json!({"commands": [{"command": "solid.box", "params": {"length": 2, "width": 3, "height": 4, "corner": [10, 0, 0]}}]}).to_string(),
    )
    .unwrap();
    let shot = |name: &str, args: &[&std::ffi::OsStr]| {
        let png = dir.0.join(name);
        json_output(cli().arg("snapshot").args(args).arg("--out").arg(&png).args(["--width", "64", "--height", "48"]).output().unwrap());
        std::fs::read(&png).unwrap()
    };
    // Pictures are drawn on the GPU: without an adapter only the option checks below run.
    match solidcraft_gpu::capture::shared() {
        Ok(_) => {
            let alone = shot("alone.png", &[shifted.as_os_str()]);
            let on_base = shot("on_base.png", &[shifted.as_os_str(), "--in".as_ref(), base.as_os_str()]);
            let base_only = shot("base.png", &["--in".as_ref(), base.as_os_str()]);
            assert_ne!(alone, on_base, "the script ran on the --in design");
            assert_ne!(base_only, on_base);
        }
        Err(e @ solidcraft_gpu::GpuError::NoAdapter(_)) if std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none() => {
            eprintln!("pictures left out: {e}")
        }
        Err(e) => panic!("{e}"),
    }
    for (bad, says) in [(["--frobnicate", "1"], "doesn't take --frobnicate"), (["--width", "wide"], "--width must be 16 to 8192")] {
        let output = cli().arg("snapshot").arg(&base).arg("--out").arg(dir.0.join("x.png")).args(bad).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains(says), "{}", String::from_utf8_lossy(&output.stderr));
    }
}

/// An unknown command's error suggests the id that was meant.
#[test]
fn unknown_command_suggests_the_closest_id() {
    let output = cli().args(["exec", "file.open"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("did you mean `doc.open`?"), "{}", String::from_utf8_lossy(&output.stderr));
}

/// #1833: `--timings` reports each command's time instead of the measurement, `--profile` adds
/// the kernel's share, `--results` puts the results beside them, and files are still written.
#[test]
fn timings_and_profile_report_each_command() {
    let dir = Scratch::new();
    let saved = dir.0.join("timed.solidcraft");
    let t = json_output(cli().arg("run").arg(dir.script()).arg("--timings").arg("--save").arg(&saved).output().unwrap());
    let rows = t["timings"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["index"].as_u64(), rows[0]["command"].as_str()), (Some(0), Some("solid.box")));
    assert!(rows[0]["ms"].as_f64().is_some() && t["total_ms"].as_f64().is_some(), "{t}");
    assert!(rows[0].get("profile").is_none() && t.get("results").is_none(), "{t}");
    assert_eq!(body_count(&saved), 1);

    // --profile alone implies --timings; a flag before the script doesn't eat it.
    let t = json_output(cli().arg("run").arg("--profile").arg(dir.script()).arg("--results").output().unwrap());
    let p = &t["timings"][0]["profile"];
    for k in ["boolean_ms", "tessellate_ms", "other_ms"] {
        assert!(p[k].as_f64().is_some_and(|ms| ms >= 0.0), "{k}: {t}");
    }
    assert_eq!(t["results"].as_array().map(Vec::len), Some(1), "{t}");

    // --results alone stays a plain array.
    assert!(json_output(cli().arg("run").arg(dir.script()).arg("--results").output().unwrap()).is_array());

    let output = cli().arg("run").arg("--in").arg(&saved).arg("--timings").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--timings needs a script"));
}
