# SolidCraft — instructions for agents

SolidCraft is a clean-room, open-source, pure-Rust parametric 3D CAD application:
sketches with a constraint solver, a parametric timeline of features, a B-rep
kernel, and a desktop UI that agents can drive. Projects published by
[storytold](https://github.com/storytold) can serve as references for implementation conventions.
Any code adapted from other projects (MIT OR Apache-2.0) is noted in ATTRIBUTION.md.

## Product name
SolidCraft everywhere: crates `solidcraft-*`, binaries `solidcraft` and `solidcraft-cli`,
`.solidcraft` designs, the `solidcraft/N` format tag, `SOLIDCRAFT_*` environment variables,
settings and recovery folders, the MCP server name, docs, CI and packaging. The old name
survives only in the explicit compatibility shims listed in `docs/compatibility.md`; don't add
new uses of it anywhere else. Public docs/web/Flatpak deployments use project Pages
`/solidcraft/`; do not add permanent old-name Pages aliases.

## Start every session here
1. Read `plan/STATUS.md` (current milestone, next task), then the task in `plan/execution-plan.md`
   and the relevant `plan/architecture.md` section.
2. Kernel and library decisions: `plan/adr/0001-geometry-kernel.md` (what we may depend on, what
   is rejected for licence reasons).
3. Follow the autonomous operation protocol (`plan/execution-plan.md` §6). Don't stop to ask
   unless it lists the decision as the owner's.

`plan/` is gitignored (local only).

## Never crash
People trust SolidCraft with their designs; a crash loses their work. **This outranks feature work.**
- **No panics in non-test code:** no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`,
  `unimplemented!`; no `unsafe` (`unsafe_code = "forbid"`). The exceptions, in the app crate and in
  `crates/threedx-client` (which deny it instead), have no safe API: the macOS-only
  `crates/threedx-client/src/macos.rs` (loads the 3Dconnexion driver's client framework at run time
  and calls through its function pointers), two functions in
  `apps/solidcraft/src/open_documents.rs` (macOS hands Finder files only to an Objective-C
  method), the Windows-only `apps/solidcraft/src/native_crash.rs` (the exception filter that
  records crashes inside native code such as graphics drivers) and the Windows-only
  `apps/solidcraft/src/native_frame.rs` (the window procedure that answers the resize-border hit
  test of the window without OS decorations).
- **Errors are `Result<T, E>`** through the crate's error type and `?`. An unfinished feature
  returns an error ("not supported yet: …"); it never panics.
- **Input-derived numbers are hostile** (command parameters, design files, control-channel
  input): `get()` not `[i]`, finite checks, caps on sizes and loop counts.
- **Third-party kernel calls are guarded:** everything that calls `truck` goes through
  `solidcraft_kernel::guard`, which turns a panic into `KernelError::Internal`.
- **Last-resort guard:** `Session::execute` runs every command under `catch_unwind`; an escaped
  panic restores the design and reports `EngineError::Internal`.
- **Crash reports:** the desktop app writes every panic and every unclean exit to a local crash folder,
  with the design and the commands that reproduce it (`solidcraft_engine::crash`, docs/crash-reports.md).
  When someone says the app crashed, look there first: `solidcraft-cli crashes`, `crashes show <id>`,
  `crashes replay <id>` (or the `diagnostics.crash_reports` command / MCP `crash_reports` tool).
- **Prove it:** every crash fix lands with a regression test (see `hostile_params_never_panic`); a
  crash report's `design.solidcraft` + `replay.json` is the test's starting point.
- Every production crate root carries
  `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`.

## Non-negotiables
- **Independent.** SolidCraft is independent. Behaviour is designed from public documentation,
  general CAD conventions and user feedback. Reference tests use analytic values and open-source
  tools. Never copy another product's code, icons, artwork, help text, tooltips, templates or
  materials. File formats come from public specifications.
- **No commercial CAD products.** Agents must not drive, script, automate or extract data from
  commercial CAD products (Fusion, SolidWorks, Onshape, …) for this project: no test data,
  measurements, command lists, ids or files produced with them.
- **Never read, copy or link GPL/LGPL/AGPL code**: OpenCASCADE, FreeCAD (incl. planegcs),
  SolveSpace, CadQuery/OCP, LibreCAD, QCAD — don't even read their sources. (`tools/freecad-oracle/`
  runs FreeCAD as a separate program through its public scripting API to compute reference values;
  nothing from FreeCAD is linked, vendored or shipped.) The sketch solver and the blend
  operations are our own.
- **Assets — absolutely essential.** SolidCraft contains **no iconography, images, fonts or
  artwork from commercial CAD products — ever.** Every UI icon is drawn in collected code
  (`assets/icons/ui/source/`, compiled through `crates/ui-egui/src/icons.rs`). Any file asset must be original or openly licensed and **must have a row in ATTRIBUTION.md**
  (`cargo xtask assets` enforces it). Screenshots of commercial CAD software are never committed.
  Breaking this rule is the most serious mistake you can make in this repo.
- **Everything is a command.** User-visible behaviour = a command in `crates/engine/src/cmd/*`
  (`CommandSpec`: id = our own dotted, lower-case id `<area>.<thing>[.<variant>]` such as
  `solid.extrude` or `sketch.rectangle.two_point`, never another product's command id; label;
  toolbar tab and panel; icon; shortcut; params doc; `enabled`; JSON `run`) + tests. The toolbar,
  dialogs, sketch tools, palette, scripts, CLI and control channel all reach the same commands.
- **Programmatic calls never open dialogs.** `engine.execute` runs the JSON form; only toolbar
  clicks (`ui.start`) start interactive tools and dialogs, which then run commands.
- **Layering** is enforced by `cargo xtask layers`: L0 `geom` → L1 `sketch`, `kernel`, `render`
  → L2 `doc` → L3 `io` → L4 `engine` → L5 `ui-egui` → apps. Nothing below L5 depends on
  egui/eframe/winit/wgpu/rfd. **The UI crate is swappable.** Only `crates/kernel` may use `truck`.
- **The UI is thin**: panels read engine state and act through `app.run(id, params)` /
  `app.start(id)`. Colours come from `theme::Tokens`.
- **One set of camera controls**: every page or canvas with a camera (viewport, Render
  preview, future flat pattern or drawing pages) calls `navigation::handle` with its `NavCaps`
  (orbit off for flat 2D pages). New canvases must use `navigation::handle`; never hand-roll
  camera input (a test rejects it).
- **Rust only.** Units are millimetres and radians internally; Z is up. The one exception is
  JavaScript the browser requires: the service worker `apps/solidcraft-web/sw.js` (browsers only
  run service workers as JavaScript) and the small inline scripts in `apps/solidcraft-web/index.html`.
  Keep them minimal; `cargo xtask web-offline-test` covers the service worker.
- **Quality gates** before every commit: `cargo xtask ci` (fmt, clippy -D warnings, tests,
  assets, layers). Commit after every feature arc that builds.
- **Publishing**: `origin` is github.com/bherbruck/solidcraft. Changes land through **pull requests**:
  push a branch, open a PR, and let GitHub CI run. Maintainers (and agents working for them) may
  merge their own PR as soon as CI is green (`gh pr merge --squash`); a trivial fix (docs, typos)
  may be merged right away. Keep each PR to one topic, rebase or merge `main` into it when it
  conflicts, and never force-push `main`. Never commit `plan/`, build dirs (`target*/`) or
  generated reference data; check `git ls-files` before pushing.
- **Never break wasm**: everything below the apps must build for `wasm32-unknown-unknown`
  (`cargo xtask wasm`, part of `cargo xtask ci`).

## Showing and following through on changes

- Help reviewers see what is broken or changed. For observable CAD, UI or model behavior,
  issues and PRs must include actual descriptive screenshots or a short recording from a
  small reproduction. This includes kernel, geometry, assembly and backend changes whose
  effects can be shown in the app. Coding agents should run the app and capture this evidence
  themselves; do not ask a human for evidence the agent can produce. Show the actual failure
  or result, preferably before and after, with brief reproduction steps, expected/actual
  behavior and annotations that point out the relevant detail.
- Flowcharts and Mermaid diagrams cannot substitute for screenshots or recordings of visible
  behavior. Use diagrams for architectural reasoning when actual screenshots cannot illustrate
  the point. Changes with no useful observable behavior do not need screenshot or video evidence.
- Use a small public-safe example and check for private designs, paths, account details or
  other sensitive content before sharing. Attach media to the issue/PR or link to an accessible
  recording; do not commit generated screenshots or videos just to provide review evidence.
- Throughout the work and before handoff, check current upstream commits and the relevant
  PR's merge status. Keep your branch current with upstream merges, resolve conflicts without
  discarding others' work, and re-run validation when integration changes the result. Use
  `git pull --rebase origin main` before pushing as above; never force-push. Report the checked
  upstream/PR state, any conflicts resolved and the validation performed. If a PR remains open,
  keep monitoring it while the task is active and make its pending status clear at handoff;
  do not claim it merged without checking.

## Claim = a draft PR
- Before working on issue #N, check nobody has it: `gh pr list -R bherbruck/solidcraft --state open --search "#N"`.
- If none, branch from the latest `origin/main`, `git commit --allow-empty -m "claim: #N"`, push,
  and open a **draft** PR titled `<summary> (#N)` right away. **The lowest-numbered open PR for an
  issue wins**; if yours lost, close it. Don't claim by commenting or by editing files.
- Push work in progress at least daily. A draft with no pushes for 72 hours may be taken over:
  comment on it first, then open your own claim (you may start from its branch).
- On a shared build machine, check the load before a big build (`cut -d' ' -f1 /proc/loadavg`)
  and wait while it is over 24; build with `nice`.

## Design file format
A change that saves something new in a design adds ONE file, `crates/doc/src/format/pending/<name>.rs`
(a `//!` note, and `pub fn step` if older designs must be rewritten). Never edit `FORMAT`,
`JSON_STEPS`, the history in `format.rs`, another change's file or a `v<N>/` folder: the build
works out the number (the next one while anything is pending), and the release folds pending
changes into `v<N>/` with `scripts/fold-format-pending.sh`. See CONTRIBUTING.md.

## Application releases
Development pushes are not application releases. Follow `docs/releasing.md` when asked to
prepare or ship a release: choose the version from the actual changes, update Cargo.toml and
Cargo.lock, pass the quality gates, advance `release` without force-pushing, and verify the
resulting draft and its artifacts. When shipping is authorized, publish that verified draft
and confirm its tag and downstream deployment. Do not auto-release every push to `main`, infer
version bumps solely from commit prefixes, overwrite published versions, or push release tags
before the draft is verified.

## Running and looking at the app
- `cargo run --release -p solidcraft -- --sample --control PORT` (sample design + control channel).
  Pick a free port. Without a display, run it under `Xvfb :99` with `DISPLAY=:99`.
- Drive it with JSON lines on `127.0.0.1:PORT` (see `docs/control-protocol.md`). The first
  request on a connection carries the launch's token as `"token"`: read it from
  `~/.local/share/solidcraft/control/PORT.token`, or start the app with
  `SOLIDCRAFT_CONTROL_TOKEN=$(openssl rand -hex 32)` and send that.
  - `{"id":1,"method":"engine.execute","params":{"command":"solid.extrude","params":{"distance":20}}}`
  - `{"id":2,"method":"ui.start","params":{"command":"sketch.create"}}` then `ui.click {x, y}`
  - `{"id":3,"method":"ui.screenshot","params":{"path":"/tmp/shot.png"}}` then read the PNG.
- **For UI work, look at the result** (screenshot, read the PNG). Without a window use
  `ui.render` or `solidcraft-cli snapshot`.
- Headless: `solidcraft-cli run script.json --out part.step`, `solidcraft-cli eval design.solidcraft`,
  `solidcraft-cli snapshot script.json --out shot.png`, `solidcraft-cli commands`.
- Reference tests: `cargo xtask oracle` replays `tests/oracle/*/recipe.json` and compares with
  each case's `expected.json` (analytic or FreeCAD values; writes `docs/oracle.md`). Expected
  values never come from a commercial CAD product.
- Build with `CARGO_TARGET_DIR=<repo>/target`. Windows: `cargo xwin build --release --target x86_64-pc-windows-msvc -p solidcraft`.

## Roadmap
`ROADMAP.md` (committed) tracks status, milestones and estimates. Update it whenever a
milestone task lands. `docs/features.md` (SolidCraft's commands by area, from the registry) and
`THIRD-PARTY-LICENSES.txt` are regenerated on `main` after each merge by the Generated files
workflow; never regenerate them in a PR.

## Lists many PRs add to
Keep them conflict-free (CONTRIBUTING.md, "Lists many changes add to"): message catalogs sorted
(`cargo xtask lists --fix`), `mod` lines in their sorted group, the language list English first
then by id, generated files left to `main`, and a design-format change as one new file.
