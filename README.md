<p align="center">
  <img src="assets/icons/app/linux/exports/hicolor/256x256/apps/ai.storyteller.solidcraft.png" width="96" height="96" alt="SolidCraft" />
</p>

<h1 align="center">SolidCraft</h1>

<p align="center">
  <strong>An open-source parametric 3D CAD application built in pure Rust.</strong><br />
  Sketch, constrain, and build solids with a parametric feature timeline. Change a dimension
  and watch your model rebuild. Run it on your desktop, in your browser, or let AI agents
  model through the built-in MCP server.
</p>

<div align="center">

[![Download for Windows][badge-windows]][windows-msi]
[![Download for macOS][badge-macos]][macos-dmg]
[![Download for Linux][badge-linux]][linux-appimage]
[![Get it from the Snap Store][badge-snap]][snap-store]
[![Try it in your browser][badge-web]][web-app]

</div>

<p align="center">
  <a href="https://solidcraft.io">Website</a> ·
  <a href="https://solidcraft.io/download">Download</a> ·
  <a href="https://solidcraft.io/app">Try it in the browser</a> ·
  <a href="https://docs.solidcraft.io/">Documentation</a> ·
  <a href="https://discord.gg/FtU4NQNVDH">Discord</a> ·
  <a href="https://github.com/bherbruck/solidcore">SolidCore</a>
</p>

<p align="center">
  <a href="https://github.com/tangency-labs/solidcraft/actions/workflows/ci.yml?query=branch%3Amain"><img src="https://github.com/tangency-labs/solidcraft/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI" /></a>
  <a href="https://github.com/tangency-labs/solidcraft/releases/latest"><img src="https://img.shields.io/github/v/release/tangency-labs/solidcraft?label=release" alt="Latest release" /></a>
  <a href="https://github.com/tangency-labs/solidcraft/releases"><img src="https://img.shields.io/github/downloads/tangency-labs/solidcraft/total?label=downloads" alt="Downloads" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/tangency-labs/solidcraft" alt="Licence" /></a>
  <a href="https://discord.gg/FtU4NQNVDH"><img src="https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
  <a href="https://remade.wiki/projects/solidcraft/"><img src="https://remade.wiki/badges/listed-on-remade-classic.svg" alt="Listed on Remade" /></a>
  <a href="https://github.com/tangency-labs/solidcraft/stargazers"><img src="https://img.shields.io/github/stars/tangency-labs/solidcraft?style=flat" alt="Stars" /></a>
</p>

