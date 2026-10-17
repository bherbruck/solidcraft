//! `cargo xtask web-offline-test [--site DIR]`: prove that the web app works offline and updates.
//!
//! Serves six release builds, a site without the app and a development build at
//! `/solidcraft/app/` (as on GitHub Pages, with its 10-minute HTTP caching) from a local server
//! that can also act offline, flaky, hung or mid-deployment, plus another copy of the app at
//! `/solidcraft/elsewhere/` on the same origin, and drives headless Chrome through the DevTools
//! protocol through these steps (twenty-one page launches, which failures are numbered by):
//!
//! 1. online, build v1: the page loads, the service worker takes over and keeps every file;
//! 2. offline: v1 starts from the kept copy;
//! 3. the other copy (build v2) is opened and kept; offline, v1 still starts (copies at
//!    different paths keep their own builds);
//! 4. build v2 deployed, but only `build.json` and `sw.js` download: v1 still starts (never a
//!    page whose code can't be fetched);
//! 5. the same for build v3: v1 starts, and v2's unfinished download is dropped (failed updates
//!    don't pile up);
//! 6. build v2 deployed again: its page fails at once while its other files are still coming:
//!    v1 starts, and its download doesn't stay in progress (so it can be dropped);
//! 7. v3's files download, but v4 is deployed meanwhile (its `index.html` comes): v1 starts, and
//!    v3 isn't marked complete (a kept build never mixes deployments);
//! 8. online: the first launch already gets v3 (resuming its download), and only v3 stays kept;
//! 9. offline: v3 starts;
//! 10. network hangs: after waiting for it a few seconds, v3 starts from the kept copy;
//! 11. build v4 deployed, but the check for it is slow while v4's worker takes over (as when
//!     another tab updates): the kept v3 starts and works, its copy retired but not deleted;
//! 12. no open page uses the retired copies any more: a launch (v4) deletes them;
//! 13. build v5's worker downloads slowly while a launch finds v6 deployed and downloads it:
//!     v6 starts, v5's download isn't dropped from under its worker, then v6's worker takes over;
//! 14. with no launch in between (a page left open), updates to v2 and then v3 both fail: the
//!     second one's install drops the first's unfinished download;
//! 15. the app removed (no `sw.js`, no `build.json`): the site's page starts, from the network;
//! 16. nothing deployed at all (every file 404): v6 starts from the kept copy;
//! 17. offline: v6 still starts from the kept copy;
//! 18. a development build (`trunk serve`) on the same origin: it starts (no `build.json`)
//!     while the worker retires, dropping everything kept;
//! 19. the development build starts, from the network, with no worker.
//!
//! The site is a small stand-in app (default) or a real web build (`--site dist/web`, after
//! `trunk build --release`); a real build must hold the `sw.js` and `build.json` the generator
//! writes for its files. Each build then gets its own `sw.js` and `build.json` from
//! `cargo xtask web-offline`'s generator and a `data-build` marker on `<html>`. Chrome comes from
//! `CHROME` or the usual install locations; `cargo xtask ci` skips this test when there is none.
//!
//! The stand-in app has a content-named script, a `shared.js` whose name is the same in every
//! build (a page must get its own build's), and an on-demand file under `fonts/` (not kept).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::web_offline;

const PREFIX: &str = "/solidcraft/app/";
/// Another copy of the app on the same origin.
const ELSEWHERE: &str = "/solidcraft/elsewhere/";
/// How long one launch may take (a real build compiles ~30 MB of wasm).
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(90);
/// sw.js's NAVIGATE_TIMEOUT_MS, less a margin: a hung network is waited for at least this long.
const SLOW_NETWORK_WAIT: Duration = Duration::from_secs(3);
/// How long a file takes in Mode::SlowFiles.
const SLOW_FILE: Duration = Duration::from_secs(6);

/// Exercise the real browser build's command bridge, using the same Chrome harness as the offline test.
pub fn run_api(site: &str) -> Result<(), String> {
    let site = std::fs::canonicalize(site).map_err(|e| format!("{site}: {e}; build apps/solidcraft-web with trunk first"))?;
    if !site.join("index.html").is_file() {
        return Err(format!("{}: index.html missing; build apps/solidcraft-web with trunk first", site.display()));
    }
    let chrome = find_chrome().ok_or("web-api-test: Chrome not found (set CHROME to its executable)")?;
    let work = TempDir::new()?;
    let server = Server::start(site.clone())?;
    server.set(Mode::Serve(site));
    let browser = Chrome::launch(&chrome, &work.0.join("profile"))?;
    let mut cdp = browser.connect()?;
    cdp.call("Emulation.setDeviceMetricsOverride", json!({"width": 1280, "height": 900, "deviceScaleFactor": 1, "mobile": false}))?;
    cdp.call("Page.navigate", json!({"url": format!("http://127.0.0.1:{}{PREFIX}?webgl", server.port)}))?;
    wait_for(&mut cdp, "!!window.solidcraft && !document.getElementById('solidcraft_loading')", "the browser API is ready", |v| v == true)?;
    let result = cdp.eval(API_TEST)?;
    println!("web-api-test: {result}");
    Ok(())
}

