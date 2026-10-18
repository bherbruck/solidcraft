//! Which ACP agents are installed here (desktop only). A desktop app often starts without the
//! shell's PATH, so the usual install folders are searched too. Detection never starts a login
//! or a session: each agent found is asked for `--version` (with a time limit), and whether it
//! looks logged in comes only from whether its own credentials file exists.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// An agent SolidCraft knows how to start in ACP mode.
pub struct Known {
    pub id: &'static str,
    pub name: &'static str,
    /// Program names to look for, in order of preference.
    pub programs: &'static [&'static str],
    /// Arguments that put it in ACP mode.
    pub args: &'static [&'static str],
    /// Its one-line install hint.
    pub install: &'static str,
    /// Its own credentials file, relative to the home folder (`None`: can't tell), and the
    /// variable that moves its settings folder (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) with the file's
    /// name inside it.
    pub credentials: Option<&'static str>,
    pub config_dir_var: Option<(&'static str, &'static str)>,
    /// Variables holding a key it uses instead of a login.
    pub key_vars: &'static [&'static str],
    /// Retired by its maker: still used when it is installed, never offered for install.
    pub retired: bool,
    /// Whether `--version` prints its version. Antigravity's server prints its build date and takes
    /// seconds to start, so it is not asked.
    pub reports_version: bool,
}

/// The agents shown in the picker. The first one found is the default.
pub const KNOWN: &[Known] = &[
    Known {
        id: "claude",
        name: "Claude Code",
        programs: &["claude-agent-acp", "claude-code-acp"],
        args: &[],
        install: "npm install -g @agentclientprotocol/claude-agent-acp",
        credentials: Some(".claude/.credentials.json"),
        config_dir_var: Some(("CLAUDE_CONFIG_DIR", ".credentials.json")),
        key_vars: &["ANTHROPIC_API_KEY"],
        retired: false,
        reports_version: true,
    },
    Known {
        id: "codex",
        name: "Codex",
        programs: &["codex-acp"],
        args: &[],
        install: "npm install -g @agentclientprotocol/codex-acp",
        credentials: Some(".codex/auth.json"),
        config_dir_var: Some(("CODEX_HOME", "auth.json")),
        key_vars: &["OPENAI_API_KEY", "CODEX_API_KEY"],
        retired: false,
        reports_version: true,
    },
    // Google's `agy` command line has no ACP mode; its ACP server is a separate download that the
    // ACP registry lists (`agy_acp_server.par`, `agy_acp_server.exe` on Windows).
    Known {
        id: "antigravity",
        name: "Google Antigravity",
        programs: &["agy_acp_server.par", "agy_acp_server"],
        args: &[],
        install: "download the Antigravity ACP server for your system (see agentclientprotocol.com/registry), unzip it and put agy_acp_server.par on your PATH",
        credentials: Some(".gemini/antigravity-acp/acp_token.json"),
        config_dir_var: None,
        key_vars: &["GEMINI_API_KEY"],
        retired: false,
        reports_version: false,
    },
    // Google retired it on 18 June 2026 in favour of Antigravity (#1790). Enterprise licences may
    // still work, so an installed one is found, but nobody is sent to install it.
    Known {
        id: "gemini",
        name: "Gemini CLI",
        programs: &["gemini"],
        args: &["--acp"],
        install: "npm install -g @google/gemini-cli",
        credentials: Some(".gemini/oauth_creds.json"),
        config_dir_var: None,
        key_vars: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        retired: true,
        reports_version: true,
    },
    Known {
        id: "opencode",
        name: "OpenCode",
        programs: &["opencode"],
        args: &["acp"],
        install: "npm install -g opencode-ai",
        credentials: Some(".local/share/opencode/auth.json"),
        config_dir_var: None,
        key_vars: &[],
        retired: false,
        reports_version: true,
    },
    Known {
        id: "grok",
        name: "XAI Grok",
        programs: &["grok"],
        args: &["agent", "stdio"],
        install: "see https://docs.x.ai/build/overview (Grok Build installer); then run grok login",
        credentials: Some(".grok/auth.json"),
        config_dir_var: None,
        key_vars: &["XAI_API_KEY"],
        retired: false,
        reports_version: true,
    },
    Known {
        id: "copilot",
        name: "GitHub Copilot CLI",
        programs: &["copilot"],
        args: &["--acp"],
        install: "npm install -g @github/copilot",
        credentials: None,
        config_dir_var: None,
        key_vars: &["GH_TOKEN", "GITHUB_TOKEN"],
        retired: false,
        reports_version: true,
    },
    Known {
        id: "qwen",
        name: "Qwen Code",
        programs: &["qwen"],
        args: &["--acp"],
        install: "npm install -g @qwen-code/qwen-code",
        credentials: Some(".qwen/oauth_creds.json"),
        config_dir_var: None,
        key_vars: &["DASHSCOPE_API_KEY"],
        retired: false,
        reports_version: true,
    },
    Known {
        id: "goose",
        name: "goose",
        programs: &["goose"],
        args: &["acp"],
        install: "see block.github.io/goose (installer script or Homebrew)",
        credentials: None,
        config_dir_var: None,
        key_vars: &[],
        retired: false,
        reports_version: true,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Login {
    /// Its credentials file is there.
    LooksLoggedIn,
    /// No login file, but one of its key variables is set.
    KeyInEnvironment,
    /// Its credentials file is missing (it may still use a key from the environment).
    NotLoggedIn,
    /// It keeps no file we can look at (system keychain, or unknown).
    Unknown,
}

impl Login {
    pub fn id(self) -> &'static str {
        match self {
            Login::LooksLoggedIn => "signed_in",
            Login::KeyInEnvironment => "key",
            Login::NotLoggedIn => "signed_out",
            Login::Unknown => "unknown",
        }
    }
}

/// A known agent that isn't installed, with its one-line install hint.
#[derive(Clone, Debug)]
pub struct Missing {
    pub id: &'static str,
    pub name: &'static str,
    pub install: &'static str,
}

#[derive(Clone, Debug)]
pub struct Found {
    pub id: &'static str,
    pub name: &'static str,
    /// The full command: the program's path and its ACP arguments.
    pub argv: Vec<String>,
    pub version: Option<String>,
    pub login: Login,
}

#[derive(Clone, Debug, Default)]
pub struct Detection {
    pub found: Vec<Found>,
    /// Known agents not found.
    pub missing: Vec<Missing>,
    pub took_ms: u64,
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).filter(|p| p.is_absolute())
}

