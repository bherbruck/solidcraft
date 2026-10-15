# Contributing to SolidCraft

SolidCraft is an open-source parametric 3D CAD app written in pure Rust.
You sketch on a plane, constrain the sketch, turn profiles into solids, and
change a parameter to watch the timeline rebuild. It runs as a desktop app (Windows, Linux), in
the browser (wasm + WebGPU/WebGL2), headless from a CLI, and as an MCP server that AI agents can
model with.

Help is welcome, whether that's a one-line fix or owning a whole area. This page covers how to
build it, how the code is laid out, the few rules that aren't negotiable, and where help is
needed most.

## Get it running (about 10 minutes on a fast machine)

You need stable Rust 1.95 or newer (required by egui/eframe 0.36). For the web
build you also need `rustup target add wasm32-unknown-unknown` and [`trunk`](https://trunkrs.dev).

```sh
git clone https://github.com/bherbruck/solidcraft && cd solidcraft
cargo run --release -p solidcraft -- --sample      # desktop app with a sample part
cargo xtask test                                   # unit, engine and UI-scenario tests
cargo xtask ci                                     # everything a PR must pass (see below)
```

SolidCraft is the product, repository, Cargo package and executable name.
Build the desktop app with `cargo build --release -p solidcraft`, or the CLI with
`cargo build --release -p solidcraft-cli`. Run the CLI with
`cargo run --release -p solidcraft-cli -- commands` (or `target/release/solidcraft-cli commands`
after building). The old name appears only in the compatibility shims listed in
[docs/compatibility.md](docs/compatibility.md); don't add new uses of it.

Web: `cd apps/solidcraft-web && trunk serve --release`, then open the printed URL (`?sample`
opens the sample part, `?webgl` forces WebGL2). Saving and opening files in the browser needs
HTTPS or localhost, because it uses the Origin Private File System.

Windows from Linux: `cargo xwin build --release --target x86_64-pc-windows-msvc -p solidcraft`.

### NixOS: `nix develop`

NixOS has no `/usr/lib`, so nothing is on `LD_LIBRARY_PATH` and a plain `cargo run` finds none of
the libraries the app loads at run time: the Wayland ones winit opens the window through, the EGL
loader wgpu renders through, and libdbus-1, which rfd's file dialogs reach xdg-desktop-portal
through (a dialog then never appears and the pick returns no path):

```
ERROR solidcraft: winit EventLoopError: ... The wayland library could not be loaded
ERROR eframe::native::run: Exiting because of error: WGPU error: Failed to create surface for any enabled backend: {}
ERROR rfd::backend::xdg_desktop_portal::portal::libdbus: Can't connect to a portal: libdbus-1.so not found
```

`nix develop` (the [`flake.nix`](flake.nix) in the repository root) supplies Wayland,
X11, EGL, Vulkan-loader and D-Bus runtime libraries. It also exposes the NixOS
driver manifests in `/run/opengl-driver/share`, so Vulkan can discover the system
drivers. Enable `hardware.graphics.enable` on NixOS; other Linux distributions
may need a matching [nixGL wrapper](https://github.com/nix-community/nixGL).
The default development shell retains the Rust/C toolchain already on `PATH`;
`nix develop .#rust` supplies the pinned toolchain and builds in `target/nix`.

To launch, use `nix run .#solidcraft -- --sample`. To install the app's terminal
commands, desktop entry and generated icons, use `nix profile install .#solidcraft`.
`nix build` alone creates a build result; it does not install a launcher in your
desktop menu. The installed `solidcraft` command runs the wrapped app; `solidcraft-cli` is included as
well. See
[Nix installation and troubleshooting](docs/nix.md) for remote-flake commands,
desktop integration and how to distinguish an old executable on `PATH`.

## How the code is laid out

The [README](README.md#architecture) has the layer table. The short version:

- `crates/geom`: vectors, planes, meshes, measures.
- `crates/sketch`: our own constraint solver, plus profile finding.
- `crates/kernel`: the B-rep kernel boundary. It wraps [truck](https://github.com/ricosjp/truck),
  and it's the only crate allowed to touch truck. Fillets, chamfers, shell/offset, Delete Face,
  Split Face, STEP/IGES read and write, and face provenance are our own code here. Patched
  copies of three truck crates live in `vendor/` and are excluded from our fmt/clippy rules.
- `crates/doc`: the document. It holds parameters and expressions, the feature timeline and its
  incremental rebuild, components and occurrences, joints, appearances, and persistent naming.
- `crates/io`: design files and mesh formats (3MF, STL, OBJ).
- `crates/engine`: the session and the command registry. **Everything the user can do is a
  command**, and the toolbar, dialogs, scripts, CLI, control channel and MCP all run the same
  commands.
- `crates/ui-egui`: the desktop/web front end (egui + wgpu). It's swappable; nothing below it
  depends on egui.
- `crates/mcp`, `apps/solidcraft`, `apps/solidcraft-cli`, `apps/solidcraft-web`.

`cargo xtask layers` fails the build if a lower layer depends on a higher one.

## Rules that aren't negotiable

1. **Independent.** SolidCraft is independent. Behaviour is designed from public
   documentation, general CAD conventions and user feedback. Reference tests use analytic values
   and open-source tools. We don't drive, script, automate or extract data from commercial CAD
   products, and we never copy another product's code, icons, artwork, help text, tooltips,
   templates or materials. Command ids are our own (`solid.fillet`).
2. **No GPL/LGPL/AGPL code, not even reading it.** That rules out OpenCASCADE, FreeCAD
   (including planegcs), SolveSpace, CadQuery/OCP, LibreCAD and QCAD. SolidCraft is
   MIT OR Apache-2.0, and its solver and blends are written from the maths. (Running an
   open-source application to get reference values for a test is fine; reading its source is
   not.) Permissive
   dependencies are fine. Note which one you're adding and why in the PR, and run
   `cargo xtask licences` so THIRD-PARTY-LICENSES.txt lists it (CI checks that it is current).
   Nightly/manual CI also scans advisories, bans and sources with cargo-deny; see
   [the local command and coverage limits](docs/book/src/dev/testing.md#dependency-advisories).
3. **Every asset is original or openly licensed and has a row in
   [ATTRIBUTION.md](ATTRIBUTION.md).** Icons are drawn in code (`crates/ui-egui/src/icons.rs`).
   `cargo xtask assets` enforces this.
4. **Never crash.** People trust a CAD app with their work.
   - Non-test code never uses `unwrap`, `expect`, `panic!` or indexing on input-derived data.
     There's no `unsafe` outside `apps/solidcraft/src/open_documents.rs` (macOS file-open events)
     and `apps/solidcraft/src/native_crash.rs` (Windows native crash capture), and the compiler
     denies it.
   - An unsupported case returns an error ("not supported yet: …"); it never panics.
   - Every call into truck goes through `solidcraft_kernel::guard`.
   - Every crash fix comes with a regression test.
5. **Everything is a command, with a test.** A new feature means a `CommandSpec` in
   `crates/engine/src/cmd/`, engine tests that measure the result (volume, area,
   face/edge/vertex counts against a value you can derive), and for UI work a scenario (below).

[AGENTS.md](AGENTS.md) has the long form of these rules. It's written for AI agents, but it's
the most complete description of the house style.

## Tests

| What | Where | Run |
|---|---|---|
| Unit and engine tests | next to the code, or `*_tests.rs` files | `cargo xtask test` (or `cargo test --workspace --profile ci`) |
| UI scenarios | `crates/ui-egui/tests/scenarios/*.json` | part of `cargo test` |
| Reference tests | `cargo xtask oracle` | `tests/oracle/` (see below) |

**UI scenarios** drive the real app headless: start a tool, click a plane or a sketch point,
press keys, then assert on the result.

```json
[
  {"start": "sketch.create"},
  {"click": {"plane": "XY"}},
  {"start": "sketch.rectangle.two_point"},
  {"click": {"sketch": [0, 0]}},
  {"click": {"sketch": [40, 30]}},
  {"expect": {"errors": 0}}
]
```

A new `.json` file in that folder is picked up automatically. To record a bug before it's fixed,
put `{"pending": "why"}` in the scenario, and remove it in the PR that fixes the bug.

**A failing test without its fix can't merge as-is:** it would turn `main`'s CI red for everyone.
Either include the fix in the same PR, or mark the test `#[ignore = "fails until #123 is fixed"]`
(it can then merge right away), and remove that line in the PR that fixes the bug.

**Use the `ci` profile.** CI runs the tests with `--profile ci`: light optimisation, line tables,
no LTO, and debug assertions off (truck has debug-only assertions that reject valid geometry). It
builds several times faster than `--release`. If a test fails only in a plain debug build, say so
in the PR. `cargo xtask test` builds the tests with it and then runs every test binary at once
(`cargo test` runs them one after another); arguments after `--` go to each binary, e.g.
`cargo xtask test -- shell`. Tests run beside each other on a busy machine, so a test must not
depend on wall-clock timing.

**CI shards the tests.** One job builds them into a cargo-nextest archive and four jobs run a
quarter each, so every test runs in its own process: don't rely on state left by another test.
Pull requests skip a few heavy tests (one to four minutes each), which run on every push to
`main` and nightly; check the `main` run after merging. They are listed in
`.config/nextest.toml` and the [testing page](docs/book/src/dev/testing.md):

- `solidcraft-engine`: `tests::hostile_params_never_panic`, everything in `tests::robustness` and
  `tests::robustness_goldens` (pull requests that touch `crates/kernel` run its `sample_*` part),
  `pattern_suppress_tests::enclosure_arc_extinction_keeps_a_real_sharp_inner_lip_after_resize_and_reopen`,
  `pattern_suppress_tests::enclosure_datums_follow_wall_height_and_board_depth_with_material_proofs`,
  `cmd::plastic::tests::enclosure_snap_fit_follows_resized_front_wall`
- `solidcraft-kernel`: `tests::boolean_identity_on_tangent_primitives`
- `solidcraft-ui-egui` scenarios: `acceptance_sensor_enclosure`

**The action matrix** tries every ordinary action on every face, edge, vertex and body of a
corpus of designs against a checked-in ratchet ([docs/action-matrix.md](docs/action-matrix.md)).
A pull request that makes an action pass should record it with `cargo xtask action-matrix --bless`;
one that makes a passing action fail fails the sample every CI run checks.

**The reference tests** are 76 parts in [`tests/oracle/`](tests/oracle/), each a `recipe.json`
in our own format with an `expected.json` beside it. SolidCraft rebuilds the recipe and compares
body count, volume, area, centre of mass and bounding box (and, where there is a `reference.stl`,
the surface itself). Expected values are either worked out in closed form
(`tools/oracle-analytic/`) or computed by building the same recipe in FreeCAD, run as a separate
program (`tools/freecad-oracle/`); each file says which. [docs/oracle.md](docs/oracle.md) has
the current results, and CI runs `cargo xtask oracle` on every pull request. A case that fails
because of a known bug names its issue in `expected.json` (`"known_failure": "#123"`); never
change an expected value to match SolidCraft's output.

## Sending a change

1. Work is tracked in [GitHub issues](https://github.com/bherbruck/solidcraft/issues), labelled
   by area (`area:kernel`, `area:sketch`, `area:doc`, `area:ui`, `area:infra`). Before starting,
   comment on the issue so it gets the `in progress` label and two people don't build the same
   thing. For anything bigger than a bug fix that has no issue yet, open one first.
2. Branch from `main` and keep the PR to one topic.
3. Run `cargo xtask ci` before pushing. It runs fmt, the asset, licence and layer checks,
   clippy (`-D warnings`), then the tests with the wasm build check alongside. A PR has to pass it.
4. Reference the issue (`Fixes #12`). In the PR, say what you tested and how. For geometry work, include the measured numbers (and
   the expected ones).

Help reviewers see what the words mean. For observable CAD, UI or model behavior, please
include actual descriptive screenshots or a short recording in the issue or PR. This evidence
is required for changes or bugs that can be demonstrated in the app, including kernel,
geometry, assembly and backend work. Coding agents should run a small reproduction and
capture the evidence themselves; please don't ask a human for evidence the agent can produce.
Show the actual failure or result, preferably before and after, with brief steps and
expected/actual behavior. An arrow or annotation can help identify the detail that matters.

Flowcharts and Mermaid diagrams cannot replace evidence of visible behavior. Diagrams are
useful for architectural reasoning when actual screenshots cannot illustrate the point.
Changes with no useful observable behavior do not need screenshot or video evidence. Use a
small example without private designs or sensitive details; attach the media or link to an
accessible recording rather than committing generated review artifacts.

Keep checking upstream commits and the relevant PR's merge status throughout the work and
before handoff. Integrate upstream merges, resolve conflicts without losing others' changes,
and validate the integrated result. Before pushing, run `git pull --rebase origin main`, re-run
CI if it brought changes, and push without force. Report the upstream/PR state you checked,
any conflicts resolved and validation results. While a task remains active, follow an open
PR through its status changes; at handoff, clearly identify anything still awaiting merge.

Much of SolidCraft so far was written by AI coding agents (Claude) working in parallel, each
owning one area, with a human setting direction and reporting bugs. Their working notes are in
[AGENTS.md](AGENTS.md). Human contributors are just as welcome. Use whatever tools you like, as
long as the change meets the rules above.

## Where help is wanted most

See the [`help wanted`](https://github.com/bherbruck/solidcraft/labels/help%20wanted) and
[`good first issue`](https://github.com/bherbruck/solidcraft/labels/good%20first%20issue) labels.
In order of how much they'd help users:

1. **Fillet and blend robustness.** Fillets are our own code (truck has no fillet operation).
   Constant, variable-radius and chord fillets work, and so do corners where fillets of
   different radii meet, including three rounds meeting at a vertex. Next are setback corners, fillets along curved edges between curved
   faces, and fillets that consume neighbouring faces.
2. **Boolean robustness.** A random-design fuzzer currently gets about half of its designs
   through without a kernel error. Each failing seed is a reproducible bug.
3. **Y-up / Z-up.** SolidCraft is Z-up only, so most STEP files (from SolidWorks, Onshape, or
   Fusion's default) open lying on their backs. Fusion solves this with a "default modeling
   orientation" preference.
4. **Imported appearances.** Colours from STEP files are lost when a feature changes the
   imported body. They should become design appearances on import.
5. **Drawings, CAM and mesh tools.** These are out of scope for now (see
   [ROADMAP.md](ROADMAP.md)), but anyone who wants to own one is welcome.

## Updating the generated feature list

After adding or renaming commands, run `cargo xtask features`. The registry is the
source of truth for `docs/features.md`; do not hand-edit the generated rows.
The generator sorts commands by ID and omits global and per-section counters,
so additions to unrelated areas no longer rewrite shared summary lines.

If two branches still conflict in this file (for example, adjacent additions in
the same area), first resolve the command registry and source conflicts, then run
`cargo xtask features` again and stage the regenerated file. Do not use Git's
`union` merge driver: it can silently retain stale or duplicate command rows.
The generator tests cover stable ordering and additions that leave existing lines
unchanged. The initial switch to sorted rows is a one-time larger diff.

## Licence

By contributing you agree that your contribution is licensed as MIT OR Apache-2.0, like the rest
of SolidCraft. SolidCraft isn't affiliated with Autodesk. Fusion is a trademark of Autodesk, Inc.