const API_TEST: &str = r#"(async () => {
    const app = window.solidcraft;
    const check = (condition, message) => { if (!condition) throw new Error(message); };
    const rejects = async (call, message) => {
        try { await call(); } catch (error) {
            check(error instanceof Error, 'rejections must be Error objects');
            check(error.message.includes(message), error.message);
            return;
        }
        throw new Error('expected rejection: ' + message);
    };
    const commands = await app.commands();
    check(JSON.stringify(commands).includes('solid.box'), 'command discovery');
    await app.request('ui.home', {open: false});
    const before = await app.inspect();
    check(before.bodies.length === 0, 'test starts with an empty design');
    await app.execute('solid.box', {length: 30, width: 20, height: 10, body_name: 'Browser API box'});
    const after = await app.inspect({measure: true});
    check(after.bodies.length === 1, 'box appears in the active document');
    check(Math.abs(after.bodies[0].volume_mm3 - 6000) < 0.001, 'box volume is 6000 mm³');
    await app.execute('edit.undo');
    check((await app.inspect()).bodies.length === 0, 'undo removes the box');
    await app.execute('edit.redo', null);
    check((await app.inspect()).bodies.length === 1, 'redo restores the box');
    await rejects(() => app.request('missing.browser.method'), 'unknown');
    await rejects(() => app.execute('missing.browser.command'), 'unknown');
    const circular = {}; circular.self = circular;
    await rejects(() => app.request('ui.inspect', circular), 'JSON serializable');
    await rejects(() => app.request('ui.inspect', {value: 1n}), 'JSON serializable');
    await rejects(() => app.request('ui.inspect', {text: 'x'.repeat(16 * 1024 * 1024)}), '16 MiB');
    const queued = Array.from({length: 257}, () => app.request('ui.inspect'));
    const settled = await Promise.allSettled(queued);
    check(settled.filter(r => r.status === 'fulfilled').length === 256, 'queued requests drain across frames');
    check(settled.filter(r => r.status === 'rejected' && r.reason instanceof Error && r.reason.message.includes('pending')).length === 1,
        'queue limit rejects excess work');
    check((await app.inspect()).bodies.length === 1, 'API still works after rejected requests');
    const ui = await app.request('ui.inspect');
    check(ui.home === false && ui.viewport[2] > 600, 'model viewport is visible');
    return {commands: 'discovered', bodies: [0, 1, 0, 1], errors: 'rejected', queue: '256 completed, 1 rejected'};
})()"#;