/// PATH, then the usual places npm, Homebrew, nvm, Volta, bun, pipx and Grok put commands.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(prefix) = std::env::var_os("NPM_CONFIG_PREFIX").map(PathBuf::from) {
        dirs.push(if cfg!(windows) { prefix.clone() } else { prefix.join("bin") });
    }
    if let Some(h) = home() {
        for d in [".local/bin", ".npm-global/bin", ".npm-packages/bin", ".volta/bin", ".bun/bin", ".cargo/bin", ".grok/bin", "bin"] {
            dirs.push(h.join(d));
        }
        // nvm: every installed Node version's bin.
        if let Ok(rd) = std::fs::read_dir(h.join(".nvm/versions/node")) {
            let mut v: Vec<PathBuf> = rd.flatten().take(64).map(|e| e.path().join("bin")).collect();
            v.sort();
            dirs.extend(v.into_iter().rev());
        }
    }
    if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(a).join("npm"));
        }
    } else {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/home/linuxbrew/.linuxbrew/bin", "/usr/bin"] {
            dirs.push(PathBuf::from(d));
        }
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| d.is_absolute() && seen.insert(d.clone()));
    dirs
}

/// The first `program` in `dirs` (on Windows with `.exe`/`.cmd`/`.bat`).
pub fn find_in(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let names = names_for(program, cfg!(windows));
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// The file names `program` can have. On Windows only ones it can run: npm puts a Unix shell
/// script with the bare name next to the `.cmd` shim, and starting that fails with "not a valid
/// Win32 application".
fn names_for(program: &str, windows: bool) -> Vec<String> {
    if windows { [".exe", ".cmd", ".bat"].iter().map(|e| format!("{program}{e}")).collect() } else { vec![program.to_string()] }
}

/// A Windows command saved without an extension (`…\npm\codex-acp`, from an older version's
/// lookup): the `.exe`/`.cmd`/`.bat` beside it, if there is one.
pub fn runnable_sibling(program: &Path) -> Option<PathBuf> {
    if program.extension().is_some() {
        return None;
    }
    let dir = program.parent()?;
    let name = program.file_name()?.to_str()?;
    names_for(name, true).into_iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// Whether Node.js `major.minor` can run the npm ACP adapters. They load JSON with import
/// attributes (`import … with { type: "json" }`), which Node reads from 18.20, 20.10 and 21 on;
/// an older one stops at once with `SyntaxError: Unexpected token 'with'`.
fn node_runs_adapters((major, minor): (u32, u32)) -> bool {
    match major {
        18 => minor >= 20,
        20 => minor >= 10,
        m => m >= 21,
    }
}

/// `v18.19.1` (what `node --version` prints, and the `Node.js v18.19.1` line Node ends an
/// uncaught error with) as `(18, 19)`.
fn node_version_of(text: &str) -> Option<(u32, u32)> {
    let v = text.trim().strip_prefix("Node.js").unwrap_or(text).trim().strip_prefix('v')?;
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    Some((major, minor))
}

/// Whether `program` is a script that asks `env` for `node` (`#!/usr/bin/env node`, as npm's
/// adapters are), so the PATH decides which Node runs it.
fn runs_on_env_node(program: &Path) -> bool {
    let mut head = [0u8; 256];
    let n = std::fs::File::open(program).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    let first = head.get(..n).unwrap_or_default().split(|&b| b == b'\n').next().unwrap_or_default();
    let Some(line) = std::str::from_utf8(first).ok().and_then(|l| l.strip_prefix("#!")) else { return false };
    let mut words = line.split_whitespace();
    if !words.next().is_some_and(|w| w.ends_with("/env")) {
        return false;
    }
    // `env -S node --flag` and `env node` both name it as the first word that isn't an option.
    words.find(|w| !w.starts_with('-')).is_some_and(|w| w == "node")
}

/// The Node.js version in an agent's last stderr lines, when it is one the adapters can't run on:
/// the panel then says so instead of leaving only Node's own syntax error.
pub(super) fn node_too_old_in(tail: &[String]) -> Option<String> {
    let line = tail.iter().rev().map(|l| l.trim()).find(|l| l.starts_with("Node.js v"))?;
    let v = node_version_of(line)?;
    (!node_runs_adapters(v)).then(|| line.trim_start_matches("Node.js ").to_string())
}

/// `dirs` for starting `program`, with one change: when `program` runs on `env node` and the
/// first `node` in `dirs` is too old for it (Ubuntu's packaged Node ahead of a newer one from
/// nvm, say), the folder of the first `node` that can run it is moved to the front. Otherwise
/// `dirs` is returned as it is. `probe` reads a `node`'s version.
fn dirs_for(program: &Path, mut dirs: Vec<PathBuf>, probe: impl Fn(&Path) -> Option<(u32, u32)>) -> Vec<PathBuf> {
    if !cfg!(unix) || !runs_on_env_node(program) {
        return dirs;
    }
    let nodes: Vec<usize> = dirs.iter().enumerate().filter(|(_, d)| d.join("node").is_file()).map(|(i, _)| i).take(16).collect();
    let Some(&first) = nodes.first() else { return dirs };
    let ok = |i: usize| dirs.get(i).and_then(|d| probe(&d.join("node"))).is_some_and(node_runs_adapters);
    if ok(first) {
        return dirs;
    }
    if let Some(better) = nodes.iter().skip(1).copied().find(|&i| ok(i)) {
        let d = dirs.remove(better);
        dirs.insert(0, d);
    }
    dirs
}

/// `node --version` of the `node` at `path`.
fn node_version(path: &Path) -> Option<(u32, u32)> {
    let mut cmd = Command::new(path);
    cmd.arg("--version");
    first_line(cmd, Duration::from_secs(2)).as_deref().and_then(node_version_of)
}

/// Start probes and sessions with the same PATH used for discovery. Desktop launchers often
/// omit Node's install folder even when we found an adapter with a `#!/usr/bin/env node` shebang.
/// Only the child's environment changes; inherited absolute PATH entries keep precedence, except
/// that a Node too old to run the adapter gives way to a newer one found later ([`dirs_for`]).
pub(super) fn command_for(program: &Path) -> Command {
    // Older saved Windows commands may name npm's bare Unix script rather than its .cmd shim.
    let shim = if cfg!(windows) { runnable_sibling(program) } else { None };
    let program = shim.as_deref().unwrap_or(program);
    let mut cmd = if cfg!(windows) && program.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat")) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    };
    if let Ok(path) = std::env::join_paths(dirs_for(program, search_dirs(), node_version)) {
        cmd.env("PATH", path);
    }
    cmd
}

/// `program --version`'s first line, or `None` after `limit` (the probe is then stopped).
fn version(program: &Path, limit: Duration) -> Option<String> {
    let mut cmd = command_for(program);
    cmd.arg("--version");
    first_line(cmd, limit).map(|line| version_of(&line))
}

/// The first non-empty line `cmd` prints, or `None` after `limit` (it is then stopped).
fn first_line(mut cmd: Command, limit: Duration) -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: probing a console agent doesn't flash a terminal window.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    out.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// The version number in a `--version` line (`codex-cli 0.162.1`, `2.1.295 (Claude Code)`,
/// `@scope/pkg 2.2.2`), else the line itself, cut short.
fn version_of(line: &str) -> String {
    let v = line.split_whitespace().find(|w| w.trim_start_matches('v').starts_with(|c: char| c.is_ascii_digit())).unwrap_or(line);
    v.chars().take(40).collect()
}

/// Whether `k` looks signed in, from files and variables only: nothing is started or asked.
fn login_of(k: &Known, home: Option<&Path>) -> Login {
    let file = match k.config_dir_var.and_then(|(var, name)| std::env::var_os(var).filter(|v| !v.is_empty()).map(|d| PathBuf::from(d).join(name))) {
        Some(p) => Some(p),
        None => k.credentials.zip(home).map(|(rel, h)| h.join(rel)),
    };
    let key = || k.key_vars.iter().any(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()));
    match file {
        Some(f) if f.is_file() => Login::LooksLoggedIn,
        _ if key() => Login::KeyInEnvironment,
        // Claude Code keeps its login in the keychain on macOS: the file says nothing there.
        _ if cfg!(target_os = "macos") && k.id == "claude" => Login::Unknown,
        Some(_) => Login::NotLoggedIn,
        None => Login::Unknown,
    }
}

/// Look for every known agent. Blocking (each probe has a time limit): run it off the UI thread.
pub fn detect() -> Detection {
    detect_in(&search_dirs(), home().as_deref())
}

fn detect_in(dirs: &[PathBuf], home: Option<&Path>) -> Detection {
    let t0 = Instant::now();
    let mut d = Detection::default();
    for k in KNOWN {
        let Some(path) = k.programs.iter().find_map(|p| find_in(p, dirs)) else {
            if !k.retired {
                d.missing.push(Missing { id: k.id, name: k.name, install: k.install });
            }
            continue;
        };
        let login = login_of(k, home);
        let mut argv = vec![path.to_string_lossy().into_owned()];
        argv.extend(k.args.iter().map(|a| a.to_string()));
        let version = if k.reports_version { version(&path, Duration::from_secs(3)) } else { None };
        d.found.push(Found { id: k.id, name: k.name, version, argv, login });
    }
    d.took_ms = u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_found_in_the_first_folder_that_has_it() {
        let root = std::env::temp_dir().join(format!("sc-agent-find-{}", std::process::id()));
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        // A name this platform runs: Windows looks only for `.exe`/`.cmd`/`.bat` (#1554).
        let name = names_for("codex-acp", cfg!(windows)).remove(0);
        std::fs::write(b.join(&name), "").unwrap();
        assert_eq!(find_in("codex-acp", &[a.clone(), b.clone()]), Some(b.join(name)));
        assert_eq!(find_in("gemini", &[a, b]), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn on_windows_only_runnable_shims_are_candidates() {
        // npm's bare `codex-acp` is a shell script; Windows can't start it (os error 193).
        assert_eq!(names_for("codex-acp", true), ["codex-acp.exe", "codex-acp.cmd", "codex-acp.bat"]);
        assert_eq!(names_for("codex-acp", false), ["codex-acp"]);
    }

    #[test]
    fn a_saved_bare_npm_path_runs_its_cmd_shim() {
        let root = std::env::temp_dir().join(format!("sc-agent-shim-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("codex-acp"), "#!/bin/sh").unwrap();
        std::fs::write(root.join("codex-acp.cmd"), "@echo off").unwrap();
        assert_eq!(runnable_sibling(&root.join("codex-acp")), Some(root.join("codex-acp.cmd")));
        assert_eq!(runnable_sibling(&root.join("codex-acp.cmd")), None);
        assert_eq!(runnable_sibling(&root.join("gemini")), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_version_number_is_picked_out_of_the_line() {
        assert_eq!(version_of("@agentclientprotocol/codex-acp 2.2.2"), "2.2.2");
        assert_eq!(version_of("2.1.295 (Claude Code)"), "2.1.295");
        assert_eq!(version_of("goose v1.9.0"), "v1.9.0");
        assert_eq!(version_of("grok 1.0.58 (26283d10dd58) [alpha]"), "1.0.58");
        assert_eq!(version_of("dev build"), "dev build");
    }

    #[test]
    fn node_versions_that_read_import_attributes() {
        assert_eq!(node_version_of("v18.19.1"), Some((18, 19)));
        assert_eq!(node_version_of("Node.js v22.11.0"), Some((22, 11)));
        assert_eq!(node_version_of("v20.10.0-nightly"), Some((20, 10)));
        assert_eq!(node_version_of("node: not found"), None);
        // Ubuntu 24.04's packaged Node (18.19) stops at `with`; 18.20, 20.10 and 21 on read it.
        for (v, ok) in [((18, 19), false), ((18, 20), true), ((19, 9), false), ((20, 9), false), ((20, 10), true), ((21, 0), true), ((24, 1), true)] {
            assert_eq!(node_runs_adapters(v), ok, "{v:?}");
        }
    }

    #[test]
    fn an_agents_stderr_tells_when_its_node_is_too_old() {
        let tail = |last: &str| {
            ["SyntaxError: Unexpected token 'with'", "    at ModuleLoader.moduleStrategy (node:internal/modules/esm/translators:152:18)", "", last]
                .map(String::from)
        };
        assert_eq!(node_too_old_in(&tail("Node.js v18.19.1")).as_deref(), Some("v18.19.1"));
        assert_eq!(node_too_old_in(&tail("Node.js v22.11.0")), None, "a new enough Node failed for another reason");
        assert_eq!(node_too_old_in(&tail("error: not signed in")), None);
        assert_eq!(node_too_old_in(&[]), None);
    }

    /// Ubuntu's packaged Node 18.19 in /usr/bin comes before nvm's folders in the desktop PATH,
    /// and an adapter with `#!/usr/bin/env node` would run on it and stop at its first import.
    /// The first `node` that can run it is moved to the front; nothing else changes.
    #[test]
    #[cfg(unix)]
    fn an_env_node_adapter_skips_a_node_too_old_for_it() {
        let root = std::env::temp_dir().join(format!("sc-agent-node-{}", std::process::id()));
        let (old, other, new, bin) = (root.join("usr-bin"), root.join("other"), root.join("nvm-22"), root.join("adapters"));
        for d in [&old, &other, &new, &bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(old.join("node"), "18").unwrap();
        std::fs::write(new.join("node"), "22").unwrap();
        let adapter = bin.join("claude-agent-acp");
        std::fs::write(&adapter, "#!/usr/bin/env node\nimport pkg from './package.json' with { type: 'json' };\n").unwrap();
        let native = bin.join("goose");
        std::fs::write(&native, "\x7fELF").unwrap();
        // The fake `node`s hold their major version; nothing is started.
        let probe = |p: &Path| std::fs::read_to_string(p).ok().and_then(|s| s.parse().ok()).map(|m| (m, if m == 18 { 19 } else { 11 }));
        let dirs = vec![old.clone(), other.clone(), new.clone()];

        assert_eq!(dirs_for(&adapter, dirs.clone(), probe), [new.clone(), old.clone(), other.clone()]);
        // A new enough first Node, a program that isn't a Node script, or no better Node: unchanged.
        assert_eq!(dirs_for(&adapter, vec![new.clone(), old.clone()], probe), [new.clone(), old.clone()]);
        assert_eq!(dirs_for(&native, dirs.clone(), probe), dirs);
        assert_eq!(dirs_for(&adapter, vec![old.clone(), other.clone()], probe), [old.clone(), other.clone()]);

        std::fs::write(&adapter, "#!/usr/bin/env -S node --no-warnings\n").unwrap();
        assert!(runs_on_env_node(&adapter));
        std::fs::write(&adapter, "#!/bin/sh\nexec node \"$0\"\n").unwrap();
        assert!(!runs_on_env_node(&adapter), "a shell script picks its Node itself");
        assert!(!runs_on_env_node(&root.join("missing")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn signed_in_comes_from_the_agents_own_file_or_a_key_variable() {
        let home = std::env::temp_dir().join(format!("sc-agent-login-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        let gemini = KNOWN.iter().find(|k| k.id == "gemini").unwrap();
        let qwen = KNOWN.iter().find(|k| k.id == "qwen").unwrap();
        let goose = KNOWN.iter().find(|k| k.id == "goose").unwrap();
        let grok = KNOWN.iter().find(|k| k.id == "grok").unwrap();
        if std::env::var_os("GEMINI_API_KEY").is_none() && std::env::var_os("GOOGLE_API_KEY").is_none() {
            assert_eq!(login_of(gemini, Some(&home)), Login::NotLoggedIn);
        }
        std::fs::write(home.join(".gemini/oauth_creds.json"), "{}").unwrap();
        assert_eq!(login_of(gemini, Some(&home)), Login::LooksLoggedIn);
        if std::env::var_os("DASHSCOPE_API_KEY").is_none() {
            assert_eq!(login_of(qwen, Some(&home)), Login::NotLoggedIn);
        }
        assert_eq!(login_of(goose, Some(&home)), Login::Unknown, "keeps no file we know");
        if std::env::var_os("XAI_API_KEY").is_none() {
            assert_eq!(login_of(grok, Some(&home)), Login::NotLoggedIn);
        }
        // Antigravity's server keeps its own login, apart from the retired Gemini CLI's.
        let agy = KNOWN.iter().find(|k| k.id == "antigravity").unwrap();
        if std::env::var_os("GEMINI_API_KEY").is_none() {
            assert_eq!(login_of(agy, Some(&home)), Login::NotLoggedIn, "the Gemini CLI's file is not its login");
        }
        std::fs::create_dir_all(home.join(".gemini/antigravity-acp")).unwrap();
        std::fs::write(home.join(".gemini/antigravity-acp/acp_token.json"), "not credential data").unwrap();
        assert_eq!(login_of(agy, Some(&home)), Login::LooksLoggedIn);
        std::fs::create_dir_all(home.join(".grok")).unwrap();
        // Discovery checks only for the file; it never needs to parse the user's tokens.
        std::fs::write(home.join(".grok/auth.json"), "not credential data").unwrap();
        assert_eq!(login_of(grok, Some(&home)), Login::LooksLoggedIn);
        let _ = std::fs::remove_dir_all(home);
    }

    /// Google retired the Gemini CLI (#1790): one that is installed is still found, but nobody is
    /// told to install it. Antigravity's ACP server is offered, and found under its file name.
    #[test]
    #[cfg(unix)]
    fn a_retired_agent_is_found_but_not_offered_and_antigravity_is() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("sc-agent-retired-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let run = |name: &str, body: &str| {
            let path = root.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let ids = |d: &Detection| (d.found.iter().map(|f| f.id).collect::<Vec<_>>(), d.missing.iter().map(|m| m.id).collect::<Vec<_>>());

        let none = detect_in(std::slice::from_ref(&root), None);
        let (found, missing) = ids(&none);
        assert!(found.is_empty());
        assert!(missing.contains(&"antigravity") && !missing.contains(&"gemini"), "{missing:?}");
        let hint = none.missing.iter().find(|m| m.id == "antigravity").map(|m| m.install).unwrap_or_default();
        assert!(hint.contains("agy_acp_server.par") && !hint.contains("npm"), "{hint}");

        run("gemini", "echo gemini 0.64.0");
        // A server that would take seconds to answer `--version`, which Antigravity is never asked.
        run("agy_acp_server.par", "sleep 30");
        let both = detect_in(std::slice::from_ref(&root), None);
        let (found, missing) = ids(&both);
        assert_eq!(found, ["antigravity", "gemini"]);
        assert!(!missing.contains(&"antigravity") && !missing.contains(&"gemini"), "{missing:?}");
        let agy = both.found.iter().find(|f| f.id == "antigravity");
        assert_eq!(agy.map(|f| f.version.clone()), Some(None), "its --version is a build date, not asked");
        assert_eq!(agy.and_then(|f| f.argv.get(1)), None, "the registry starts it without arguments");
        assert!(both.took_ms < 5000, "Antigravity's server was started to read a version");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_known_agent_has_a_program_and_an_install_hint() {
        for k in KNOWN {
            assert!(!k.programs.is_empty() && !k.install.is_empty(), "{}", k.id);
        }
        assert_eq!(KNOWN.first().map(|k| k.id), Some("claude"), "the Claude Code adapter is the default");
    }

    /// Finder's PATH lacks user-installed runtimes. Discovery, the version probe and a live
    /// agent must all work with an env shebang whose interpreter is in ~/.local/bin.
    #[test]
    #[cfg(unix)]
    fn desktop_path_can_start_an_agent_and_its_runtime() {
        use super::super::process::{AgentProcess, FromAgent};
        use std::os::unix::fs::PermissionsExt;

        const CHILD: &str = "SOLIDCRAFT_AGENT_PATH_TEST";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = PathBuf::from(root);
            let agent = find_in("solidcraft-test-agent", &search_dirs()).unwrap();
            assert_eq!(version(&agent, Duration::from_secs(3)).as_deref(), Some("1.2.3"));
            let mut p = AgentProcess::spawn(&[agent.to_string_lossy().into_owned()], &root, &[], || {}).unwrap();
            let message = serde_json::json!({"id": 1, "method": "initialize"});
            p.send(&message);
            let reply = p.from_agent.recv_timeout(Duration::from_secs(5)).unwrap();
            p.shutdown(true);
            match reply {
                FromAgent::Line(line) => assert_eq!(serde_json::from_str::<serde_json::Value>(&line).unwrap(), message),
                FromAgent::Exited(tail) => panic!("the agent could not start its runtime: {tail:?}"),
            }
            return;
        }

        let root = std::env::temp_dir().join(format!("sc-agent-desktop-path-{}", std::process::id()));
        let bin = root.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        for (name, script) in [
            ("solidcraft-test-agent", "#!/usr/bin/env solidcraft-test-runtime\n"),
            ("solidcraft-test-runtime", "#!/bin/sh\nif [ \"$2\" = \"--version\" ]; then printf 'test-agent 1.2.3\\n'; else cat; fi\n"),
        ] {
            let path = bin.join(name);
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // Change only a child process's environment: no global mutation or unsafe set_var.
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "agent_chat::discover::tests::desktop_path_can_start_an_agent_and_its_runtime", "--nocapture"])
            .env(CHILD, &root)
            .env("HOME", &root)
            .env("PATH", "/usr/bin:/bin")
            .env_remove("NPM_CONFIG_PREFIX")
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(root);
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}
