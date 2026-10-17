//! SolidCraft headless command line.
//!
//! ```text
//! solidcraft-cli run <script.json|design.solidcraft|part.step|mesh.3mf> [--in design|part.step|mesh.3mf] [--out FILE]... [--save FILE] [--quiet] [--results] [--timings] [--profile] [--json-out FILE] [--allow-errors]
//! solidcraft-cli eval <script.json|design.solidcraft|part.step> [--json-out FILE] (alias: inspect) measurements as JSON
//! solidcraft-cli script <design.solidcraft> [--out script.json]     the design's history as a command script
//! solidcraft-cli bench-open <design.solidcraft | part.step> [--parse-only] [--display LEVEL [--edit NAME EXPR]] loading timings as JSON lines
//! solidcraft-cli bench-agent [--scale F] [--save F] [--script-out F] a ~1,300-body design built command by command, timed
//! solidcraft-cli snapshot <script|design> [--in design|part.step] --out shot.png [--width W] [--height H] [--view iso|front|top|…|NAME | --camera JSON] [--no-grid] [--no-axes]
//! solidcraft-cli exec <command> [json-params]                 run one command on an empty design
//! solidcraft-cli commands                                     the command registry as JSON
//! solidcraft-cli recipe <recipe.json>                         translate an oracle recipe to a script
//! solidcraft-cli oracle <case-dir>...                         replay recipes and compare with expected.json
//! solidcraft-cli mcp [--in design|script] [--connect HOST:PORT [--token T]] [--allow-path DIR]... [--allow-any-path]
//!                                                             MCP server on stdio (docs/mcp.md)
//! solidcraft-cli licences                                     SolidCraft's licence and the third-party licences
//! solidcraft-cli icon --out F.png|F.ico [--size N]             the app icon (packaging; drawn, not a file)
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod bench_agent;
mod crashes;
mod fuzz;
mod oracle;
mod recipe;
mod render;
mod spatial;

use std::process::ExitCode;

use serde_json::{Value, json};
#[path = "../../../assets/icons/app/shared/source/app_icon.rs"]
mod app_icon;

use solidcraft_engine::Session;
use solidcraft_engine::view::Guides;