![SolidCraft 0.8: model a Raspberry Pi 4 case from an empty design: a sketch dimensioned with parameters, extrude, fillets and shell, a rim rounded in one pick, standoffs patterned on the board's mounting holes, port cut-outs, vent slots, "Pi 4" engraved in a font, a PETG material with its mass in Properties, and a height change in Parameters](docs/screenshots/demo-0.8.gif)

*A Raspberry Pi 4 case from an empty design: a sketch dimensioned with the board's parameters, extrude, fillet, shell, the rim rounded in one pick, an M2.5 standoff patterned on the 58 × 49 mm hole rectangle, port cut-outs, vent slots, "Pi 4" engraved in DejaVu Sans Bold and a PETG material defined in the design, with its mass in Properties, then height 24 → 32 mm in Parameters and it all rebuilds. 9× playback. [Full video](https://github.com/tangency-labs/solidcraft/raw/refs/heads/main/docs/screenshots/demo-0.8.mp4), recorded with the [demo script](examples/demo/record_ui.sh).*

## Download

- **Windows:** download and run the [installer][windows-msi].
  Also for [ARM64][windows-arm64-msi] and [32-bit][windows-x86-msi] PCs, or as a
  [portable zip][windows-portable] that runs without installing.
  With [WinGet](https://learn.microsoft.com/windows/package-manager/winget/):
  `winget install bherbruck.SolidCraft`.
- **macOS:** open the [disk image][macos-dmg] and drag SolidCraft into Applications
  (Apple silicon and Intel).
- **Linux:** download the [AppImage][linux-appimage], allow it to run as a program and open it.
  Or install the [.deb][linux-deb] (Ubuntu, Debian, Mint) or [.rpm][linux-rpm] (Fedora,
  openSUSE), or use [Flatpak](https://docs.solidcraft.io/guide/getting-started.html#linux-flatpak).
  With Snap: `sudo snap install solidcraft` ([Snap Store][snap-store], or the [.snap file][linux-snap]).
  For ARM64: [AppImage][linux-arm64-appimage], [.deb][linux-arm64-deb], [.rpm][linux-arm64-rpm].
- **Web:** [try it in your browser][web-app], no install.

The downloads aren't signed yet. On Windows, if SmartScreen warns you, choose **More info**, then
**Run anyway**. On macOS, the first time, right-click SolidCraft in Applications and choose **Open**.

**On SolidCraft 0.11 to 0.14?** Its **Update now** can't install 0.15.0 or later, because the
releases moved to a new address. Download the new version once from the links above; after that,
updating from inside SolidCraft works again ([details](https://docs.solidcraft.io/guide/updates.html#updating-from-011-to-014)).

Older versions, checksums and other formats are on the [releases page](https://github.com/tangency-labs/solidcraft/releases).
Install details, upgrading from earlier builds and building from source are in
[Getting started](https://docs.solidcraft.io/guide/getting-started.html).

[badge-windows]: https://img.shields.io/badge/Windows-Download-0078D6?style=for-the-badge&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0id2hpdGUiPjxwYXRoIGQ9Ik0yIDJoOS41djkuNUgyek0xMi41IDJIMjJ2OS41aC05LjV6TTIgMTIuNWg5LjVWMjJIMnpNMTIuNSAxMi41SDIyVjIyaC05LjV6Ii8%2BPC9zdmc%2B
[badge-macos]: https://img.shields.io/badge/macOS-Download-333333?style=for-the-badge&logo=apple&logoColor=white
[badge-linux]: https://img.shields.io/badge/Linux-AppImage-E95420?style=for-the-badge&logo=linux&logoColor=white
[badge-snap]: https://img.shields.io/badge/Snap-Snap_Store-82BEA0?style=for-the-badge&logo=snapcraft&logoColor=white
[badge-web]: https://img.shields.io/badge/Web-Try_in_browser-2EA44F?style=for-the-badge&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSJ3aGl0ZSIgc3Ryb2tlLXdpZHRoPSIyIj48Y2lyY2xlIGN4PSIxMiIgY3k9IjEyIiByPSIxMCIvPjxwYXRoIGQ9Ik0yIDEyaDIwTTEyIDJhMTUgMTUgMCAwIDEgMCAyME0xMiAyYTE1IDE1IDAgMCAwIDAgMjAiLz48L3N2Zz4%3D
[windows-msi]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-windows-x64.msi
[windows-arm64-msi]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-windows-arm64.msi
[windows-x86-msi]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-windows-x86.msi
[windows-portable]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-windows-x64-portable.zip
[macos-dmg]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-macos-universal.dmg
[linux-appimage]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-linux-x86_64.AppImage
[linux-deb]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-linux-x86_64.deb
[linux-rpm]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-linux-x86_64.rpm
[linux-arm64-appimage]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-linux-aarch64.AppImage
[linux-arm64-deb]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-linux-aarch64.deb
[linux-arm64-rpm]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-linux-aarch64.rpm
[linux-snap]: https://github.com/tangency-labs/solidcraft/releases/latest/download/solidcraft-linux-x86_64.snap
[snap-store]: https://snapcraft.io/solidcraft
[web-app]: https://solidcraft.io/app

**Documentation:** the [SolidCraft book](https://docs.solidcraft.io/) (user guide,
developer guide and command reference; sources in [docs/book](docs/book)).

**Community:** chat with users and contributors on [Discord](https://discord.gg/FtU4NQNVDH).

**Status: early but usable for simple parts.** 75 of 76 public reference parts
match their analytic or FreeCAD-computed volume, area, centre of mass and bounding box
([docs/oracle.md](docs/oracle.md)); the other is a tracked bug.
These checks do not establish complete geometric or workflow equivalence. The commands SolidCraft has
are listed by area in [docs/features.md](docs/features.md); status and plans are in [ROADMAP.md](ROADMAP.md).
Open work and known bugs are in [GitHub issues](https://github.com/tangency-labs/solidcraft/issues),
and help is welcome: see [CONTRIBUTING.md](CONTRIBUTING.md).

## Built with SolidCraft

<a href="https://github.com/0xlillith/solidcraft-rover"><img src="docs/screenshots/rover-build.gif" alt="SELENE, a scientific rover by 0xlillith, assembling itself as the timeline plays through its 3,323 steps in SolidCraft: tub, wheels, electronics, suspension, sampling arm, solar wings, dish and camera turret" loading="lazy"></a>

*SELENE, a scientific rover built entirely through SolidCraft's MCP server by
[@0xlillith](https://github.com/0xlillith/solidcraft-rover): 3,323 editable timeline steps that
rebuild with no errors, replayed here at 2× speed.
[Full video](https://github.com/tangency-labs/solidcraft/raw/refs/heads/main/docs/screenshots/rover-build.mp4)
· [download the editable model](https://github.com/0xlillith/solidcraft-rover/releases/tag/v1.0.0).*

## What works

- **Current-view DXF export** from File → Export Current View as DXF, with hidden edges
  omitted or dashed, shown body placements and the current camera. See [view DXF](docs/view-dxf.md).

- **Sketches** on origin planes, offset planes or planar faces: lines, rectangles, circles,
  arcs, polygons, slots, ellipses, fit/control splines, conics and text; construction geometry,
  projection, trimming, fillets and chamfers, including corners at the origin.
  New face attachments retain supported construction identities and chosen occurrences;
  missing sources expose an explicit plane repair workflow. See [face attachments](docs/face-attachments.md).
  Explicitly closed fit splines and closed associative offsets use periodic C2 interpolation.
  Supported convex line-and-arc offsets retain their identity when an inset consumes corner
  arcs, and linked extrusions follow the current boundary. Breaking the association freezes
  the boundary and keeps existing extrusions usable.
  Curve drawing and picking adapt together to zoom and display scale, including small circles
  at Fit and sketches in placed components.
- **Constraints and dimensions** solved by our own solver. Points the constraints fix relative
  to known geometry are placed exactly by construction (loci intersections and rigid-cluster
  decomposition, after Owen 1991 and Fudos–Hoffmann 1997); Levenberg–Marquardt solves the rest,
  and the rank of its Jacobian gives the remaining degrees of freedom. Constraints: coincident,
  point on curve, horizontal/vertical, parallel, perpendicular, tangent, curvature (G2), equal,
  concentric, collinear, midpoint, symmetry, fix; linear, aligned, horizontal, vertical,
  point-to-line, radius, diameter, angle, arc sweep, arc length and slot length dimensions driven
  by parameters. Over-constraining edits are rejected; fully constrained geometry turns black.
  Dragging solves the sketch every frame and rebuilds the model once, when the drag ends.
  Select a line or its two endpoints, type its dimension and press Enter to commit one undo
  step; with Sketch Dimension it's D, click, type, Enter. Fit keeps sketch geometry clear of the palette and navigation controls.
- **Profiles** found automatically, including regions split by lines and holes inside outlines.
  Newly captured complete adaptive boundaries retain their outer and hole source identities
  when spline span counts change. Partial or ambiguous boundaries do not qualify.
- **Features** on a parametric timeline: extrude, revolve, sweep, loft, fillet/chamfer,
  shell, draft, holes, threads, patterns, mirror, split, combine, move and construction geometry.
  The timeline rebuilds incrementally; features can be edited, suppressed, renamed, reordered
  and deleted. Supported geometry and current limitations are described in [ROADMAP.md](ROADMAP.md).
  New revolves preserve symmetric and two-sided angular placement across reversed axes;
  older designs retain their saved convention when reopened or edited.
- **Parameters and configurations** with unit-aware expressions (`2 * width + 5 mm`,
  `angle / 2`), ordered rules, feature suppression and per-row body materials/appearances.
- **Assemblies** with nested components and occurrences, joints, motion studies, contact sets,
  interference and material-aware mass properties. Recorded body transfers preserve modelling
  history and world placement; 3D sketch wires retain their coordinate frames through edits.
- **Sheet metal** with flanges, hems, fold/unfold/refold, material cutouts, flat patterns,
  retained shallow prismatic pockets, exact line/arc/circle DXF outlines and bend tables;
  plastic enclosure features and rules. Pocket depth and side survive unfold/refold and appear
  separately from through-hole outlines in the flat preview and DXF.
- **Files**: design containers (`.solidcraft`) with lossless binary mesh payloads;
  plain JSON remains supported for opening, Save As and export. Desktop Open can show a saved
  provisional picture from warm geometry caches while rebuilding before editing; caches never
  replace the parametric design. Export to STEP (AP242 with body names, colours and
  components as assemblies; revolved faces as native planes, cylinders, cones, spheres and tori),
  IGES 5.3 (manifold solid B-reps with names and colours), 3MF, STL (binary/ASCII) and OBJ, and
  assemblies with joints as URDF for robot simulators (docs/urdf.md);
  3MF, STL, OBJ and glTF 2.0 (`.glb`/`.gltf`, its node tree as components and its materials as
  appearances) import as mesh bodies (they render, measure, move and export; solid features
  need B-rep bodies). The `mesh.convert` command reconstructs supported closed constant-section parts
  as parametric solids with planar and circular cylindrical faces;
  STEP import (AP203/AP214/AP242 solids, assemblies, units, names, colours) and IGES import
  (manifold solid B-reps, or trimmed and bounded surfaces sewn into solids) as a base feature
  that later features build on — open a `.step`/`.stp`/`.igs`/`.iges` file, insert one into a
  design, or drop it on the window ([docs/step-import.md](docs/step-import.md)); FreeCAD
  `.FCStd` and Rhino `.3dm` solids import the same way through
  [cadmpeg](https://github.com/cadmpeg/cadmpeg) in the desktop app and CLI.
- **Render:** opaque surfaces can use metal fraction, roughness and dielectric IOR in the
  integrated product viewport and CPU preview PNGs. Existing simple colors remain supported.
  Fixed direct lighting is a preview; HDRI, refraction and texture maps are not implemented.
  See [opaque surfaces](docs/render-opaque-pbr.md).
- **Display meshes** from our own tessellation planner: each face becomes a plan of (u, v)
  points and triangles that shares its edge nodes with its neighbours, so the mesh is watertight.
  Facets follow the chord tolerance, the facet angle and a longest side, with cells at most 2:1
  along straight directions. Box-bounded and four-cornered faces mesh as structured grids.
  Trimmed faces are cut into four-sided patches: squares around holes, butterflies in round
  faces, motorcycle traces from concave corners (after Lyon et al. 2019). Each patch then meshes
  as a Coons grid. Intersection edges are evaluated by Newton on both surfaces. A compute shader
  evaluates the plans for the whole scene in one dispatch, with a CPU fallback when the GPU
  can't build it. Planning and evaluating the 78 reference bodies takes about 65 ms, 50–65×
  faster than the previous mesher.
- **Desktop app** (egui + wgpu): toolbar with workspace tabs, browser, timeline, 3D viewport with
  axis view and navigation bar, sketch tools, feature dialogs, command palette, and a JSON-lines
  control channel so agents and tests can drive it.
  Feature errors and Help can open an editable **Create Issue** draft with relevant context
  and an optional reviewed screenshot; opening GitHub does not submit the report.
- **Headless CLI** `solidcraft-cli`: run command scripts, measure (volume, area, centre of mass,
  face/edge/vertex counts), export, render PNG snapshots and replay reference recipes.
- **MCP server** `solidcraft-cli mcp`: AI agents model, measure and look at parts headless or
  in the running app (see below).

## Build and run

```sh
cargo run --release -p solidcraft -- --sample          # desktop app with a sample part
cargo run --release -p solidcraft-cli -- commands      # the command registry
cargo run --release -p solidcraft-cli -- run examples/bracket.json --out bracket.step
cargo run --release -p solidcraft-cli -- snapshot examples/bracket.json --out bracket.png
cargo run --release -p solidcraft-cli -- render examples/render-studio.json --out product.png
cargo run --release -p solidcraft -- part.step          # open a STEP file as a new design
cargo run --release -p solidcraft-cli -- eval part.step # measure an imported STEP file
cargo xtask ci                                          # fmt, clippy, tests, icons, assets, layers, wasm
```

UI artwork changes follow the [icon specification](assets/icons/ui/STYLE_GUIDE.md).
The [included review tool](assets/icons/ui/README.md) discovers new icons and keeps
unaccepted artwork Open; run `cargo xtask icons` to check the collection.

Windows builds: `cargo xwin build --release --target x86_64-pc-windows-msvc -p solidcraft`.

On Windows and Linux, SolidCraft detects a connected 3D mouse through its HID multi-axis
controller interface: a 3Dconnexion SpaceNavigator or SpaceMouse, or a homemade device that
declares itself one. Move the cap to pan and zoom, and tilt or twist it to rotate the 3D view.
While editing a sketch, the cap pans and zooms without changing the sketch view. The Navigation
page in Preferences has an on/off switch, a speed control and, for each of the cap's six axes, an
invert switch and a speed for the 3D mouse. The web app reads
it too, through the browser; Firefox needs a 3DxWare setting first (see
[Getting started › 3D mouse in the browser](docs/book/src/guide/getting-started.md#3d-mouse-in-the-browser)).

With 3DxWare on Windows, SolidCraft follows 3DxWare's profile for it when there is one, and
3DxWare's object mode (tilt to roll, twist to spin) until there is. The mouse-wheel turns that
3DxWare makes from the cap's motion are ignored, so the view doesn't zoom in steps as the cap tilts.

On Linux SolidCraft reads the 3D mouse through `spacenavd` when that daemon runs (its socket
`/var/run/spnav.sock`), so what you set in `spnavcfg` (swapped or inverted axes, sensitivity, dead
zones) applies as it does in other programs, and no udev rule is needed. With nothing set there,
the motion is the same as without the daemon. Without `spacenavd`, SolidCraft reads `/dev/hidrawN`
directly, so the logged-in user needs access to the device. A udev rule that adds the `uaccess`
tag does that, for example
`/etc/udev/rules.d/70-3d-mouse.rules` for USB devices with 3Dconnexion's vendor id, and for its
Bluetooth ones (where `idVendor` isn't set, so the HID id is matched instead):

```
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="256f", TAG+="uaccess"
SUBSYSTEM=="hidraw", KERNELS=="0005:256F:*", TAG+="uaccess"
```

Older SpaceNavigators use Logitech's vendor id `046d`, so add a rule with their product id (see
`lsusb`) instead of matching every Logitech device. A homemade device needs a rule with its own
vendor and product id. Reload with `sudo udevadm control --reload && sudo udevadm trigger`, then
replug the device. The Flatpak sandbox does not expose hidraw devices, so the 3D mouse only works
with native builds there for now.

SolidCraft finds the axes from the device's report descriptor, so it doesn't matter how a device
lays them out: translation and rotation in separate reports (3Dconnexion's wired devices), all six
in one (its wireless ones and most homemade devices), after the buttons, or narrower than 16 bits.
Each axis is scaled from the range the descriptor declares.

In the browser: `cd apps/solidcraft-web && trunk build --release --public-url ./` (WebGPU, or
WebGL2 with `?webgl`; `?sample` opens the sample part).

Agents can control the document open in a browser tab through `window.solidcraft.execute(...)`
and `window.solidcraft.request(...)`; see the [browser command API](docs/browser-api.md).

## Scripts

A script is a JSON list of commands, the same ones the UI runs:

```json
{"commands": [
  {"command": "sketch.create", "params": {"plane": "XY"}},
  {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [40, 30]}},
  {"command": "solid.extrude", "params": {"distance": 20}},
  {"command": "solid.fillet", "params": {"edges": [[0, 0, 10]], "radius": 3}}
]}
```

## AI agents (MCP)

`solidcraft-cli mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server on
stdio. Headless it runs its own design session; with `--connect 127.0.0.1:PORT` it drives the app
started with `solidcraft --control PORT`, so you can watch the agent model. Tools:
`list_commands`, `execute`, `batch`, `inspect_design`, `measure`, `body_topology`,
`set_parameter`, `screenshot` (returns the image), `export`, `undo`, `redo`, `new_design`, `open`,
`save`. Tool calls reach files only in the open design's folder (when it lies inside your home
folder, never home itself or a hidden folder), SolidCraft's recovery folder and folders given with
`--allow-path DIR`. Claude Code: `claude mcp add solidcraft -- solidcraft-cli mcp`. Details, client
configuration and an example session: [docs/mcp.md](docs/mcp.md).

## Architecture

<!-- ANCHOR: architecture -->
| Layer | Crates | Role |
|---|---|---|
| L0 | `geom` | vectors, planes, profiles, meshes and their measures |
| L1 | `sketch`, `kernel`, `render` | constraint solver and profiles; the B-rep kernel boundary (truck); view math and the headless scene description |
| L2 | `doc` | parameters, expressions, the feature timeline and its incremental evaluation |
| L3 | `io` | design files, STL/OBJ/STEP/3MF export, STEP and 3MF/STL/OBJ/glTF import features |
| L4 | `engine` | the session and the command registry (everything is a command) |
| L5 | `gpu`, `ui-egui`, `mcp` | compute-shader evaluation of display tessellation plans; the swappable desktop front end; the MCP server (headless or bridged to the app) |
| apps | `solidcraft`, `solidcraft-cli` | desktop app, headless CLI |
<!-- ANCHOR_END: architecture -->

## Contributing

Coming from Autodesk Fusion? SolidCraft uses familiar parametric CAD workflows: constrained
sketches, solid features, an editable timeline, parameters and assemblies. See the
[roadmap](ROADMAP.md) for supported operations and current limitations.

See [CONTRIBUTING.md](CONTRIBUTING.md): how to build and test, the code map, the clean-room
and licence rules, and where help is wanted most.

## Licence

MIT OR Apache-2.0, at your option. Developed under the
[clean-room contribution rules](CLAUDE.md); third-party code and assets retain their
documented licences and attribution.

SolidCraft is independent. Behaviour is designed from public documentation, general CAD
conventions and user feedback. Reference tests use analytic values and open-source tools.
SolidCraft is not affiliated with, sponsored by or endorsed by Autodesk. Autodesk and
Fusion are trademarks of Autodesk, Inc.