/// Run the test; `required: false` (from `cargo xtask ci`) skips it when Chrome is missing.
pub fn run(root: &Path, site: Option<&str>, required: bool) -> Result<(), String> {
    let Some(chrome) = find_chrome() else {
        if required {
            return Err("web-offline-test: Chrome not found (set CHROME to its executable)".into());
        }
        println!("web-offline-test: skipped (no Chrome; set CHROME to its executable)");
        return Ok(());
    };
    let work = TempDir::new()?;
    let builds = ["v1", "v2", "v3", "v4", "v5", "v6", "gone", "dev"].map(|b| (work.0.join(b), b));
    let started = match site {
        Some(dir) => {
            check_site(root, Path::new(dir), &work.0.join("check"))?;
            for (to, build) in &builds {
                copy_dir(Path::new(dir), to)?;
                let index = to.join("index.html");
                let html = std::fs::read_to_string(&index).map_err(|e| format!("{}: {e}", index.display()))?;
                std::fs::write(&index, mark(&html, build)?).map_err(|e| format!("{}: {e}", index.display()))?;
            }
            // The wasm ran: the loading message is gone (started) or says why it couldn't start.
            "(() => { const el = document.getElementById('solidcraft_loading'); return !el || !el.innerText.startsWith('Loading'); })()"
        }
        None => {
            let registration = registration_script(root)?;
            for (to, build) in &builds {
                // The site without the app doesn't register a worker.
                stand_in(to, build, if *build == "gone" { "" } else { &registration })?;
            }
            "['started', 'shared'].every((k) => document.documentElement.dataset[k] === document.documentElement.dataset.build)"
        }
    };
    let mut kept = Vec::new();
    let [(v1, _), (v2, _), (v3, _), (v4, _), (v5, _), (v6, _), (gone, _), (dev, _)] = builds;
    for dir in [&v1, &v2, &v3, &v4, &v5, &v6] {
        kept.push(web_offline::generate(root, dir)?);
    }
    // The site without the app: no worker, no build.json.
    for file in ["sw.js", "build.json"] {
        let _ = std::fs::remove_file(gone.join(file));
    }
    // A development build, as `trunk serve` writes it: no build.json, the retiring worker.
    let _ = std::fs::remove_file(dev.join("build.json"));
    web_offline::retire(root, &dev)?;

    let server = Server::start(v2.clone())?;
    let browser = Chrome::launch(&chrome, &work.0.join("profile"))?;
    let mut cdp = browser.connect()?;
    // Every file any build keeps, by name (the state counts a cache's entries among them).
    let mut files = std::collections::BTreeSet::new();
    for dir in [&v1, &v2, &v3, &v4, &v5, &v6] {
        let path = dir.join("build.json");
        let build: Value =
            std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or_else(|| format!("{}: not readable", path.display()))?;
        files.extend(build["files"].as_array().into_iter().flatten().filter_map(|f| f["file"].as_str().map(str::to_string)));
    }
    let state_js = STATE_JS.replace("STARTED", started).replace("BUILD_FILES", &json!(files.into_iter().collect::<Vec<_>>()).to_string());
    let count = std::cell::Cell::new(0);
    let launch_at = |cdp: &mut Cdp, at: &str, build: &str| -> Result<Duration, String> {
        let n = count.get() + 1;
        count.set(n);
        let t = Instant::now();
        let r = cdp.call("Page.navigate", json!({ "url": format!("http://127.0.0.1:{}{at}?launch={n}", server.port) }))?;
        if let Some(e) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(format!("launch {n} (build {build}): the page didn't load: {e}"));
        }
        let search = format!("?launch={n}");
        wait_for(cdp, &state_js, &format!("launch {n} starts build {build}"), |s| {
            s["search"] == search.as_str() && s["ready"] == "complete" && s["build"] == build && s["started"] == true
        })?;
        Ok(t.elapsed())
    };
    let launch = |cdp: &mut Cdp, build: &str| launch_at(cdp, PREFIX, build);
    // The page is controlled by the worker, which keeps exactly this build's files (and the mark
    // that they are all there).
    let keeps = |i: usize| {
        let (name, n) = kept.get(i).map(|(v, n)| (cache_name(PREFIX, v), n + 1)).unwrap_or_default();
        move |s: &Value| s["controlled"] == true && s["kept"] == json!({ name.clone(): n })
    };
    let step = |what: &str, t: Duration| println!("ok    {what} ({:.1}s)", t.as_secs_f64());

    server.set(Mode::Serve(v1));
    let t = launch(&mut cdp, "v1")?;
    wait_for(&mut cdp, &state_js, "the worker keeps build v1", keeps(0))?;
    step("online: build v1 starts and is kept for offline use", t);

    server.set(Mode::Refuse);
    step("offline: build v1 starts from the kept copy", launch(&mut cdp, "v1")?);

    // The other copy is always online, with build v2.
    let t = launch_at(&mut cdp, ELSEWHERE, "v2")?;
    let elsewhere = kept.get(1).map(|(v, n)| (cache_name(ELSEWHERE, v), n + 1)).unwrap_or_default();
    wait_for(&mut cdp, &state_js, "the other copy keeps build v2", |s| {
        s["controlled"] == true && s["kept"] == json!({ elsewhere.0.clone(): elsewhere.1 })
    })?;
    launch(&mut cdp, "v1")?;
    step("another copy at another path keeps its own build; offline, v1 still starts here", t);

    server.set(Mode::Flaky(v2.clone()));
    step("flaky: build v2 can't be downloaded whole, so v1 starts", launch(&mut cdp, "v1")?);

    server.set(Mode::Flaky(v3.clone()));
    let t = launch(&mut cdp, "v1")?;
    let [kept1, kept3] = [0, 2].map(|i| kept.get(i).map(|(v, _)| cache_name(PREFIX, v)).unwrap_or_default());
    wait_for(&mut cdp, &state_js, "only v1 and v3's unfinished download are kept", |s| {
        s["caches"].as_object().is_some_and(|c| c.len() == 2 && c.contains_key(&kept1) && c.contains_key(&kept3))
    })?;
    step("flaky again: build v3 neither, so v1 starts; v2's unfinished download is dropped", t);

    server.set(Mode::FailFast(v2.clone()));
    let t = launch(&mut cdp, "v1")?;
    // Past the time the slow files would have arrived.
    std::thread::sleep(SLOW_FILE + Duration::from_secs(1));
    wait_for(&mut cdp, &state_js, "no download is left marked in progress", |s| s["filling"] == json!([]))?;
    step("one file fails at once, others are still coming: v1 starts; v2's isn't left in progress", t);

    server.set(Mode::Mixed(v3.clone(), v4.clone()));
    let t = launch(&mut cdp, "v1")?;
    let only_v1 = kept1.clone();
    wait_for(&mut cdp, &state_js, "v3 isn't marked complete", |s| s["complete"] == json!([only_v1.clone()]))?;
    step("deployed mid-download: v4's page doesn't fit v3's files, so v1 starts", t);

    server.set(Mode::Serve(v3.clone()));
    let t = launch(&mut cdp, "v3")?;
    wait_for(&mut cdp, &state_js, "the worker keeps build v3 only", keeps(2))?;
    step("update: build v3 starts on the first launch; only it stays kept", t);

    server.set(Mode::Refuse);
    step("offline: build v3 starts from the kept copy", launch(&mut cdp, "v3")?);

    server.set(Mode::Hang);
    let t = launch(&mut cdp, "v3")?;
    if t < SLOW_NETWORK_WAIT {
        return Err(format!("slow network: the kept copy started after {t:?}, before the network was given a chance"));
    }
    step("slow network: build v3 starts from the kept copy", t);

    // Every page was remembered long ago.
    let age = "caches.keys().then((ks) => Promise.all(ks.map(async (k) => {
      const c = await caches.open(k);
      if (k.startsWith('solidcraft-app-pages ')) for (const r of await c.keys()) {
        const page = await (await c.match(r)).json();
        await c.put(r, new Response(JSON.stringify({ ...page, at: 0 })));
      }
    }))).then(() => true)";
    // A second tab runs v3. Build v4 is deployed, past the HTTP cache's ten minutes (only the
    // worker's copies hold v3); while this tab's launch waits for build.json, the other tab
    // updates the worker to v4 and leaves. No other page uses v3 then: only the launched page
    // keeps it (v4's worker takes over once the launch has ended).
    server.set(Mode::Serve(v3.clone()));
    let mut other = browser.new_tab()?;
    launch(&mut other, "v3")?;
    server.set(Mode::SlowCheck(v4.clone()));
    cdp.call("Network.clearBrowserCache", json!({}))?;
    other.eval(age)?;
    other.eval(
        "navigator.serviceWorker.getRegistration().then((r) => {
          setTimeout(() => { r.update(); location.replace('about:blank'); }, 1000);
          return true;
        })",
    )?;
    cdp.call("Page.navigate", json!({ "url": "about:blank" }))?;
    let t = launch(&mut cdp, "v3")?;
    let [name3, name4] = [2, 3].map(|i| kept.get(i).map(|(v, _)| cache_name(PREFIX, v)).unwrap_or_default());
    wait_for(&mut cdp, &state_js, "v4's worker keeps v4, v3's copy retired", |s| {
        s["kept"].as_object().is_some_and(|k| k.len() == 1 && k.contains_key(&name4))
            && s["retired"].as_array().is_some_and(|r| r.contains(&json!(name3)))
    })?;
    step("update in another tab: the kept v3 still starts; its copy is retired, not deleted", t);
    other.call("Page.close", json!({}))?;

    server.set(Mode::Serve(v4));
    cdp.eval(age)?;
    // The page on screen still uses v3 while the next one loads; the launch after that deletes it.
    let t = launch(&mut cdp, "v4")?;
    launch(&mut cdp, "v4")?;
    wait_for(&mut cdp, &state_js, "the retired copies are deleted", |s| s["retired"] == json!([]))?;
    step("no open page uses them any more: the retired copies are deleted; v4 starts", t);

    // v5's worker installs, its files coming slowly; meanwhile v6 is deployed and launched.
    let update = "navigator.serviceWorker.getRegistration().then((r) => { r.update(); return true; })";
    let name5 = kept.get(4).map(|(v, _)| cache_name(PREFIX, v)).unwrap_or_default();
    let asked = server.slow.load(Ordering::SeqCst);
    server.set(Mode::SlowFiles(v5));
    cdp.eval(update)?;
    wait_for(&mut cdp, &state_js, "v5's worker starts downloading", |s| s["caches"].get(&name5).is_some())?;
    // Chrome sends a server at most six requests at once: a build with more files has the rest
    // wait for the first ones. v6 is deployed once all of v5's are asked for (any asked later
    // would get v6's file, which rightly ends v5's download, but that isn't this step).
    let files5 = kept.get(4).map_or(0, |(_, n)| *n);
    server.wait_asked_slowly(asked + files5)?;
    server.set(Mode::Serve(v6));
    let t = launch(&mut cdp, "v6")?;
    wait_for(&mut cdp, &state_js, "v5's download is finished, not dropped", |s| s["complete"].as_array().is_some_and(|c| c.contains(&json!(name5))))?;
    cdp.eval(update)?;
    wait_for(&mut cdp, &state_js, "the worker keeps build v6", keeps(5))?;
    step("overlapping downloads: v6 starts; v5's worker still finishes its own; v6 takes over", t);

    // The page stays open while two updates fail in a row: only installs run, no launch.
    let [name2, name3] = [1, 2].map(|i| kept.get(i).map(|(v, _)| cache_name(PREFIX, v)).unwrap_or_default());
    let t = Instant::now();
    server.set(Mode::Flaky(v2.clone()));
    cdp.eval(update)?;
    wait_for(&mut cdp, &state_js, "v2's update fails, leaving an unfinished download", |s| {
        s["caches"].get(&name2).is_some() && s["filling"] == json!([])
    })?;
    server.set(Mode::Flaky(v3.clone()));
    cdp.eval(update)?;
    wait_for(&mut cdp, &state_js, "v3's install drops v2's unfinished download", |s| {
        s["caches"].get(&name3).is_some() && s["caches"].get(&name2).is_none() && s["filling"] == json!([])
    })?;
    step("two failed updates with no launch: the second's install drops the first's leftovers", t.elapsed());

    server.set(Mode::Serve(gone));
    step("app removed (no build.json): the site's page starts from the network", launch(&mut cdp, "gone")?);
    server.set(Mode::Serve(work.0.join("nothing")));
    step("nothing deployed (every file 404): v6 starts from the kept copy", launch(&mut cdp, "v6")?);
    server.set(Mode::Refuse);
    step("offline: build v6 still starts from the kept copy", launch(&mut cdp, "v6")?);

    server.set(Mode::Serve(dev));
    let t = launch(&mut cdp, "dev")?;
    wait_for(&mut cdp, &state_js, "the worker retires", |s| s["registered"] == 0 && s["caches"] == json!({}))?;
    step("development build deployed: it starts while the worker retires and drops what it kept", t);
    let t = launch(&mut cdp, "dev")?;
    wait_for(&mut cdp, &state_js, "no worker", |s| s["controlled"] == false && s["registered"] == 0)?;
    step("development build: starts from the network", t);
    println!("web-offline-test: passed");
    Ok(())
}