const USAGE: &str = "usage:
  solidcraft-cli run <script.json|design.solidcraft|part.step> [--in design.solidcraft|part.step] [--out FILE]... [--save FILE] [--quiet] [--results] [--timings] [--profile] [--json-out FILE] [--allow-errors]
  solidcraft-cli run --in <design.solidcraft|part.step> [--out FILE]...
      (a timeline feature that fails fails the run and nothing is written; --allow-errors writes anyway)
      (--timings prints {timings: [{index, command, ms}], total_ms} for a script, with --results its results too;
       --profile adds each command's profile: {boolean_ms, tessellate_ms, other_ms}, the kernel's share)
  solidcraft-cli eval <script.json|design.solidcraft|part.step> [--json-out FILE]     (alias: inspect; or --in FILE)
      (--json-out FILE writes the JSON that run (with --results, else the measurement) and eval print to FILE instead of stdout,
       as plain UTF-8: a shell's > can re-encode it, e.g. Windows PowerShell adds a byte-order mark)
  solidcraft-cli script <design.solidcraft|script.json> [--out script.json]
      (the design's history as a command script that eval and run replay)
  solidcraft-cli bench-open <design.solidcraft | part.step> [--parse-only] [--display LEVEL [--edit NAME EXPR]]
  solidcraft-cli bench-agent [--scale F] [--save design.solidcraft] [--script-out script.json]
  solidcraft-cli snapshot <script|design> [--in design.solidcraft|part.step] --out shot.png [--width W] [--height H] [--view NAME | --camera JSON] [--no-grid] [--no-axes]
  solidcraft-cli snapshot --in <design.solidcraft|part.step> --out shot.png ...
      (as with run, --in starts from a design and the script runs on top of it; an option snapshot doesn't take is an error;
       --view: iso, front, back, top, bottom, left, right, or the name of a view saved in the design with view.save;
       --camera: {target: [x,y,z], yaw, pitch, distance, fov?} in mm and radians, as the MCP screenshot takes it (docs/mcp.md);
       --no-grid and --no-axes leave the ground grid (on Z 0) and the origin's axes out of the picture)
  solidcraft-cli render <script|design> --out image.png [--setup NAME] [--width W] [--height H] [--transparent] [--overwrite] [--timeout SECONDS]
  solidcraft-cli exec <command> [json-params]
  solidcraft-cli commands
  solidcraft-cli recipe <recipe.json>
  solidcraft-cli oracle <case-dir>... [--json]
  solidcraft-cli fuzz [--from N] [--count N] [--jobs J] [--timeout S] [--steps N] [--minimise] [--out DIR] [--report FILE]
  solidcraft-cli mcp [--in design.solidcraft|script.json] [--connect 127.0.0.1:PORT [--token T]] [--allow-path DIR]... [--allow-any-path]
  solidcraft-cli crashes [list [--json] | show <id> | replay <id> | dir | clear [<id>]]
  solidcraft-cli licences
  solidcraft-cli gpu [--policy auto|hardware|software|cpu]
      (the GPU adapters a headless render can open, best first, the one the policy picks and the tier it
       lands on: gpu, gpu-software, or the CPU rasterizer (auto with no adapter, or cpu); the policy defaults
       to SOLIDCRAFT_GPU, else auto; exits with an error when hardware or software finds no such adapter)
  solidcraft-cli icon --out icon.png [--size 256] | --out icon.ico
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("run") => cmd_run(&args[1..]),
        Some("eval" | "inspect") => cmd_eval(&args[1..]),
        Some("script") => cmd_script(&args[1..]),
        Some("snapshot") => cmd_snapshot(&args[1..]),
        Some("render") => render::run(&args[1..]),
        Some("exec") => cmd_exec(&args[1..]),
        Some("gpu") => cmd_gpu(&args[1..]),
        Some("commands") => {
            use std::io::Write as _;
            let mut s = Session::default();
            s.execute("engine.commands", &json!({})).map_err(|e| e.to_string()).and_then(|v| {
                // A pipeline consumer may stop early (e.g. `commands | head`).
                // Unlike println!, writing explicitly lets us handle that without a panic.
                match writeln!(std::io::stdout().lock(), "{}", pretty(&v)) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                    Err(e) => Err(format!("stdout: {e}")),
                }
            })
        }
        Some("recipe") => args.get(1).ok_or_else(|| USAGE.to_string()).and_then(|p| {
            let v = read_json(p)?;
            recipe::to_script(&v).map(|s| println!("{}", pretty(&s)))
        }),
        Some("oracle") => oracle::run(&args[1..]),
        Some("fuzz") => fuzz::run(&args[1..]),
        Some("fuzz-one") => fuzz::one(&args[1..]),
        Some("mcp") => cmd_mcp(&args[1..]),
        Some("crashes") => crashes::run(&args[1..]),
        Some("icon") => cmd_icon(&args[1..]),
        Some("licences" | "licenses") => {
            // Written, not print!ed: a closed pipe (`| head`) is not a crash.
            use std::io::Write as _;
            let _ = std::io::stdout().lock().write_all(solidcraft_engine::licences::all().as_bytes());
            Ok(())
        }
        // Hidden, for the kill-mid-save test: save a large design over and over.
        Some("save-stress") => cmd_save_stress(&args[1..]),
        // Hidden: time recomputes after parameter edits (docs/perf.md).
        Some("bench-edit") => cmd_bench_edit(&args[1..]),
        Some("bench-open") => cmd_bench_open(&args[1..]),
        // A ~1,300-body design built command by command, as an agent builds one over MCP.
        Some("bench-agent") => bench_agent::run(&args[1..]),
        Some("--version" | "-V") => {
            println!("solidcraft-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

pub fn read_json(path: &str) -> Result<Value, String> {
    // Through the engine's file access, which refuses anything that is not a regular file and
    // reads no more than the cap: a character device or a FIFO would otherwise never end.
    let s = solidcraft_engine::io::vfs::read_to_string(path, solidcraft_engine::io::MAX_DESIGN_BYTES).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("{path}: {e}"))
}

/// A session from a design file, a STEP/3MF/STL file, a command script, or an oracle recipe.
pub fn load(path: &str) -> Result<Session, String> {
    let mut s = Session::default();
    if solidcraft_engine::io::is_step_path(path) || solidcraft_engine::io::is_mesh_path(path) {
        s.execute("doc.open", &json!({"path": path})).map_err(|e| e.to_string())?;
        return Ok(s);
    }
    let bytes = solidcraft_engine::io::vfs::read(path, solidcraft_engine::io::MAX_DESIGN_BYTES).map_err(|e| format!("{path}: {e}"))?;
    if solidcraft_engine::io::is_design_container(&bytes) {
        let doc = solidcraft_engine::io::read_design(&bytes).map_err(|e| format!("{path}: {e}"))?;
        let mut session = Session::new(doc);
        session.path = Some(path.to_owned());
        return Ok(session);
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{path}: {e}"))?;
    if v.get("format").and_then(Value::as_str).is_some_and(|f| solidcraft_engine::doc::format::version_of(f).is_some()) {
        s.execute("doc.open", &json!({"path": path})).map_err(|e| e.to_string())?;
    } else if v.get("features").is_some() && v.get("commands").is_none() {
        let script = recipe::to_script(&v)?;
        s.run_script(&script).map_err(|e| e.to_string())?;
    } else {
        s.run_script(&v).map_err(|e| e.to_string())?;
    }
    Ok(s)
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn flags<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.windows(2).filter(|w| w[0] == name).map(|w| w[1].as_str()).collect()
}

/// The first argument that is not an option or an option's value.
fn positional(args: &[String]) -> Option<&String> {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        if a.starts_with("--") {
            i += if matches!(a.as_str(), "--quiet" | "--results" | "--timings" | "--profile" | "--allow-errors" | "--no-grid" | "--no-axes") {
                1
            } else {
                2
            };
            continue;
        }
        return Some(a);
    }
    None
}

/// `--json-out FILE`: where the JSON a command prints goes instead of stdout.
fn json_out(args: &[String]) -> Result<Option<&str>, String> {
    if !args.iter().any(|a| a == "--json-out") {
        return Ok(None);
    }
    match flag(args, "--json-out") {
        Some(file) if !file.starts_with("--") => Ok(Some(file)),
        _ => Err("--json-out needs a FILE".to_string()),
    }
}

/// `v` as indented JSON with a final newline: on stdout, or in `file`. The file is plain UTF-8
/// with no byte-order mark, whatever the platform's shell does to redirected output (Windows
/// PowerShell's `>` writes UTF-16, or UTF-8 with a BOM), so strict JSON readers take it.
fn print_json(v: &Value, file: Option<&str>) -> Result<(), String> {
    match file {
        Some(file) => {
            let mut text = pretty(v);
            text.push('\n');
            std::fs::write(file, text).map_err(|e| format!("{file}: {e}"))
        }
        None => {
            println!("{}", pretty(v));
            Ok(())
        }
    }
}

fn cmd_run(args: &[String]) -> Result<(), String> {
    install_renderer();
    let json_out = json_out(args)?;
    let base = flag(args, "--in");
    let path = positional(args);
    if path.is_none() && base.is_none() {
        return Err(USAGE.to_string());
    }
    let want_results = args.iter().any(|a| a == "--results");
    // `--profile` times each command and splits out its kernel time; it implies `--timings`.
    let profile = args.iter().any(|a| a == "--profile");
    let timings = profile || args.iter().any(|a| a == "--timings");
    let (mut s, results) = if want_results || timings {
        // Collect every command's result or time (scripts only), then process requested files too.
        let mut s = match base {
            Some(b) => load(b)?,
            None => Session::default(),
        };
        let needs = if want_results { "--results needs a script" } else { "--timings needs a script" };
        let v = read_json(path.ok_or(needs)?)?;
        let out = if timings {
            let (r, mut t) = s.run_script_timed(&v, profile).map_err(|e| e.to_string())?;
            if want_results && let Some(o) = t.as_object_mut() {
                o.insert("results".into(), Value::Array(r));
            }
            t
        } else {
            Value::Array(s.run_script(&v).map_err(|e| e.to_string())?)
        };
        (s, Some(out))
    } else {
        (open(base, path)?, None)
    };
    // A feature that fails leaves the bodies as they were before it (a parameter change can
    // break a feature without failing the command that made it): writing them would hand out
    // the wrong part with success, so fail the run unless asked to write it anyway.
    if !args.iter().any(|a| a == "--allow-errors") {
        timeline_ok(&s)?;
    }
    for out in flags(args, "--out") {
        s.execute("file.export", &json!({"path": out})).map_err(|e| e.to_string())?;
    }
    if let Some(p) = flag(args, "--save") {
        s.execute("file.save_as", &json!({"path": p})).map_err(|e| e.to_string())?;
    }
    if let Some(results) = results {
        print_json(&results, json_out)?;
    } else if json_out.is_some() || !args.iter().any(|a| a == "--quiet") {
        // `--quiet` silences stdout; a file that was asked for is still written.
        let m = s.execute("inspect.measure", &json!({})).map_err(|e| e.to_string())?;
        print_json(&m, json_out)?;
    }
    Ok(())
}

/// The session `run` and `snapshot` start from: `--in` (a design or STEP file) with the
/// positional script run on top of it, or either one alone.
fn open(base: Option<&str>, path: Option<&String>) -> Result<Session, String> {
    match (base, path) {
        (Some(b), Some(p)) => {
            let mut s = load(b)?;
            s.run_script(&read_json(p)?).map_err(|e| e.to_string())?;
            Ok(s)
        }
        (Some(b), None) => load(b),
        (None, Some(p)) => load(p),
        (None, None) => Err(USAGE.to_string()),
    }
}

/// Refuses an option `cmd` doesn't take, and one missing its value: a typo, or another
/// command's option, would otherwise be ignored without a word.
fn known_flags(cmd: &str, args: &[String], valued: &[&str], switches: &[&str]) -> Result<(), String> {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        if valued.contains(&a.as_str()) {
            match args.get(i + 1) {
                Some(v) if !v.starts_with("--") => i += 2,
                _ => return Err(format!("{cmd}: {a} needs a value")),
            }
        } else if a.starts_with("--") && !switches.contains(&a.as_str()) {
            return Err(format!("{cmd} doesn't take {a} (it takes {})", [valued, switches].concat().join(", ")));
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// Every failing timeline feature, one per line, as an error; Ok when none fails.
fn timeline_ok(s: &Session) -> Result<(), String> {
    let failed: Vec<String> = s.model.results.iter().filter_map(|r| r.error.as_ref().map(|e| format!("  {}: {e}", r.name))).collect();
    if failed.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} timeline feature(s) failed, so nothing was written (--allow-errors writes the bodies anyway):\n{}",
        failed.len(),
        failed.join("\n")
    ))
}

fn cmd_eval(args: &[String]) -> Result<(), String> {
    let json_out = json_out(args)?;
    let path = flag(args, "--in").or_else(|| positional(args).map(String::as_str)).ok_or_else(|| USAGE.to_string())?;
    let mut s = load(path)?;
    let m = s.execute("inspect.measure", &json!({})).map_err(|e| e.to_string())?;
    let d = s.execute("document.inspect", &json!({})).map_err(|e| e.to_string())?;
    let out = json!({
        "body_count": m["body_count"],
        "bodies": m["bodies"],
        "total": m["total"],
        "timeline": d["timeline"],
        "params": d["params"],
        "sketches": d["sketches"],
    });
    print_json(&out, json_out)
}

/// A design's history as a command script (`timeline.export_script`): printed, or written to
/// `--out`; what the script leaves out goes to stderr.
fn cmd_script(args: &[String]) -> Result<(), String> {
    let path = positional(args).ok_or_else(|| USAGE.to_string())?;
    let s = &mut load(path)?;
    let out = flag(args, "--out");
    let r = s.execute("timeline.export_script", &json!({"path": out})).map_err(|e| e.to_string())?;
    for what in r["omitted"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        eprintln!("note: the script leaves out {what}");
    }
    match out {
        Some(out) => eprintln!("{out}: {} steps", r["steps"]),
        None => println!("{}", pretty(&r["script"])),
    }
    Ok(())
}

/// `gpu`: the headless adapter survey and the policy's pick.
fn cmd_gpu(args: &[String]) -> Result<(), String> {
    use solidcraft_gpu::select::{Policy, survey};
    let policy = match args {
        [] => Policy::from_env()?,
        [flag, v] if flag == "--policy" => Policy::parse(v).ok_or_else(|| format!("--policy must be auto, hardware, software or cpu, not `{v}`"))?,
        _ => return Err("usage: solidcraft-cli gpu [--policy auto|hardware|software|cpu]".into()),
    };
    let s = survey(policy);
    let chosen = s.chosen.and_then(|i| s.adapters.get(i)).cloned();
    // The tier pictures land on: a hardware GPU, a software adapter, or the CPU rasterizer.
    let renderer = s.lands_on.map(|l| l.renderer());
    println!(
        "{}",
        pretty(&json!({
            "policy": policy.name(),
            "backends": s.backends,
            "adapters": s.adapters,
            "chosen": chosen,
            "lands_on": s.lands_on,
            "renderer": renderer,
        }))
    );
    match s.lands_on {
        Some(solidcraft_gpu::select::Landing::Cpu) => {
            eprintln!("warning: {}", solidcraft_engine::capture::CPU_WARNING);
            Ok(())
        }
        Some(_) => Ok(()),
        None => Err(format!("no adapter qualifies under the {} policy", policy.name())),
    }
}

/// Draw this process's pictures (snapshots, MCP screenshots, saved thumbnails) on the GPU with the
/// viewport's pipelines: the headless device the `SOLIDCRAFT_GPU` policy picks, opened on the first
/// picture; under `auto` with no qualifying adapter (or `cpu`), the CPU rasterizer, and the
/// picture says so. Under `hardware` or `software` with no such adapter a picture fails with an
/// error that names the policy.
fn install_renderer() {
    solidcraft_engine::capture::install(std::sync::Arc::new(|scene, cam, w, h| {
        let (rgba, renderer) = solidcraft_gpu::capture::headless_rgba(scene, cam, w, h)?;
        Ok(solidcraft_engine::capture::Picture { rgba, renderer })
    }));
}

fn cmd_snapshot(args: &[String]) -> Result<(), String> {
    install_renderer();
    known_flags("snapshot", args, &["--in", "--out", "--width", "--height", "--view", "--camera"], &["--no-grid", "--no-axes"])?;
    if positional(args).is_none() && flag(args, "--in").is_none() {
        return Err(USAGE.to_string());
    }
    let out = flag(args, "--out").ok_or("snapshot needs --out FILE.png")?;
    let size = |name: &str, default: u32| match flag(args, name) {
        None => Ok(default),
        Some(x) => x.parse().ok().filter(|n| (16..=8192).contains(n)).ok_or_else(|| format!("{name} must be 16 to 8192 pixels, not `{x}`")),
    };
    let (w, h) = (size("--width", 1280)?, size("--height", 800)?);
    let mut s = open(flag(args, "--in"), positional(args))?;
    // The camera choices of the MCP `screenshot`, through the same command (`view.apply`): a
    // standard view, a view saved in the design, or an explicit camera. A script that applies a
    // view itself sets the camera too; without any, the iso view framing the model.
    match (flag(args, "--view"), flag(args, "--camera")) {
        (Some(_), Some(_)) => return Err("snapshot: give --view or --camera, not both".into()),
        (Some(v), None) => {
            let hint = "standard views: iso, front, back, top, bottom, left, right";
            s.execute("view.apply", &json!({"view": v})).map_err(|e| format!("--view {v}: {e} ({hint}; or a view saved with view.save)"))?;
        }
        (None, Some(c)) => {
            let camera: Value = serde_json::from_str(c).map_err(|e| format!("--camera: not JSON: {e}"))?;
            s.execute("view.apply", &json!({"camera": camera})).map_err(|e| format!("--camera: {e}"))?;
        }
        (None, None) => {}
    }
    let cam = s.camera.unwrap_or_else(|| solidcraft_engine::view::home_camera(&s));
    let guides = Guides { grid: !args.iter().any(|a| a == "--no-grid"), axes: !args.iter().any(|a| a == "--no-axes") };
    let scene = solidcraft_engine::view::scene_with_guides(&s, &cam, s.section, guides);
    let png = solidcraft_engine::capture::png(&scene, &cam, w, h)?;
    std::fs::write(out, &png.bytes).map_err(|e| format!("{out}: {e}"))?;
    let mut reply = json!({"path": out, "width": w, "height": h, "bytes": png.bytes.len(), "renderer": png.renderer});
    if let Some(w) = solidcraft_engine::capture::warning(png.renderer) {
        eprintln!("warning: {w}");
        reply["warning"] = json!(w);
    }
    println!("{}", pretty(&reply));
    Ok(())
}

/// The app icon: the SolidCraft logo, drawn in code on a transparent background
/// (so packages need no committed image). `.ico` holds the Windows sizes as PNGs.
fn cmd_icon(args: &[String]) -> Result<(), String> {
    let out = flag(args, "--out").ok_or("icon needs --out FILE.png or FILE.ico")?;
    let size: usize = flag(args, "--size").and_then(|x| x.parse().ok()).unwrap_or(256).clamp(8, 2048);
    let bytes = if out.to_ascii_lowercase().ends_with(".ico") {
        let pngs: Vec<(usize, Vec<u8>)> =
            [16, 20, 24, 32, 40, 48, 64, 128, 256].into_iter().map(|s| icon_png(s).map(|p| (s, p))).collect::<Result<_, _>>()?;
        ico(&pngs)?
    } else {
        icon_png(size)?
    };
    std::fs::write(out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    println!("{}", pretty(&json!({"path": out, "bytes": bytes.len()})));
    Ok(())
}

fn icon_png(size: usize) -> Result<Vec<u8>, String> {
    solidcraft_engine::render::png(size, size, app_icon::rgba(size)?).ok_or_else(|| "icon: PNG encoding failed".to_string())
}

/// A Windows icon file holding PNG images (Vista and later read these).
fn ico(images: &[(usize, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let n = u16::try_from(images.len()).map_err(|_| "too many icon sizes")?;
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&n.to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, data) in images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&u32::try_from(data.len()).map_err(|_| "icon too large")?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset).map_err(|_| "icon too large")?.to_le_bytes());
        offset += data.len();
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    Ok(out)
}

fn cmd_exec(args: &[String]) -> Result<(), String> {
    install_renderer();
    let id = args.first().ok_or_else(|| USAGE.to_string())?;
    let p: Value = match args.get(1) {
        Some(t) => serde_json::from_str(t).map_err(|e| format!("params: {e}"))?,
        None => json!({}),
    };
    let mut s = Session::default();
    let v = s.execute(id, &p).map_err(|e| e.to_string())?;
    println!("{}", pretty(&v));
    Ok(())
}

/// MCP server on stdio: headless (optionally starting from a design or script) or bridged to
/// a running app's control port. stdout carries only protocol messages. Files the agent's tool
/// calls reach are confined to `solidcraft_mcp::agent_scope` unless `--allow-any-path` is given.
fn cmd_mcp(args: &[String]) -> Result<(), String> {
    install_renderer();
    let scope = (!args.iter().any(|a| a == "--allow-any-path"))
        .then(|| solidcraft_mcp::agent_scope(flags(args, "--allow-path").into_iter().map(std::path::PathBuf::from)));
    if let Some(s) = &scope {
        let roots: Vec<String> = s.roots().iter().map(|r| r.display().to_string()).collect();
        eprintln!(
            "solidcraft mcp: files limited to {} and the open design's folder (--allow-path DIR adds one, --allow-any-path lifts the limit)",
            roots.join(", ")
        );
        let design_dir = flag(args, "--in")
            .and_then(|p| std::path::Path::new(p).parent())
            .map(|d| if d.as_os_str().is_empty() { std::path::Path::new(".") } else { d });
        if let Some(why) = design_dir.and_then(solidcraft_engine::io::vfs::design_folder_refusal) {
            eprintln!("solidcraft mcp: the design's folder is left out because {why}; add it with --allow-path if you mean it");
        }
    }
    let backend: Box<dyn solidcraft_mcp::Backend> = match (flag(args, "--connect"), flag(args, "--in")) {
        (Some(_), Some(_)) => return Err("mcp: use either --connect or --in, not both".into()),
        (Some(addr), None) => {
            let Some(port) = addr.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).filter(|p| *p != 0) else {
                return Err(format!("mcp: --connect wants HOST:PORT, such as 127.0.0.1:7878, not `{addr}`"));
            };
            // Serve without the app too: the tools connect once it runs, and say it isn't until then.
            let mut remote = solidcraft_mcp::Remote::new(addr, flag(args, "--token"));
            if let Err(e) = remote.try_connect() {
                eprintln!(
                    "solidcraft mcp: no SolidCraft app at {addr} yet ({e}); start it with `solidcraft --control {port}` \
                     (or SOLIDCRAFT_CONTROL_PORT={port}), tool calls connect once it runs"
                );
            }
            Box::new(remote)
        }
        (None, Some(path)) => Box::new(solidcraft_mcp::Headless::new(load(path)?)),
        (None, None) => Box::new(solidcraft_mcp::Headless::default()),
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    solidcraft_mcp::Server::new(backend).with_path_scope(scope).serve(stdin.lock(), stdout.lock()).map_err(|e| e.to_string())
}

/// `save-stress <path> <rounds>`: save a design of a few hundred kilobytes `rounds` times, its
/// parameter `save_round` set to the round number each time.
fn cmd_save_stress(args: &[String]) -> Result<(), String> {
    let (Some(path), Some(rounds)) = (args.first(), args.get(1).and_then(|n| n.parse::<u64>().ok())) else {
        return Err("usage: solidcraft-cli save-stress <path> <rounds>".into());
    };
    let mut s = Session::default();
    let filler = (0..3000).map(|i| solidcraft_engine::doc::Param {
        name: format!("p{i}"),
        expr: format!("{i} mm + 0.5 mm"),
        unit: "mm".into(),
        comment: "filler to make the file large".into(),
        model: false,
    });
    s.doc_mut().params.extend(filler);
    for r in 0..rounds {
        s.doc_mut().set_param("save_round", &r.to_string(), None, None).map_err(|e| e.to_string())?;
        s.execute("file.save", &json!({ "path": path })).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `bench-edit <script.json|recipe.json> <param> <expr>`: build the design, then time a
/// parameter edit, setting it back, undo and redo (ms and features recomputed each).
fn cmd_bench_edit(args: &[String]) -> Result<(), String> {
    let (Some(path), Some(param), Some(expr)) = (args.first(), args.get(1), args.get(2)) else {
        return Err("usage: solidcraft-cli bench-edit <script.json|recipe.json> <param> <expr>".into());
    };
    let v = read_json(path)?;
    let script = if v.get("features").is_some() { recipe::to_script(&v)? } else { v };
    let mut s = Session::default();
    let t = std::time::Instant::now();
    s.run_script(&script).map_err(|e| e.to_string())?;
    let build = t.elapsed().as_secs_f64() * 1000.0;
    let old = s.doc.all_param_exprs().into_iter().find(|(n, _, _)| n == param).map(|(_, e, _)| e).ok_or(format!("no parameter `{param}`"))?;
    let mut out = vec![json!({"step": "build", "ms": build, "features": s.doc.features.len()})];
    let mut time = |s: &mut Session, step: &str, id: &str, p: Value| -> Result<(), String> {
        let t = std::time::Instant::now();
        let r = s.execute(id, &p).map_err(|e| format!("{step}: {e}"))?;
        let n = r.get("recomputed").cloned().unwrap_or_else(|| json!(s.model.last_recomputed));
        out.push(json!({"step": step, "ms": t.elapsed().as_secs_f64() * 1000.0, "recomputed": n}));
        Ok(())
    };
    let expr = expr.replace("{old}", &old);
    time(&mut s, "edit", "parameters.change", json!({"name": param, "expression": expr}))?;
    time(&mut s, "set back", "parameters.change", json!({"name": param, "expression": old}))?;
    time(&mut s, "undo", "edit.undo", json!({}))?;
    time(&mut s, "redo", "edit.redo", json!({}))?;
    println!("{}", pretty(&json!(out)));
    Ok(())
}

/// Separate file reading, JSON decoding/repair, timeline evaluation and display meshing.
pub(crate) fn cmd_bench_open(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("usage: solidcraft-cli bench-open <design.solidcraft | part.step> [--parse-only]")?;
    let t = std::time::Instant::now();
    let step = solidcraft_engine::io::is_step_path(path);
    let limit = if step { solidcraft_engine::io::MAX_STEP_BYTES } else { solidcraft_engine::io::MAX_DESIGN_BYTES };
    let bytes = solidcraft_engine::io::vfs::read(path, limit).map_err(|e| format!("{path}: {e}"))?;
    let read_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("{}", json!({"step": "read", "ms": read_ms, "bytes": bytes.len()}));
    let t = std::time::Instant::now();
    // The file as read, held until the timeline has evaluated it (as the import command does).
    let mut read = None;
    let doc = if step {
        let imported = solidcraft_engine::io::step_import_feature(&bytes, path).map_err(|e| e.to_string())?;
        let mut doc = solidcraft_engine::doc::Document::new(&imported.name);
        doc.add_feature(imported.kind, Some(&imported.name)).map_err(|e| e.to_string())?;
        read = Some(imported.import);
        doc
    } else {
        solidcraft_engine::io::read_design(&bytes).map_err(|e| e.to_string())?
    };
    let parse_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("{}", json!({"step": "parse", "ms": parse_ms, "features": doc.features.len()}));
    if args.iter().any(|a| a == "--parse-only") {
        return Ok(());
    }
    let t = std::time::Instant::now();
    let s = Session::new(doc);
    drop(read);
    println!("{}", json!({"step": "evaluate", "ms": t.elapsed().as_secs_f64() * 1000.0}));
    for r in &s.model.results {
        println!("{}", json!({"step": "feature", "id": r.id, "name": r.name, "ms": r.ms, "error": r.error}));
    }
    let t = std::time::Instant::now();
    let st = s.world_state();
    // As the desktop app prepares a design it opened.
    st.warm_meshes(|| true);
    let triangles: usize = st.bodies.iter().map(|b| b.mesh().triangles.len()).sum();
    println!("{}", json!({"step": "mesh", "ms": t.elapsed().as_secs_f64() * 1000.0, "bodies": st.bodies.len(), "triangles": triangles}));
    // `--display LEVEL`: the display plans the viewport draws at that display quality, made as
    // the GPU viewport makes them; `--edit NAME EXPR` then changes a parameter and times the
    // evaluation and the plans of the bodies it changed.
    let flag = |name: &str| args.iter().position(|a| a == name);
    let Some(level) = flag("--display").and_then(|i| args.get(i + 1)) else { return Ok(()) };
    let q = solidcraft_engine::doc::display_quality::DisplayQuality::parse(level).ok_or("--display: coarse, normal, fine or ultra")?;
    let plans = |st: &solidcraft_engine::doc::ModelState| {
        let t = std::time::Instant::now();
        solidcraft_engine::kernel::for_each_on_threads(&st.bodies, 16, |b| {
            let _ = b.plan_mesh_at(q);
        });
        let triangles: usize = st.bodies.iter().filter_map(|b| b.plan_mesh_at(q)).map(|m| m.triangles.len()).sum();
        (t.elapsed().as_secs_f64() * 1000.0, triangles)
    };
    let (ms, triangles) = plans(&st);
    println!("{}", json!({"step": "display", "quality": q.id(), "ms": ms, "triangles": triangles}));
    if let Some(i) = flag("--edit") {
        let (Some(name), Some(expr)) = (args.get(i + 1), args.get(i + 2)) else { return Err("--edit NAME EXPR".into()) };
        let mut s = s;
        let t = std::time::Instant::now();
        s.execute("parameters.change", &json!({"name": name, "expression": expr})).map_err(|e| e.to_string())?;
        let eval_ms = t.elapsed().as_secs_f64() * 1000.0;
        let st = s.world_state();
        let t = std::time::Instant::now();
        st.warm_meshes(|| true);
        let mesh_ms = t.elapsed().as_secs_f64() * 1000.0;
        let (ms, triangles) = plans(&st);
        println!(
            "{}",
            json!({"step": "edit", "quality": q.id(), "evaluate_ms": eval_ms, "mesh_ms": mesh_ms, "display_ms": ms, "triangles": triangles})
        );
    }
    Ok(())
}
