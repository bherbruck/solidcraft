//! Workspace tooling: `cargo xtask <command>`.
//!
//! Rust workspace tooling. External tools (`cargo`, and Python 3.11+ for the
//! original icon collection) are invoked through `std::process::Command`.

mod app_icons;
mod assets;
mod book;
mod features;
mod i18n;
mod icons;
mod layers;
mod licences;
mod lists;
mod test;
mod web_offline;
mod web_offline_test;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  layers          enforce the crate dependency layering (plan/architecture.md §3)
  assets          check that every non-code asset file is attributed in ATTRIBUTION.md
  app-icons       regenerate shared app-icon exports for Linux, FreeBSD and Windows
  icons           validate UI icon standards, discovery and review-tool behavior (Python 3.11+)
  features        list SolidCraft's commands by area in docs/features.md (from the registry)
  i18n [--check | --add-missing]
                  report how complete the message catalogs in assets/locales are:
                  which messages a locale has not translated and which command labels the English
                  catalog cannot translate (so they stay English everywhere). --check fails when
                  there is a gap, for CI; --add-missing writes the uncovered command labels into
                  en.json under cmd.<command id> so only the translations are left to do
  oracle [--cli BIN]
                  replay the reference tests (tests/oracle/*/recipe.json), compare with each
                  case's expected.json and write docs/oracle.md (--cli: a solidcraft-cli
                  already built, instead of cargo run --release)
  action-matrix [--bless] [--results FILE] [--from DIR]
                  try every ordinary action on every face, edge, vertex and body of the design
                  corpus and compare with tests/action_matrix/baseline.json (--bless rewrites it;
                  --results keeps every outcome with its message; --from takes the outcomes of the
                  nightly shards' artifacts in DIR instead of running, and --bless then also
                  rewrites tests/action_matrix/costs.json). See docs/action-matrix.md
  book [--no-build]
                  generate docs/book's command reference and screenshots, then mdbook build
                  into target/book
  licences [--check]
                  write THIRD-PARTY-LICENSES.txt (the licences of every shipped crate, embedded
                  in the programs); --check only reports when it is out of date: pull requests
                  leave it to the Generated files workflow on main
  lists [--fix]   check that files many changes add to are sorted one item per line (the
                  message catalogs in assets/locales); --fix sorts them
  test [CARGO ARGS] [-- TEST ARGS]
                  build the tests with the ci profile, then run every test binary at once
                  (and the doc tests alongside)
  wasm            cargo check every library crate and the web app for wasm32-unknown-unknown
  web-offline [DIR]
                  write the web build's service worker (sw.js: offline use, updates); run by
                  Trunk after release builds of apps/solidcraft-web, or on a built site in DIR
  web-api-test [--site DIR]  Test the command API in a real browser build (default: dist/web)
  web-offline-test [--site DIR]
                  check in headless Chrome that the web app works offline and updates to a new
                  build (a stand-in app, or the web build in DIR such as dist/web)
  ci              fmt --check, assets, icons, licences --check, lists, layers, clippy -D warnings, then the tests
                  with the wasm check alongside, then web-offline-test when Chrome is installed
                  (stops at the first failing step)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match args.first().map(String::as_str) {
        Some("layers") => cmd_layers(),
        Some("assets") => assets::run(&root()),
        Some("icons") => icons::run(&root()),
        Some("app-icons") => app_icons::run(&root()),
        Some("features") => features::run(&root()),
        Some("i18n") => i18n::run(&root(), &rest),
        Some("oracle") => cmd_oracle(rest.iter().position(|a| *a == "--cli").and_then(|i| rest.get(i + 1)).copied()),
        Some("action-matrix") => {
            let arg = |name: &str| rest.iter().position(|a| *a == name).and_then(|i| rest.get(i + 1)).copied();
            cmd_action_matrix(rest.contains(&"--bless"), arg("--results"), arg("--from"))
        }
        Some("book") => book::run(&root(), !rest.contains(&"--no-build")),
        Some("licences") => licences::run(&root(), rest.contains(&"--check")),
        Some("lists") => lists::run(&root(), rest.contains(&"--fix")),
        Some("test") => {
            let (cargo_args, test_args) = rest.iter().position(|a| *a == "--").map_or((&rest[..], &[][..]), |i| (&rest[..i], &rest[i + 1..]));
            test::run(cargo_args, test_args, vec![test::doc_tests(cargo_args, test_args)])
        }
        Some("ci") => cmd_ci(),
        Some("wasm") => cmd_wasm(),
        Some("web-offline") => web_offline::run(&root(), rest.first().copied()),
        Some("web-offline-test") => {
            let site = rest.iter().position(|a| *a == "--site").and_then(|i| rest.get(i + 1)).copied();
            web_offline_test::run(&root(), site, true)
        }
        Some("web-api-test") => {
            let site = rest.iter().position(|a| *a == "--site").and_then(|i| rest.get(i + 1)).copied().unwrap_or("dist/web");
            web_offline_test::run_api(site)
        }
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Workspace root (parent of the xtask crate).
pub fn root() -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.parent().unwrap_or(here).to_path_buf()
}

pub fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    eprintln!("$ {what}");
    let status = cmd.status().map_err(|e| format!("{what}: failed to spawn: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what}: exited with {status}")) }
}

pub fn metadata() -> Result<serde_json::Value, String> {
    let out = cargo().args(["metadata", "--format-version", "1", "--no-deps"]).output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: bad JSON: {e}"))
}

/// The full action matrix (apps/solidcraft-cli/tests/action_matrix.rs), with the ci profile.
fn cmd_action_matrix(bless: bool, results: Option<&str>, from: Option<&str>) -> Result<(), String> {
    let mut c = cargo();
    c.args([
        "test",
        "--profile",
        "ci",
        "-p",
        "solidcraft-cli",
        "--test",
        "action_matrix",
        "action_matrix_all",
        "--",
        "--exact",
        "--ignored",
        "--nocapture",
    ]);
    if bless {
        c.env("ACTION_MATRIX_BLESS", "1");
    }
    if let Some(r) = results {
        let p = std::path::absolute(r).map_err(|e| format!("{r}: {e}"))?;
        c.env("ACTION_MATRIX_RESULTS", p);
    }
    if let Some(d) = from {
        let p = std::path::absolute(d).map_err(|e| format!("{d}: {e}"))?;
        c.env("ACTION_MATRIX_MERGE", p);
    }
    run(c, "cargo test --profile ci -p solidcraft-cli --test action_matrix action_matrix_all")
}

fn cmd_layers() -> Result<(), String> {
    let crates = layers::from_metadata(&metadata()?)?;
    println!("Dependency layering (plan/architecture.md §3)\n");
    println!("{:<28} {:<14} workspace deps", "crate", "layer");
    for c in &crates {
        let ws: Vec<String> = c
            .deps
            .iter()
            .filter(|d| d.workspace)
            .map(|d| {
                let k = match d.kind {
                    layers::DepKind::Normal => "",
                    layers::DepKind::Dev => " (dev)",
                    layers::DepKind::Build => " (build)",
                };
                format!("{}{k}", layers::short_name(&d.name))
            })
            .collect();
        println!("{:<28} {:<14} {}", c.name, layers::describe(layers::classify(&c.name)), ws.join(", "));
    }
    let violations = layers::check(&crates);
    println!();
    if violations.is_empty() {
        println!("OK: {} crates, no layering violations.", crates.len());
        Ok(())
    } else {
        println!("{} violation(s):", violations.len());
        for v in &violations {
            println!("  - {v}");
        }
        Err(format!("{} layering violation(s)", violations.len()))
    }
}

/// Features with no reference case yet, and the tests that check them against hand-computed
/// geometry.
const NO_ORACLE: [(&str, &str); 5] = [
    ("Boss (draft, root fillet, hole, ribs)", "post, hole and rib volumes (`boss_with_hole_fillet_and_ribs`)"),
    ("Lip / Groove", "band offsets and groove clearance on a shelled box (`lip_and_groove_on_a_shelled_box`, `groove_takes_the_rule_clearance`)"),
    ("Snap Fit", "arm and catch volume, hook direction (`snap_fit_arm_and_catch`)"),
    ("Rest", "rectangle, round, hollow and drafted pads (`rest_pads`)"),
    (
        "Plastic Rules (manage, assign)",
        "library values, Thickness-based values, rule draft as the boss default (`plastic_rules_library_edit_and_assign`)",
    ),
];

/// Replay every reference case in tests/oracle, compare with its expected.json and write
/// docs/oracle.md. Fails when a case fails, unless its expected.json names the issue that
/// tracks it (`known_failure`); a known failure that passes fails too, so the mark is removed.
fn cmd_oracle(cli: Option<&str>) -> Result<(), String> {
    let dir = root().join("tests/oracle");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("recipe.json").is_file())
        .collect();
    cases.sort();
    if cases.is_empty() {
        return Err(format!("no cases with recipe.json in {}", dir.display()));
    }
    let mut cmd = match cli {
        Some(bin) => {
            let mut c = Command::new(bin);
            c.current_dir(root());
            c
        }
        None => {
            let mut c = cargo();
            c.args(["run", "-q", "--release", "-p", "solidcraft-cli", "--"]);
            c
        }
    };
    cmd.args(["oracle", "--json"]);
    for c in &cases {
        cmd.arg(c.strip_prefix(root()).unwrap_or(c));
    }
    let out = cmd.output().map_err(|e| format!("solidcraft-cli: {e}"))?;
    let results: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("oracle JSON: {e}\n{}", String::from_utf8_lossy(&out.stderr)))?;
    let list = results.as_array().cloned().unwrap_or_default();
    let expected = |case: &str| -> serde_json::Value {
        std::fs::read_to_string(dir.join(case).join("expected.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    };
    let (mut pass, mut missing, mut known, mut failed, mut fixed) = (0, 0, 0, Vec::new(), Vec::new());
    let mut sources: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut rows = String::new();
    for r in &list {
        let case = r["case"].as_str().unwrap_or("");
        let want = expected(case);
        let source = want["source"].as_str().unwrap_or("none yet").to_string();
        let source_kind = source.split_whitespace().next().unwrap_or("").to_string();
        *sources.entry(if want.is_null() { "none yet".into() } else { source_kind }).or_default() += 1;
        let ok = r["pass"].as_bool().unwrap_or(false);
        let issue = want["known_failure"].as_str().map(str::to_string);
        let status = match (ok, want.is_null(), &issue) {
            (_, true, _) => {
                missing += 1;
                "no expected values yet".to_string()
            }
            (true, _, Some(i)) => {
                fixed.push(format!("{case} ({i})"));
                format!("pass (marked known failure {i}: remove the mark)")
            }
            (true, _, None) => {
                pass += 1;
                "pass".to_string()
            }
            (false, _, Some(i)) => {
                known += 1;
                format!("known failure ({i})")
            }
            (false, _, None) => {
                failed.push(case.to_string());
                "**FAIL**".to_string()
            }
        };
        let mut notes: Vec<String> = r["checks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| !c["ok"].as_bool().unwrap_or(false))
            .map(|c| format!("{}: got {} want {}", c["check"].as_str().unwrap_or(""), c["got"], c["want"]))
            .collect();
        if let Some(e) = r["error"].as_str().filter(|_| !want.is_null()) {
            notes.push(e.to_string());
        }
        if let Some(n) = want["note"].as_str() {
            notes.push(n.to_string());
        }
        let how = want["method"].as_str().unwrap_or("");
        let surface = r["checks"]
            .as_array()
            .and_then(|cs| cs.iter().find(|c| c["check"] == "surface_distance_mm"))
            .and_then(|c| Some((c["got"]["max"].as_f64()?, c["got"]["mean"].as_f64()?, c["got"]["worst_at"].as_array()?.clone())))
            .map(|(max, mean, at)| {
                let at: Vec<String> = at.iter().filter_map(|x| x.as_f64()).map(|x| format!("{x:.1}")).collect();
                format!("{max:.3} / {mean:.4} (worst at {})", at.join(", "))
            })
            .unwrap_or_default();
        rows += &format!("| {case} | {source} | {status} | {surface} | {} | {} |\n", how.replace('|', "/"), notes.join("; ").replace('|', "/"));
    }
    let counts: Vec<String> = sources.iter().map(|(k, v)| format!("{v} {k}")).collect();
    let mut md = format!(
        "# Reference tests\n\nGenerated by `cargo xtask oracle`. Each case in [`tests/oracle/`](../tests/oracle/) is a part described by our own `recipe.json`. SolidCraft replays the recipe through its command engine and compares the result with `expected.json`: body count, volume and area (relative tolerance 1e-3 unless the case states another), centre of mass and bounding box (within 1e-3 of the part's size), face count where the geometry fixes it, and, where the case has a `reference.stl`, the sampled symmetric (Hausdorff) distance between the two surfaces and the principal second moments of both triangulations. Every rebuilt part is also exported as STEP, validated and read back.\n\nWhere the expected values come from (the **Source** column):\n\n- **analytic**: closed-form geometry worked out by hand (prisms, cylinders, cones, spheres, tori and Pappus' theorems, fillets on straight edges with a constant radius or one varying along the edge, sheet-metal bends as annular sectors). The scripts in [`tools/oracle-analytic/`](../tools/oracle-analytic/) compute them and give the formulas.\n- **freecad &lt;version&gt;**: the same recipe built with FreeCAD's public Python scripting API by [`tools/freecad-oracle/`](../tools/freecad-oracle/), an independent kernel run as an external program. FreeCAD is not a dependency of SolidCraft. Where the two kernels legitimately build a different shape (for example the corner patch where fillets meet), only volume and area are compared, within the tolerance the case states.\n\nNo expected value comes from a commercial CAD product.\n\n**{pass} / {} cases pass**; sources: {}. {known} known failure(s) tracked in issues; {missing} case(s) without expected values yet.\n\n| Case | Source | Result | Surface distance mm (max / mean) | Method | Notes |\n|---|---|---|---|---|---|\n",
        list.len(),
        counts.join(", ")
    );
    md += &rows;
    md += "\n## Not covered by a reference case\n\n| Feature | Tested by |\n|---|---|\n";
    for (feature, test) in NO_ORACLE {
        md += &format!("| {feature} | {test} |\n");
    }
    std::fs::write(root().join("docs/oracle.md"), md).map_err(|e| format!("docs/oracle.md: {e}"))?;
    println!("oracle: {pass}/{} cases pass, {known} known failures, {missing} without expected values → docs/oracle.md", list.len());
    if !failed.is_empty() {
        return Err(format!("{} case(s) failed: {}", failed.len(), failed.join(", ")));
    }
    if !fixed.is_empty() {
        return Err(format!("now passing, remove `known_failure` from expected.json: {}", fixed.join(", ")));
    }
    Ok(())
}

/// Never break wasm: every layered crate (everything below the apps) and the web app must
/// build for the browser.
fn cmd_wasm() -> Result<(), String> {
    let (c, n) = wasm_check()?;
    run(c, &format!("cargo check --target wasm32-unknown-unknown ({n} crates)"))?;
    println!("wasm: {n} crates build for wasm32-unknown-unknown");
    Ok(())
}

/// The wasm check and how many crates it covers. It uses the ci profile so the host build
/// scripts and proc macros built for clippy and the tests are reused.
fn wasm_check() -> Result<(Command, usize), String> {
    let crates = layers::from_metadata(&metadata()?)?;
    let mut set: Vec<String> =
        crates.iter().filter(|c| matches!(layers::classify(&c.name), Some(layers::Class::Layer(_)))).map(|c| c.name.clone()).collect();
    set.push("solidcraft-web".into());
    let mut c = cargo();
    c.args(["check", "--target", "wasm32-unknown-unknown", "--profile", "ci"]);
    for p in &set {
        c.args(["-p", p]);
    }
    Ok((c, set.len()))
}

/// The quick gates first, so they fail fast; the wasm check (and doc tests) run while the test
/// binaries do, which leaves cargo's build lock free.
fn cmd_ci() -> Result<(), String> {
    let mut c = cargo();
    c.args(["fmt", "--all", "--", "--check"]);
    run(c, "cargo fmt --check")?;
    assets::run(&root())?;
    icons::run(&root())?;
    licences::run(&root(), true)?;
    lists::run(&root(), false)?;
    cmd_layers()?;
    let mut c = cargo();
    c.args(["clippy", "--workspace", "--all-targets", "--profile", "ci", "--", "-D", "warnings"]);
    run(c, "cargo clippy -D warnings")?;
    let (wasm, n) = wasm_check()?;
    test::run(&[], &[], vec![(format!("wasm32-unknown-unknown check ({n} crates)"), wasm), test::doc_tests(&[], &[])])?;
    web_offline_test::run(&root(), None, false)?;
    eprintln!("ci: all gates passed");
    Ok(())
}