/// What a launch shows: the page, which build it is, whether it started, the worker's caches.
const STATE_JS: &str = "(async () => {
  const scope = new URL('.', location.href).href;
  const prefix = 'solidcraft-app ' + new URL(scope).pathname + ' ';
  const caches_ = {}, kept = {}, retired = [], complete = [];
  // Each cache is read with `caches.match`, never opened: opening a cache that a worker has just
  // deleted makes it again (empty), and nothing would delete it after that.
  const marks = ['build-complete', 'build-retired'];
  for (const k of await caches.keys()) {
    if (!k.startsWith(prefix)) continue;
    const here = async (file) => Boolean(await caches.match(new URL(file, scope).href, { cacheName: k }));
    let n = 0;
    for (const file of BUILD_FILES.concat(marks)) if (await here(file)) n += 1;
    caches_[k] = n;
    if (await here('build-complete')) complete.push(k);
    if (await here('build-retired')) retired.push(k);
    else kept[k] = n;
  }
  const filling = (await navigator.locks.query()).held.map((l) => l.name).filter((n) => n.startsWith(prefix));
  return { search: location.search, ready: document.readyState, build: document.documentElement.dataset.build ?? null,
    started: STARTED, controlled: !!navigator.serviceWorker.controller, caches: caches_, kept, retired, complete, filling,
    registered: (await navigator.serviceWorker.getRegistrations()).filter((r) => r.scope === scope).length };
})()";

/// The worker's cache for a build of the copy at `path` (sw.js's CACHE_PREFIX + version).
fn cache_name(path: &str, version: &str) -> String {
    format!("solidcraft-app {path} {version}")
}

/// A real build must hold what the generator writes for its files: `build.json`, and a `sw.js`
/// with them (not the retiring one a debug build gets).
fn check_site(root: &Path, site: &Path, scratch: &Path) -> Result<(), String> {
    copy_dir(site, scratch)?;
    web_offline::generate(root, scratch)?;
    for file in ["sw.js", "build.json"] {
        let built =
            std::fs::read(site.join(file)).map_err(|e| format!("{}: {e} (build it with `trunk build --release`)", site.join(file).display()))?;
        let expected = std::fs::read(scratch.join(file)).map_err(|e| format!("{}: {e}", scratch.join(file).display()))?;
        if built != expected {
            return Err(format!("{}: not what `cargo xtask web-offline` writes for this build (was it a release build?)", site.join(file).display()));
        }
    }
    Ok(())
}

/// Poll the page's state until `done` (errors while a page is loading count as not yet).
fn wait_for(cdp: &mut Cdp, state_js: &str, what: &str, done: impl Fn(&Value) -> bool) -> Result<(), String> {
    let t = Instant::now();
    let mut last = Value::Null;
    while t.elapsed() < LAUNCH_TIMEOUT {
        match cdp.eval(state_js) {
            Ok(s) if done(&s) => return Ok(()),
            Ok(s) => last = s,
            Err(e) => last = Value::String(e),
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("{what}: timed out after {LAUNCH_TIMEOUT:?}; last state: {last}"))
}

/// `<html …>` with `data-build="build"`.
fn mark(html: &str, build: &str) -> Result<String, String> {
    if !html.contains("<html") {
        return Err("index.html has no <html> tag".into());
    }
    Ok(html.replacen("<html", &format!("<html data-build=\"{build}\""), 1))
}

/// The service worker registration from the web app's index.html (so the stand-in uses it).
fn registration_script(root: &Path) -> Result<String, String> {
    let path = root.join("apps/solidcraft-web/index.html");
    let html = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let at = html.find("serviceWorker").ok_or("apps/solidcraft-web/index.html doesn't register the service worker")?;
    let start = html.get(..at).and_then(|h| h.rfind("<script>")).ok_or("index.html: no <script> around the registration")?;
    let end = html.get(at..).and_then(|h| h.find("</script>")).map(|e| at + e + "</script>".len()).ok_or("index.html: unclosed <script>")?;
    html.get(start..end).map(str::to_string).ok_or_else(|| "index.html: bad registration script".into())
}

/// A stand-in app: a page and a content-named script, as Trunk builds them.
fn stand_in(dir: &Path, build: &str, registration: &str) -> Result<(), String> {
    let write = |name: &str, text: String| std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let page = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>Offline test</title></head>\n<body>\n<script src=\"app-{build}.js\"></script>\n<script src=\"shared.js\"></script>\n{registration}\n</body>\n</html>\n"
    );
    write("index.html", mark(&page, build)?)?;
    write(&format!("app-{build}.js"), format!("document.documentElement.dataset.started = \"{build}\";\n"))?;
    write("shared.js", format!("document.documentElement.dataset.shared = \"{build}\";\n"))?;
    std::fs::create_dir_all(dir.join("fonts")).map_err(|e| format!("fonts: {e}"))?;
    write("fonts/on-demand.txt", format!("{build}\n"))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let e = e.map_err(|e| format!("{}: {e}", from.display()))?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// A scratch directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<TempDir, String> {
        let dir = std::env::temp_dir().join(format!("solidcraft-web-offline-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(TempDir(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- The site's server -----------------------------------------------------------------------

#[derive(Clone)]
enum Mode {
    /// Online, serving this build.
    Serve(PathBuf),
    /// Offline: connections close unanswered.
    Refuse,
    /// Flaky: the build's `build.json` and `sw.js` come; its files fail.
    Flaky(PathBuf),
    /// Mid-deployment: the first build, but the second one's `index.html`.
    Mixed(PathBuf, PathBuf),
    /// The network hangs: connections stay open, unanswered (until the mode changes).
    Hang,
    /// Online, serving this build, but `build.json` hangs (until the mode changes).
    SlowCheck(PathBuf),
    /// Online, serving this build, but its files (not `build.json`, `sw.js`) take SLOW_FILE.
    SlowFiles(PathBuf),
    /// Like SlowFiles, but `index.html` fails at once.
    FailFast(PathBuf),
}

struct Server {
    port: u16,
    mode: Arc<Mutex<Mode>>,
    /// How many files were asked for in Mode::SlowFiles.
    slow: Arc<AtomicUsize>,
}

impl Server {
    /// Serves the app at PREFIX in the server's mode, and `elsewhere` at ELSEWHERE (always online).
    fn start(elsewhere: PathBuf) -> Result<Server, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("server: {e}"))?;
        let port = listener.local_addr().map_err(|e| format!("server: {e}"))?.port();
        let mode = Arc::new(Mutex::new(Mode::Refuse));
        let slow = Arc::new(AtomicUsize::new(0));
        let (shared, asked) = (mode.clone(), slow.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (shared, asked, elsewhere) = (shared.clone(), asked.clone(), elsewhere.clone());
                std::thread::spawn(move || answer(stream, &shared, &asked, &elsewhere));
            }
        });
        Ok(Server { port, mode, slow })
    }

    /// Wait (at most 30 s) until `n` files in all were asked for in Mode::SlowFiles.
    fn wait_asked_slowly(&self, n: usize) -> Result<(), String> {
        for _ in 0..300 {
            if self.slow.load(Ordering::SeqCst) >= n {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!("only {} of {n} slow files were asked for", self.slow.load(Ordering::SeqCst)))
    }

    fn set(&self, mode: Mode) {
        if let Ok(mut m) = self.mode.lock() {
            *m = mode;
        }
    }
}

/// Hold a connection while the server's mode stays as it was (at most 20 s).
fn hold(shared: &Mutex<Mode>, still: impl Fn(&Mode) -> bool) {
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(100));
        if !shared.lock().is_ok_and(|m| still(&m)) {
            return;
        }
    }
}

fn answer(mut stream: TcpStream, shared: &Mutex<Mode>, slow: &AtomicUsize, elsewhere: &Path) {
    let Ok(read) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read);
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => return,
            Ok(_) if header == "\r\n" => break,
            Ok(_) => {}
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or_default();
    let (file, other) = match path.strip_prefix(ELSEWHERE) {
        Some(f) => (Some(f), true),
        None => (path.strip_prefix(PREFIX), false),
    };
    let file = file.map(|f| if f.is_empty() { "index.html" } else { f }).filter(|f| !f.contains(".."));
    // The mode when the request comes, not when the connection was opened: Chrome opens
    // connections ahead of use.
    let mode = if other { Mode::Serve(elsewhere.to_path_buf()) } else { shared.lock().map(|m| m.clone()).unwrap_or(Mode::Refuse) };
    let small = file.is_some_and(|f| ["build.json", "sw.js"].contains(&f));
    let (dir, flaky, slow_check) = match mode {
        Mode::Serve(dir) => (dir, false, false),
        Mode::Flaky(dir) => (dir, true, false),
        Mode::SlowCheck(dir) => (dir, false, true),
        Mode::SlowFiles(dir) => {
            if !small {
                slow.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(SLOW_FILE);
            }
            (dir, false, false)
        }
        Mode::FailFast(dir) => {
            if file == Some("index.html") {
                return;
            }
            if !small {
                std::thread::sleep(SLOW_FILE);
            }
            (dir, false, false)
        }
        Mode::Mixed(dir, next) => (if file == Some("index.html") { next } else { dir }, false, false),
        Mode::Refuse => return,
        Mode::Hang => return hold(shared, |m| matches!(m, Mode::Hang)),
    };
    if flaky && !small {
        return;
    }
    if slow_check && file == Some("build.json") {
        return hold(shared, |m| matches!(m, Mode::SlowCheck(_)));
    }
    let body = file.and_then(|f| std::fs::read(dir.join(f)).ok());
    let kind = match file.and_then(|f| f.rsplit('.').next()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    };
    let (status, body) = match body {
        Some(b) => ("200 OK", b),
        None => ("404 Not Found", b"not found".to_vec()),
    };
    // GitHub Pages caches everything for ten minutes. The stand-in's shared.js is the exception:
    // with that, the browser would reuse an older build's copy without asking the worker (a real
    // build names its files after their contents), and the test checks the worker's choice.
    let cache = if file == Some("shared.js") { "no-cache" } else { "max-age=600" };
    let head =
        format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: {cache}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(&body));
}

// ---- Headless Chrome ----------------------------------------------------------------------------

fn find_chrome() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CHROME") {
        return Some(PathBuf::from(p));
    }
    let installed = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    if let Some(p) = installed.iter().map(PathBuf::from).find(|p| p.is_file()) {
        return Some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|d| ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"].map(|n| d.join(n)))
        .find(|p| p.is_file())
}

struct Chrome {
    child: Child,
    profile: PathBuf,
}

impl Chrome {
    fn launch(exe: &Path, profile: &Path) -> Result<Chrome, String> {
        let mut c = Command::new(exe);
        c.arg("--headless=new")
            .arg(format!("--user-data-dir={}", profile.display()))
            .args(["--remote-debugging-port=0", "--no-first-run", "--no-default-browser-check", "about:blank"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Linux CI runners don't allow Chrome's sandbox; the browser only opens this test's pages.
        if cfg!(target_os = "linux") {
            c.arg("--no-sandbox");
        }
        let child = c.spawn().map_err(|e| format!("{}: {e}", exe.display()))?;
        Ok(Chrome { child, profile: profile.to_path_buf() })
    }

    /// The DevTools connection to Chrome's page.
    fn connect(&self) -> Result<Cdp, String> {
        let port = self.port()?;
        let list: Value = serde_json::from_slice(&http(port, "GET", "/json/list")?).map_err(|e| format!("/json/list: {e}"))?;
        let page = list.as_array().and_then(|l| l.iter().find(|t| t["type"] == "page")).ok_or("Chrome has no page to drive")?;
        Cdp::to_target(port, page)
    }

    /// The DevTools connection to a new tab.
    fn new_tab(&self) -> Result<Cdp, String> {
        let port = self.port()?;
        let tab: Value = serde_json::from_slice(&http(port, "PUT", "/json/new?about:blank")?).map_err(|e| format!("/json/new: {e}"))?;
        Cdp::to_target(port, &tab)
    }

    fn port(&self) -> Result<u16, String> {
        let t = Instant::now();
        loop {
            // Chrome writes its DevTools port here once it listens.
            let file = std::fs::read_to_string(self.profile.join("DevToolsActivePort")).unwrap_or_default();
            if let Some(port) = file.lines().next().and_then(|l| l.trim().parse::<u16>().ok()) {
                return Ok(port);
            }
            if t.elapsed() > Duration::from_secs(30) {
                return Err("Chrome didn't start its DevTools server within 30 s".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http(port: u16, method: &str, path: &str) -> Result<Vec<u8>, String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("DevTools: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    write!(s, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").map_err(|e| format!("DevTools: {e}"))?;
    let mut r = BufReader::new(s);
    let mut length = 0;
    loop {
        let mut line = String::new();
        r.read_line(&mut line).map_err(|e| format!("DevTools: {e}"))?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse().map_err(|_| "DevTools: bad Content-Length")?;
        }
    }
    let mut body = vec![0; length];
    r.read_exact(&mut body).map_err(|e| format!("DevTools: {e}"))?;
    Ok(body)
}

/// A minimal DevTools protocol client: JSON messages over a WebSocket.
struct Cdp {
    stream: TcpStream,
    next: u64,
}

impl Cdp {
    /// Connect to a target from Chrome's `/json` listing.
    fn to_target(port: u16, target: &Value) -> Result<Cdp, String> {
        let ws = target["webSocketDebuggerUrl"].as_str().ok_or("DevTools target without a WebSocket URL")?;
        let path = ws.split_once("//").and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..])).ok_or("bad DevTools URL")?;
        Cdp::connect(port, path)
    }

    fn connect(port: u16, path: &str) -> Result<Cdp, String> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("DevTools: {e}"))?;
        stream.set_read_timeout(Some(LAUNCH_TIMEOUT)).map_err(|e| e.to_string())?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: c29saWRjcmFmdC10ZXN0IQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .map_err(|e| format!("DevTools: {e}"))?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).map_err(|e| format!("DevTools handshake: {e}"))?;
            head.push(byte[0]);
            if head.len() > 8192 {
                return Err("DevTools handshake: no end of headers".into());
            }
        }
        if !head.starts_with(b"HTTP/1.1 101") {
            return Err(format!("DevTools handshake refused: {}", String::from_utf8_lossy(&head)));
        }
        Ok(Cdp { stream, next: 0 })
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), String> {
        // Client frames are masked (RFC 6455 §5.3).
        const MASK: [u8; 4] = [0x5a, 0x17, 0xc3, 0x8e];
        let mut frame = vec![0x80 | opcode];
        match payload.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n <= 0xffff => {
                frame.push(0x80 | 126);
                frame.extend((n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend((n as u64).to_be_bytes());
            }
        }
        frame.extend(MASK);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ MASK[i % 4]));
        self.stream.write_all(&frame).map_err(|e| format!("DevTools: {e}"))
    }

    fn recv(&mut self) -> Result<String, String> {
        let mut message = Vec::new();
        loop {
            let mut h = [0u8; 2];
            self.stream.read_exact(&mut h).map_err(|e| format!("DevTools: {e}"))?;
            let (fin, opcode) = (h[0] & 0x80 != 0, h[0] & 0x0f);
            let len = match h[1] & 0x7f {
                126 => {
                    let mut b = [0u8; 2];
                    self.stream.read_exact(&mut b).map_err(|e| format!("DevTools: {e}"))?;
                    u64::from(u16::from_be_bytes(b))
                }
                127 => {
                    let mut b = [0u8; 8];
                    self.stream.read_exact(&mut b).map_err(|e| format!("DevTools: {e}"))?;
                    u64::from_be_bytes(b)
                }
                n => u64::from(n),
            };
            if len > 64 << 20 {
                return Err("DevTools: message too large".into());
            }
            let mut mask = [0u8; 4];
            if h[1] & 0x80 != 0 {
                self.stream.read_exact(&mut mask).map_err(|e| format!("DevTools: {e}"))?;
            }
            let mut payload = vec![0u8; len as usize];
            self.stream.read_exact(&mut payload).map_err(|e| format!("DevTools: {e}"))?;
            payload.iter_mut().enumerate().for_each(|(i, b)| *b ^= mask[i % 4]);
            match opcode {
                0x8 => return Err("DevTools closed the connection".into()),
                0x9 => self.send_frame(0xa, &payload)?,
                0x0..=0x2 => {
                    message.extend(payload);
                    if fin {
                        return String::from_utf8(message).map_err(|e| format!("DevTools: {e}"));
                    }
                }
                _ => {}
            }
        }
    }

    /// Call a DevTools method and return its result.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.send_frame(0x1, json!({ "id": id, "method": method, "params": params }).to_string().as_bytes())?;
        loop {
            let reply: Value = serde_json::from_str(&self.recv()?).map_err(|e| format!("DevTools: {e}"))?;
            if reply["id"] == id {
                if let Some(e) = reply.get("error") {
                    return Err(format!("{method}: {e}"));
                }
                return Ok(reply["result"].clone());
            }
        }
    }

    /// Evaluate a JavaScript expression in the page (awaiting a promise) and return its value.
    fn eval(&mut self, expr: &str) -> Result<Value, String> {
        let r = self.call("Runtime.evaluate", json!({ "expression": expr, "awaitPromise": true, "returnByValue": true }))?;
        if let Some(e) = r.get("exceptionDetails") {
            return Err(e["exception"]["description"].as_str().unwrap_or("exception").to_string());
        }
        Ok(r["result"]["value"].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registration_comes_from_the_web_apps_page() {
        let s = registration_script(&crate::root()).unwrap_or_default();
        assert!(s.starts_with("<script>") && s.ends_with("</script>") && s.contains("register(\"sw.js\")"), "{s}");
    }

    #[test]
    fn mark_tags_the_html_element() {
        assert_eq!(mark("<!DOCTYPE html>\n<html lang=\"en\">", "v2").ok().as_deref(), Some("<!DOCTYPE html>\n<html data-build=\"v2\" lang=\"en\">"));
        assert!(mark("<body>", "v1").is_err());
    }
}
