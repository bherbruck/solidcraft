# SolidCraft roadmap

Status as of **2026-10-14**. SolidCraft supports parametric modelling (sketch →
features → parametric timeline) as an open, pure-Rust, agent-drivable application.

## At a glance

| Question | Answer |
|---|---|
| Where are we? | **M0 and M13 done; M1, M2, M5–M7, M9, M10, M14 in progress.** Sketch with solver and the full SKETCH create/modify set; extrude, revolve (two sides, symmetric, to object), fillet/chamfer (variable radius, chord length, a radius per edge, corner patches), holes, threads (cosmetic and modelled), patterns, mirror, shell (inside/outside/both), draft, loft, sweep, coil, split body and split face, delete face, construction planes, axes and points; persistent naming of faces and edges; parameters and configurations; components, joints, motion studies, exploded views; sheet metal and a plastic enclosure subset; STEP/IGES/3MF/STL/OBJ/DXF; FreeCAD/Rhino import; autosave and crash recovery; desktop and browser (wasm) app, CLI, MCP server. |
| Features | Every registered command by area — [docs/features.md](docs/features.md) (`cargo xtask features`); registry breadth does not establish workflow or geometric depth |
| Reference tests | **82 / 82 parts match** — [docs/oracle.md](docs/oracle.md): public cases in `tests/oracle/` with analytic (61) or FreeCAD-computed (21) expected values, run by CI on every PR. |
| Tests | Solver, profiles, kernel booleans/blends/measures, expressions, timeline, persistent naming, file formats and migrations, assemblies, sheet metal, engine end-to-end, hostile-input fuzz over every command and headless UI scenarios. `cargo xtask ci` reports the current passing counts. |
| Gates | `cargo xtask ci`: fmt, clippy -D warnings, tests, asset attribution, layering, wasm32 build — green |
| Time to a useful alpha (M0–M6) | ≈ **82 agent hours** remain (sum of the M1–M6 estimates below) |

## Scope (owner decision, 2026-10-07)

The issue batch adds preservation of blind pockets in through cuts, associative projected
offsets and plastic face placements, XZ (+Y normal) with a v6 migration preserving older
designs, interactive angle placement, file-picker sketch imports, Home/recent controls,
toolbar overflow, and the remaining component-frame and assembly WIP work. The enclosure
acceptance scenarios now include board resizes; STEP reopening is automated. See [WIP reconciliation](docs/wip-reconciliation.md).
Face-based Extrude Join (including outward Press Pull) now defaults to the picked body,
so edge-touching assembly neighbours stay separate regardless of body order (#226).

In scope, in priority order:
1. SOLID and SKETCH (now).
2. Parameters / functional constraints (done: see `parameters.*`, `expr.evaluate`; unit-aware,
   cycles reported with their path, rename follows references, deletes list their users):
   user parameters with units and expressions (+-*/, ^,
   parentheses, sin/cos/sqrt/min/max/floor/ceil/round, unit conversion), model parameters
   d1, d2… for every dimension and feature input, references between parameters with a
   dependency graph and cycle detection, re-evaluation on change, comments, favourites,
   CSV/JSON import and export.
3. Components and assemblies with joints (M9).
4. SHEET METAL: flange, contour flange, bend, unfold/refold, flat pattern with DXF export, sheet
   metal rules (thickness, K-factor, bend radius) (M10).
5. A small PLASTIC subset for enclosures, after sheet metal: Boss, Rib, Web, Lip/Groove, Snap Fit.

Deferred until further notice: MESH, FORM (T-splines), SURFACE beyond what solids need (scope proposed in #772), PCB / 3D
PCB / Package, Render, Animation, Simulation, Manufacture/CAM, Drawings.

## Milestones

| # | Milestone | Status | Hours left |
|---|---|---|---|
| M0 | Vertical slice | done | — |
| M1 | Sketch depth: command coverage implemented — projection/intersect/include as linked geometry (auto-project on faces, lost-reference warnings, break link), full SKETCH create/modify set (slots, polygons, tangent arcs/circles, ellipse, fit/control splines, conic, text, trim/extend/break, fillet/chamfer, offset, mirror, patterns, move/scale, blend curve, centerline), constraints incl. curvature and polygon with over-constraint refusal, driven/arc-length/linear-diameter dimensions, AutoConstrain, DXF/SVG insert, 3D sketch curves (Project To Surface, Intersection Curve, Isoparametric Curve, Spun Profile), exact free-form profiles (B-spline/NURBS edges, any spline degree, curves cut by crossings), sweeps and pipes along 3D sketch curves, canvases and decals, curvature combs. Exact free-form Extend and certified associative offsets now preserve geometry and IDs through edits, deletion and reopen. Occurrence-frame 3D wires now store explicit coordinate spaces and retain points and analytic fits through moves, refresh, undo and reopen. Constructive sketch solving (Owen 1991, Fudos & Hoffmann 1997) ahead of the numerical solver: points and rigid clusters are placed on the branch the sketch had, so dimension edits and drags keep their solution; edits that would turn a sketch over are refused; drags re-solve only the sketch per frame. Remaining workflow repairs: mixed offset junctions, legacy natural-spline seams, and damaged legacy wire links lacking frame provenance | in progress | 4 |
| M2 | Robust modelling: coplanar booleans in the kernel, holes (placed on faces and on sketch points), threads (cosmetic, then modelled), patterns, mirror, shell, draft, split, construction geometry | in progress | 16 |
| M3 | General fillets and chamfers (curved edges, chains, vertex blends); saddle rolling-ball surfaces now use continuous seam sections and arc-length fitting with shared contact boundaries and bounded accuracy checks | in progress | 30 |
| M4 | Files: STEP import (done — [docs/step-import.md](docs/step-import.md)), FreeCAD/Rhino solids through cadmpeg (done, desktop and CLI), 3MF export and 3MF/STL import as mesh bodies (done), 3MF with B-rep face tags ([docs/formats/3mf-face-tags.md](docs/formats/3mf-face-tags.md), done), constant-section parametric mesh reconstruction, DXF sketches, OBJ import | in progress | 6 |
| M5 | Persistent naming of faces and edges (docs/book/src/dev/naming.md): faces named from their exact kernel-reported history (sketch curve sides, caps, revolve sections, primitive sides, blends, shell inner walls, pattern/mirror copies; split pieces told apart by the faces around them, lumps of a cut body named after their faces), edges by their faces; fillets/chamfers, sketches on faces, shell/draft/offset/replace faces, hole placements, joint origins and face appearances resolve by name first and warn when the entity split or vanished: done. sheet metal edges by base sketch line and flange side, shell inner faces, sweep and loft roles: done. kernel-reported face history: done (merged faces answer to every name; operations without history yet fall back to geometric names, flagged unverified; format 14 keeps older designs' geometric names). Left: history through sheet metal, plastic, surface and emboss features; FaceSource certificates through history | in progress | 20 |
| M6 | UI depth: selection feedback and origin (done), timeline editing (done: hover and selection highlight modifier faces, including fillets and chamfers; edit feature, history marker drag, dependency-checked single and multi-item reorder, suppress, rename, delete with dependents, re-resolved references; redefining selections refreshes persistent edge/face references while retaining feature identity and dimension names; optional compact named left feature history with rollback bar, multi-selection and persistent folders above the parts browser), right-docked dialogs (done), live previews for every feature dialog (done), keyboard-first commands with on-canvas value boxes and sketch inline dimensions (done), drag manipulators with automatic join/cut (done: extrude incl. two sides, fillet, shell, hole depth, move, pattern spacing, revolve rotator; taper handle open), dark theme (done), right-click menu with Repeat (done), measure tool (done), section analysis (done), selection filter (done), drag-solve of sketch points (done); appearance controls now preserve body/face/component targeting, named transparent looks, reuse/search and override removal; preferences | in progress | 6 |
| M7 | Thin extrude (walls along open or closed curves: centre, inside, outside or two sides; mitred corners, crossing walls joined; #721), sweep, loft (incl. to a point), rib and web (to next / depth), emboss (planar faces and exact radial emboss/deboss of bounded orthogonal profiles on complete exterior cylinders/sleeves), thread (cosmetic, and modelled M3–M20 outside and inside: the ISO profile screwed in as a faceted cut), coil (smooth B-spline helical walls), pipe, pattern on path, patterns of bodies and components, replace face (parallel), align, remove, feature copy/paste: done. Unique eligible cylindrical targets are inferred for new Emboss features. Left: general curved-face emboss and exact (unfaceted) modelled threads | in progress | 12 |
| M8 | Surface workspace (#772). Done: Patch (sketch regions; edge loops filled G0, plus `crown`), Stitch, Thicken (planes and cylinders), Extend (straight edges of planar surfaces), Trim (by a plane or a face), Delete Face (healing), Boundary Fill, zebra/draft/curvature-map analysis, Surface Extrude/Revolve/Sweep/Loft from open and closed curves, Offset, Unstitch and Reverse Normal; the continuity-driven core (`kernel/src/bridge.rs`, G0–G3 Hermite rows) with **Blend** (`surface.blend`, #764), **Match** (`surface.match`), **Boundary** (`surface.boundary`, #763 — two boundaries exact, three/four refused pending a Gregory-type corner), **Fair** (`surface.fair`, CV fairing), **continuity analysis** (`inspect.continuity`, #770; a toolbar tool with edge markers and a legend, and curvature combs along face isocurves) and a multi-section **loft** with per-end continuity and crown. Also in the Class-A pass: G1…G4 blend curves in the sketch, chord and variable-radius fillets on curved edges (with honest refusals where the fit cannot hold), and a G2 corner patch where filleted edges meet (kernel; the command refuses until the patch's sections can be sewn to the blends' contacts). Left: bias/form-factor fillet sections (design recorded), a Gregory-type corner, surface Fillet between separate surface bodies (#769), Ruled (#762), Untrim (#761), curved-face Extend (#760), loft guide rails (#201; sweep guide rails and two-rail sweeps are done, #771), and every remaining #757–#772 item | deferred | — |
| M9 | Components and assemblies: New Component / Create Components from Bodies, active component, per-component origin/bodies/sketches/timeline entries, occurrences with transforms, Copy/Paste vs Paste New, nested Browser tree with visibility and isolate; Ground, Move/Copy of occurrences, joints (rigid, revolute, slider, cylindrical, pin-slot, planar, ball) by joint origins with limits and Drive Joints, closed kinematic loops (four-bar, slider-crank: the loop's other joints follow the driven one on its branch, and driving stops where a loop can no longer close), cams (`joint.cam`: a revolute cam drives a slider or revolute follower along a closed sketch profile or the cam's edges, knife, roller or flat followers, worked out again when the profile changes; drives, studies, closed loops, contact sets), As-built Joint, Rigid Group, Contact Sets (driving and motion studies stop where bodies in a set would pass through each other), Motion Study (joint values keyed along a timeline, playback, export of occurrence positions as JSON/CSV), motion analysis (joint values and a point's path charted over a study as value, velocity and acceleration; the parts that collide along a study, with the steps where they do), Exploded views (automatic or by hand, shown without changing placements); interference, per-component physical properties; STEP with assembly structure; URDF export for robot simulators (links with meshes, mass and inertia; joint chains whose values match Drive Joints; docs/urdf.md). Physical properties now measure material-aware component/occurrence placements, descendants and repeated instances, including pending moves and body reassignment offsets; contact geometry and automatic exploded centres respect those offsets. Groundwork done: component tree, occurrences with transforms (instances, Paste New), world placement, active component as authoring frame, ground, Capture/Revert Position, moving bodies and sketches. UI: ASSEMBLE panel dialogs for New Component, Joint (snaps on faces, edges, circles and vertices; animated motion preview; offsets, flip, limits), As-built Joint, Joint Origin, Rigid Group, Drive Joints (live slider), Motion Link, Cam and Interference (overlaps shown red); joints in the timeline and the Browser | in progress | 32 |
| M10 | Sheet metal: flange (base, edge, contour), hem, unfold/refold, convert, flat pattern + DXF, rules: done. PLASTIC enclosure subset: Boss, Rib, Web, Lip/Groove, Snap Fit, Rest, Plastic Rules (library, edit, assign; rule draft and clearance as feature defaults): done, tested against hand-computed geometry. UI: SHEET METAL and PLASTIC tabs in Fusion's layout with dialogs (live preview, Edit Feature, pre-selection) for Flange, Hem, Unfold/Refold, Convert, Sheet Metal Rules, Create Flat Pattern (flat view with bend lines, bend table CSV/JSON and DXF export with exact circle/arc/line cutouts), Boss, Lip/Groove, Snap Fit, Rest, Manage and Assign Plastic Rules; Fold (bend along a sketch line); flanges meeting at a corner closed at any angle with the rule's gap; edge flanges with their own angle and height per edge in one feature; corner reliefs (notch, round, rectangular, tear) chosen by the rule; Convert to Sheet Metal from a solid or shelled body by its bend edges (flat faces, sharp bends) | in progress | 36 |
| M11 | 2D drawings | deferred | — |
| M12 | Release: cross-platform packages, verification and deliberate publication implemented; original current-UI demo refreshed. Trusted distribution signing and broader performance work remain | in progress | 16 |
| M14 | Configurations: a table of rows (parameter expressions and feature suppressions per row) switched with config.activate (undoable, all or nothing), kept through parameter renames and feature deletes (config.configure, config.table, config.column/row/activate). Per-row body materials and appearances (library looks, custom colour/opacity and clearing overrides), undoable activation and save/reopen: done. Ordered typed configuration rules, atomic validation and structured rule editing: done. Component appearance and certified semantic face style columns, atomic preview/apply and independent-copy remapping: done. Exact constructed sketch-side and immutable imported-face lineage now preserve legacy names and reject retargeting. Left: ambiguous shifted/two-sided caps | in progress | 4 |
| M13 | Never lose work: atomic saves (temporary file, flush, rename; a crash at any moment leaves the old file or the new one) with the replaced version kept as `<file>.bak`; autosave of unsaved changes to the recovery folder every 5 minutes (preference) and after big operations, only when something changed; "Recover unsaved design?" on the next launch (`doc.recovery_list`, `doc.recover`, `doc.recovery_discard`); a kill-mid-save test (process killed while saving a 0.5 MB design 25 times). File format: `"format": "solidcraft/12"`, older formats upgraded step by step (`doc::format`), newer ones refused with a message, out-of-range numbers and broken placements refused; a corpus of designs saved by M0, components-era, joints-era and current builds (apps/solidcraft-cli/tests/corpus) that must open and evaluate as saved; damaged and hostile variants (truncation, bit flips, hostile values, removed keys, repeated features, deep nesting) never panic | done | 6 |

M6 navigation preferences include **Touchpad**: Shift + pointer movement pans, Alt/Option +
pointer movement orbits without a mouse button, and vertical scrolling zooms. Other presets
retain their saved two-finger scroll preference; camera motion is available through
`view.navigate` and modifier-aware `ui.move` automation.

M11 precursor: File → Export Current View as DXF exports shown placed bodies from the current
camera as a 2D millimetre drawing, with hidden edges omitted or dashed. Vector occlusion splits
partly hidden edges and includes curved silhouettes. Repeated part meshes and triangle
hierarchies are shared; bounded desktop preparation and occlusion use up to eight workers.
Curves are approximated; drawing sheets,
annotations and exact projected curves remain deferred. See [view DXF](docs/view-dxf.md).

Desktop view DXF export runs in the file-operation worker with five-stage progress and
cooperative cancellation. Curve tolerance (0.0001–1 mm, default 0.01 mm) is remembered alongside
the hidden-edge setting. Cancelled exports preserve the model and the previous destination;
completed drawings publish atomically. The browser still exports synchronously.

## Interactive modelling

- Thread resolves a suggested metric size when a cylindrical face is picked. Diameter and
  pitch selectors expose the existing metric designation parameter; custom designations stay
  editable, and Automatic size returns to the face's suggestion. Cosmetic/modelled and
  full/partial-length options remain available. This does not add tolerance classes or
  left-hand threads.
- New Body extrusion and face-offset previews show completed intermediate geometry while a
  drag continues, then catch up to the exact final value. Changes to the selected geometry,
  operation or feature still discard obsolete results. Distance arrows and their value boxes
  follow the current frame's input.

## Physical material data

- Physical material card validation (#923/#922) provides typed SI mechanical and
  thermal data, per-property sources/conditions and isotropic/orthotropic stability
  checks through read-only `material.properties.validate`. It does not assign or
  persist properties, provide a solver or enable rendering; see
  [the contract and staged follow-ups](docs/physical-material-cards.md).

- Directional elasticity (#994) queries the validated card in material-local axes
  through read-only `material.properties.directional`, with source/condition review.
  It does not assign body frames, predict structural behavior or enable a solver.

## Library foundations (#785, #786, #787)

- Local scalar libraries (#846) inspect/export bounded portable catalogs and copy exact
  selected appearance/material revisions into existing design definitions in one undo step.
  Appearance offers local loading/search/export; physical materials use existing commands.
  This slice is independent of the render stack and does not change the document format.
  See [the contract and limits](docs/book/src/dev/local-libraries.md).

- Proposed phases cover offline bundled/project/user packs, physical materials and
  appearances, editable wood mapping, and parameterized component/assembly families.
  Stable identity/revision, copied design definitions, provenance and resource limits
  apply from the first implementation. Existing material, appearance and standard-part
  commands remain the application paths. See [the proposal](docs/book/src/dev/libraries.md)
  for source/license review and acceptance gates. This is planning only: pack loading,
  texture rendering and expanded component families are not implemented by this entry.

## Render foundation

- Explicit texture normalization (#1218) produces completed float RGBA with
  role-aware linear color/data semantics, bounded matrix/TRC ICC conversion and
  opt-in EXIF orientation. It retains high-precision raw samples and records lossy
  gamut clipping only when explicitly requested. It is not a renderer/resource
  consumer; see [the supported policy and limits](docs/render-texture-normalization.md).

- Opt-in opaque PBR (#1046) connects persisted surface parameters to the integrated
  product viewport and CPU PNG, using a fixed direct-light studio. Geometry and mass
  stay unchanged. HDRI, transmission and textures remain separate gates.
  See [the consumer and limitations](docs/render-opaque-pbr.md).

- Combined integration is tracked in #842, with independent scalar libraries in
  #863 and richer PBR/material-card transport in #1086. See [phase boundaries and validation gates](docs/render-integration.md).
  Render/library consumers use the merged asynchronous picker (#1127). Material-card
  UI (#1094), texture mapping and HDRI lighting remain explicit follow-ups.

- R4a adds bounded CPU product-image jobs, cancellation and atomic PNG export through
  engine commands, the Render panel, CLI and persistent MCP. Immutable captures and
  results stay outside persistence/undo; native workers and cooperative browser tasks
  share the same work protocol. See [jobs, limits and export](docs/render-jobs.md).

- R3 adds an overlay-free product scene and the integrated Render workspace, with
  runtime GPU host capabilities, per-document drafts and named camera/background
  controls. Design keeps its camera and drawing policy. Raster transparency remains
  approximate; PBR and environments are later slices. See
  [workspace and scene contracts](docs/render-workspace.md).

- R2 adds bounded named render setups in document format 21 and the read/save
  `render.settings.get/set` commands. Proven render-only changes preserve geometry
  and prepared world data through undo/redo and checkpoint restore. This slice does
  not add a Render workspace, PBR, environments or image jobs; see
  [settings and refresh policy](docs/render-settings.md).
- Bounded PNG/JPEG texture admission (#900) validates in-memory sources and returns
  completed RGBA8 pixels with explicit color/data roles. It is an input utility only:
  ICC/16-bit normalization, local wood mapping, resource persistence and the renderer
  consumer remain separate work. See [limits and licensed-file probes](docs/render-texture-input.md).

- The initial opaque surface numerical contract for #1004 provides bounded linear
  color/metallic/roughness/IOR admission and an original tested direct-light BRDF.
  The numerical module alone advertises no capabilities; the installed consumer above
  provides rendering. See
  [the model, evidence and remaining consumer gates](docs/render-surface.md).

- A bounded cooperative Radiance RGBE input utility is available in L1 render;
  it does not yet enable HDRI/PBR rendering or a document import command. See
  [the input contract](docs/render-hdr-input.md) and #813 for integration work.
- R1 adds renderer-independent backend/profile metadata, strict request resolution and
  bounded image/scratch admission checks, exposed by the read-only `render.backends`
  command. It describes the current CPU raster snapshot path without evaluating the model.
  The legacy Design snapshot route remains unchanged. Later slices above provide the
  workspace/jobs and opaque PBR; HDRI and optional engines remain future work. See [the contract](docs/render-backends.md)
  and [the phased proposal (#620)](https://github.com/bherbruck/solidcraft/issues/620).

## Dimensioning and error reporting

- MCP `execute` accepts `verbose: false`, sharing the compact reply helper used by
  `batch`: body changes and created sketch IDs stay, while repeated lists are omitted.
  Both tools retain full replies by default.

- Sketch Create groups drawing variants under Line, Rectangle, Circle, Arc, Slot,
  Polygon and Spline. Each open part remembers its last interactive choice per family
  across sketch sessions and document-tab switches; closing the part resets these UI choices.
- Inscribed, circumscribed and edge polygon tools preview their actual outline before
  placement, including orientation and the selected side count.
- Construction (X) preserves the active sketch drawing tool, picked anchors and inline
  dimension values, so line type can change while a shape is in progress.
- Planar sketch construction previews and hover feedback keep the same dashed pattern as
  committed geometry; centerlines retain their dash-dot pattern.

- Point has a default Sketch CREATE toolbar button alongside the existing drawing tools.
  Saved custom layouts are preserved; Reset Toolbar restores the default Point button.
- Construction planes cannot take an origin-plane name (XY, XZ, YZ, Top, Front, Right). Plane
  arguments reject such a name when an older design's construction plane already has it. Use
  the other alias, the construction feature's numeric ID or an explicit plane; existing
  stored references keep their saved meaning. Section Analysis uses the same check.
- Sketch text can be edited from a letter interior or outline's context menu or the Edit Text command.
  Interior selection requires every profile boundary to belong to the same text source;
  the editor highlights the whole source without replacing profile selections.
  Content, baseline X/Y, height and rotation update the original text source in one undo step;
  cancelling or rejecting an invalid value preserves the sketch. Position and height fields
  use the document units captured when editing starts; generated outlines remain associated
  with their text source, and untouched inputs preserve their saved internal values.
  Dragging a text outline moves the entire source in one undo step; Escape restores the
  gesture. Move/Copy on a selected text outline provides unit-aware X/Y distances and rotation
  about its baseline; copies keep an independent editable text source.
- Repeating the initial point of a planar multi-point sketch drawing tool is neutral: the tool
  waits for a distinct point without a degenerate-shape error or history entry. Typed input
  and programmatic commands retain their validation.
- Circular-pattern and angled-plane axes reject explicitly supplied malformed or expression
  origins instead of silently rotating about zero. Numeric origins remain in millimetres;
  associative axis expressions and construction-axis references remain unsupported.
- Single-line and rectangle-side dimensioning supports selecting a line or its two endpoints.
  Type a value and press Enter to commit one undo step; rejected values remain editable and
  cancellation leaves the sketch unchanged. Fit keeps geometry clear of the Sketch Palette
  and navigation controls, including enlarged displays and nested pending placements.
- Sketch annotations, live drawing lengths and Measure use document units. Inches show
  three decimals and retain a meaningful fourth digit. Bare length dimension entry captures
  the current design unit; changing units leaves existing geometry and expressions intact.
  Component/canvas distance controls, sketch cursor coordinates, flat-pattern dimensions
  and pocket depths, interference volumes and parts-library lengths also use design units.
- Curvature and Polygon constraints accept complete preselected operands. Fix/Unfix
  consumes complete point/curve selections consistently; mixed or ambiguous selections
  retain the picking workflow. Rejected constraints preserve selection and undo history.
- Contextual error reporting: a **Create Issue** action beside feature errors opens a
  reviewable GitHub issue draft with the error, operation settings, build diagnostics and
  useful reproduction context. The draft is editable, with optional capture of the actual
  viewport and copy/save controls. Opening GitHub does not submit the issue. Help also exposes
  reporting for unexpected results that did not produce an error.
- The header exposes **Report a bug**, including an accessible compact control on
  narrow windows. It opens the same editable report and screenshot review without
  changing the active model or a pending dimension.
- Crash reports: every panic (in a command, in the kernel, anywhere else) and every unclean
  exit (abort, stack overflow, kill: found at the next start through a locked per-run command
  journal) leaves a local report with the backtrace, version and commit, the command and the
  design that reproduce it (`solidcraft-cli crashes replay <id>`). Repeats are counted, not
  rewritten; at most 20 reports, 50 MB and 90 days. The app shows them after a crash (Help >
  Crash Reports…) with a reviewed issue draft; agents read them through
  `diagnostics.crash_reports`, the MCP `crash_reports` tool or the CLI. Nothing is sent
  anywhere. See [crash reports](docs/crash-reports.md).
- Agent chat (off by default, desktop): a side panel that drives a coding agent the user
  already has (Claude Code through its ACP adapter, Codex, Gemini CLI, OpenCode, Grok Build, …) over the
  Agent Client Protocol, under the user's own login; SolidCraft stores no keys. The agent works
  through SolidCraft's MCP tools on a gated endpoint of its own: file actions wait for Allow or
  Deny, crash reports are off limits, 24 changes per message, one undo step per message, and
  the agent's whole process group ends with the panel or the app. Docks left or right,
  minimises to a strip. Grok Build discovery includes `~/.grok/bin` and reuses the user's
  Grok login. See [agent chat](docs/agent-chat.md).
- Command scripts from designs (#886): `timeline.export_script` (CLI `solidcraft-cli script`)
  writes a design's history as a script that `eval` and `run` replay into the same design —
  user parameters, units, sheet metal and plastic rules and materials as their own commands,
  then each feature through `timeline.add` with its ids, names and dimension parameters kept,
  so every feature kind replays exactly and a failing feature fails the same way. Trimming the
  script after a step gives the design at that step. Appearances, folders, views and
  configurations are listed as left out; components and joints are not supported yet.

## 0.2.0 release batch

- Includes the new axis navigation, adaptive sketch drawing/picking, origin-corner repairs,
  durable supported profile/face references, endpoint dimensioning and contextual reporting.
- Smooth ruled-wall display joins are hidden only within the supported geometric certificate;
  authored splits and actual creases remain protected. Wireframe retains physical edges.
- Paste New uses a bounded current-geometry comparison for supported convex line/circle
  prisms with separated circular holes; unsupported shapes retain the existing strict check.
- Common-bore Union retries check additional boundary curves before recutting the bore,
  preventing the reproduced thin-web loss. Unknown additional trims refuse; inherited
  common-cylinder trims retain their existing policy, so this is not a general Boolean proof.
- Exact full-path assembly joint editing remains separate work and is not part of this batch.
- Stable 0.2.0 is published: all release payload checksums, native cold reopening, deployed
  web app and both Flatpak architectures were verified. The replacement README demo uses
  the original electronics enclosure story and animated pointer in the actual English
  0.2.0 UI, at 4× playback. Publication follows [the release workflow](docs/releasing.md).

## 0.2.1 critical fixes

- Axis view and navigation controls stay anchored to the viewport when feature dialogs or
  the command palette open, at normal and enlarged display scales. Fit still accounts for
  dialog coverage; covered controls do not receive clicks through dialogs.
- Eligible planar faces survive fillet, chamfer and shell operations with exact source
  certificates. Sketches placed directly on the enclosure wall and inner floor follow
  width and wall changes, including after the USB cut and cold reopening.
- Remove artificial straight-wall and rounded-corner subdivisions left by shell and
  extrusion cuts. Identical generated wall segments span adjacent slabs as one physical
  face, with exact full-height supports and boundary stitching. Preserve authored face
  splits and face appearances. Permanent regressions cover material, STEP, reopening and
  unaffected bodies across the extrusion end plane.
- Feature dialogs use the available screen height for their fields, with OK and Cancel
  outside the scrolling region, including enlarged display scales.
- Design format 13 records shell-face attachments. Older designs remain readable;
  newly saved designs require 0.2.1 or later. Ambiguous or unsupported faces still refuse
  attachment rather than guessing a plane.
- Saved projections from a removed strip retain their geometry and report the lost
  reference. Explicitly reselect the wall to repair that projection.

## View navigation

- Desktop Ctrl/Cmd +/- and 0 change or reset the saved UI scale. The scale slider keeps
  its layout stable during a drag and applies the selected scale on release.

- OpenSCAD navigation preset: left drag orbits, right/middle drag pans, Shift+right/middle
  drag zooms, and the wheel zooms. Available in the shared desktop/web Navigation preferences.
- Compact clickable signed X/Y/Z axis tips, smooth drag-to-orbit, and Shift-orbit snapping
  to 26 standard and diagonal views. Pointer motion accumulates independently of a lightly
  damped spring, with a boundary margin to prevent flicker. Z-up/Y-up and the nearest square
  top/bottom heading are preserved. Diagonals include 45-degree edge and isometric corner views.

## Known limitations

- Scheduled and manual CI scan locked dependency advisories, bans and sources with
  cargo-deny. Vulnerabilities fail the scan; unmaintained crates warn. Local vendored
  patches require separate upstream review because advisory databases cannot version-match
  their path dependencies. Pull requests retain the existing licence checks.

- Associative shell-face sketches require a unique certified planar source. Nested shell
  attachments and ambiguous coincident sources remain unsupported.
- G2 fillets: exact quintic blends of straight edges between planar faces, with one perpendicular
  planar end face at each end. The UI and `solid.fillet` expose `curvature` (alias `g2`); the size
  is an equivalent circular setback. Curved faces/edges, sloped ends and corner junctions remain unsupported.
- Fillets and chamfers: constant, variable-radius and chord-length fillets; a radius per edge,
  rounds next to earlier rounds of another radius, and smooth corner patches where three rounds
  meet (a sphere for equal radii); perpendicular corner fillets after a closed ring of earlier
  rounds preserve the ring's radii and the enclosed planar face (#1557);
  straight edges (convex or concave) between planar faces with perpendicular
  planar end faces (equal-distance chamfers also meet sloped planar end faces, including inner
  basin rims within the rim width); convex fillets also meet sloped planar ends and reflex
  corners around pocket rims, with unequal wall slopes and extensions confined to the face;
  connected equal-distance chamfers also trim each other across unequal sloped rim walls;
  curved chamfer chains with straight planar ends can terminate across two existing planar
  bevel faces, splitting their shared seam; picks on the same tangent chain are applied once (#1303);
  whole smooth loops of a planar face, picked whole or by one edge (tangent
  chain), including circles and walls at any angle (boss bases and tops, bores, pocket floors,
  plate outlines), loops with sharp corners between straight or mixed arc/line edges (straight
  blends meet in mitres; perpendicular curved walls use offset intersections, with D-rim removal
  checked within 0.1% of analytic offset slices), runs of
  neighbouring edges along a planar face's loop (two top edges of a box: mitred, ending square;
  with the edge between them too, the corner becomes a sphere octant),
  rounded corners rounded again (sphere octants, tori at concave corners); every edge of a
  convex planar body or of an extruded part; fillets inside patterns; closed chains of curved
  edges between any two smooth sides (a branch pipe on a main pipe), by a rolling ball (the
  blend a rational B-spline through exact arcs), closed or ending at planar faces square to the
  edges, where straight or curved cap edges are trimmed to the contacts (the straight edge
  between two overlapping extruded arcs); knife edges, where the two faces have opposite
  normals (a wall that runs out tangent into a face: the ball that fits there); an edge that
  runs out along an end face tangent to one side (the arc top edge of that wall), cut where the
  blend meets it; rounds that end in an earlier round on curved faces, in any order (the rounds
  of two edges at a vertex meet in a crease, as on a box), picked together or one after the
  other, except a round after a round of the edge between the wall and a flat face that it
  would run over; the three edges at a vertex of any smooth faces, of one radius, end in a
  piece of sphere. A limiting blend that collapses a side wall into coincident boundary
  curves is refused as too large rather than returning an unmeshable face (#1113).
- Booleans: prisms along one direction (plates with holes, slots, patterns of them, equal
  sections stacked end to end) are combined exactly in 2D. Qualified axial cuts preserve
  annular holes with overrun cutters, including inward Press Pull after a joined extrusion;
  curved wall qualification rejects false transverse axes instead of filling holes.
  Bodies touching along one whole face
  (a part and its mirror image) are stitched; coincident planar faces are pushed apart when
  their neighbours are planes or cylinders. Other coincident curved configurations can still
  fail. Bounded containment classification preserves fully internal cavities and nested
  material islands independently of shell order, with STEP round-trip and material-membership
  regressions. Near-contact curved shells use the general boolean path rather than a
  containment shortcut. Spheres are built from six pole-free patches.
- Mass properties come from fine tessellation (curved faces within ~1e-4 relative); triangles
  that cut through curved faces (trimmed faces after booleans) are split onto the surface first.
- STEP import (our own reader, crates/kernel/src/step_in) reads solids (B-rep with analytic and B-spline geometry), units,
  product names, colours and assemblies; analytic offsets on planes, cylinders, cones, spheres
  and regular tori are read exactly (signed/nested distances, units and face sense). Freeform
  offset surfaces and pcurve-only edges are not read yet,
  and assembly components are flattened into bodies. Closed periodic faces (torus, B-spline
  bands) are split in two for meshing; measure() uses a finer tolerance for small radii.
- Persistent naming follows the kernel's exact face history (docs/book/src/dev/naming.md):
  references resolve by name first, follow splits and merges, and warn when the entity split,
  merged or vanished, or was found by geometry only (operations that report no history yet).
  New planar sketch attachments use exact box, extrusion cap/side and immutable imported-face
  source certificates with checked target and author occurrence paths. A missing or changed
  source fails explicitly; Redefine Sketch Plane previews reattachment or a fixed plane while
  retaining current geometry and dimension identities. Unsupported modified faces need an
  explicit fixed plane. Legacy attachments retain their original resolver behavior.
- Modelled threads are cut as a faceted body (curved faces within 1/200 of the pitch); Extrude
  and Revolve To Object on a curved face cut the tool back to the target's surface.
- Explicit sketch breaks preserve exact ellipses, fit/control splines (including high degrees)
  and conics; analytic trim/break also sees free-form cutters. Free-form curves cut at a tangent
  touch (no clean crossing) enter that profile as polylines.
  Sweeps along 3D sketch curves follow their polyline (smooth walls when the path is smooth).
  A sweep with a twist, an end scale or a parallel or direction orientation (#771) follows its
  path that way too, even along sketch lines and arcs: B-spline walls through stations, ruled
  walls through denser stations where the path has corners or the profile has holes.
  Guide rails (one or two, #771) do the same: each station meets every rail once, and the rail
  turns the profile and scales it (or stretches it across to the rail) against the start; two
  rails carry it between them. Rails along a path with sharp corners are refused for now.
- Extend intersects exact ellipses and rational/high-degree splines without moving the line or
  circular support. Associative free-form offsets have a whole-span 0.002 mm normal-distance
  certificate and refine within bounded node limits while retaining existing IDs.
  At the node limit, geometric certificate subdivisions accommodate unequal spline parameter
  speeds without changing stored nodes or relaxing the distance tolerance (#971).
  Associated offset fit nodes stay hidden from display, picking and snapping; dragging the
  editable source updates the derived boundary live. Break Association restores point editing.
  Singular, self-intersecting and ambiguous curves refuse atomically; mixed junctions and nonsmooth
  closed natural-spline seams remain limitations. New explicitly closed fit splines and closed
  offset outputs use periodic C2 interpolation, supporting repeated certified offsets; older
  natural splines are preserved. Clicking the starting fit point deliberately closes a new loop.
  Newly captured adaptive profile references with complete source coverage retain their outer
  and hole identities through span-count changes, including parameter edits after extrusion.
  Partial, intersected or ambiguous boundaries do not qualify for these whole-source references.
  Legacy curve-span multisets remain exact and unchanged; already-lost selections may need
  reselection. Format 12 stores the new bounded whole-boundary references.
  Projected analytic boundaries follow nested loop identities through resizing, including
  stationary holes and rounded corners. Supported convex, tangent line-and-arc boundaries
  can lose or regain corner arcs during an inset while preserving reserved curve identities
  and linked profile selections. Format 12 saves those associations; compatible older closed
  offsets upgrade structurally without changing their curve IDs. Breaking an association
  freezes its current boundary and materializes downstream profile references. Child offsets
  of a boundary with retired arcs still require breaking its association first; mixed or
  nonconvex junctions and retained references to disappearing individual arcs remain limited.
- Extrude and Revolve editors retain original direction, extent expressions, explicit targets,
  axes, From/To inputs and All-profile intent. No-op edits preserve downstream dimension names
  and selected occurrence frames; cancel and successful edits preserve one-step undo history.
  Placed feature edits preview through the edited timeline entry, and cylindrical Emboss guides
  use the sketch's actual target, author or uniquely placed third-component frame.
  Revolve rotators follow the actual signed axis and support symmetric and two-sided extents;
  dragging a To Object extent converts its derived value to an explicit angle. Legacy reversed-axis
  symmetric geometry is preserved through format 11's explicit legacy convention. New revolves
  use the sketch-normal direction for each selected profile, including To Object and curved
  target trimming, independently of the selected axis sign.
- Cylindrical emboss/deboss wraps bounded orthogonal line profiles, including holes, concave
  outlines and nested islands, onto complete exterior cylinders or coaxial sleeves with exact
  radial walls. New features infer a unique eligible cylinder when the face field is empty;
  ambiguous targets require an explicit face. Saved target
  and author occurrence paths retain the chosen placements; missing/split faces refuse rather
  than retarget. General free-form profiles, seams, stepped cylinders and other curved surfaces
  remain unsupported. STEP round-trip and independent radial/volume checks cover this subset.
- Shell and offset faces work on bodies of planes, cylinders, cones and spheres (convex or not:
  filleted boxes, bosses, domes, countersinks; corners where four or more planes meet, such as a
  floor falling to a drain, as long as the corner's edges are all convex or all concave); draft on convex planar bodies and on walls (planes, cylinders → cones) between caps; lofts through three or more sections are smooth (B-spline sides through every section), two
  sections ruled.

- Sketch corner tools: fillet and all three chamfer modes pick a connected corner even when
  its endpoint overlaps the fixed origin or a loose point. Ordinary drawing still snaps to
  the origin; clicked UI regressions verify geometry, closed profiles and undo.
- Sketch display: drawing, picking, highlighting, tool previews and profile shading share
  adaptive projected curve boundaries. Fit, zoom, display scale and placed sketch frames
  invalidate the display cache without changing CAD geometry or timeline history. Work
  budgets and clipped paths prevent unbounded sampling or invented edges across the eye plane.
  The Sketch Palette scrolls its options on short windows and enlarged displays while
  keeping constraint status and Finish Sketch visible.
- Face projection: closed boundaries have a deterministic outer-first order. Refresh matches
  existing geometry before assigning positions, retaining saved curve IDs and constraints
  across enumeration changes and resize, including concentric circles. Ambiguous identities
  retain the last geometry and report a lost reference; permutation matching is bounded.
- Project accepts complete supported preselection in one undo step. Visible external sketch
  curves and points retain their source sketch identity; model geometry, axes and planes use
  the existing linked projection commands. Rejection preserves the document and selection;
  mixed selections retain ordinary tool-first picking.
- Inline sketch dimensions: Tab and Shift+Tab cycle within the active shape's value boxes,
  selecting visited values for replacement. Expression completion retains forward-Tab
  priority; real keyboard regressions cover rectangles, lines and single-value circles.
  Typed lengths, radii, line angles, signed arc sweeps and slot lengths persist as constraints
  and expressions through save/reopen. Shape creation and its dimensions form one undo step;
  rejected values keep the editable field focused.
- Repeated components: Browser selection, visibility, properties, copy and Move/Copy use
  checked occurrence paths. Active nested occurrences author planar sketches and solids in
  their selected frame. Pending moves compose through parents; dialog cancellation restores
  its selected move. Source materials and body/face appearances follow transformed instances.
  Shared nested definitions remain shared. Deep Paste New copies reachable definition graphs,
  independent feature/input histories, styles and internal motion records, with bounded geometry
  checks and explicit external-link notices. Paste New selects the copy while preserving the active
  authoring context. Imported periodic STEP seams survive placement and reexport with shared
  topology and appearances intact. Recorded body transfers preserve historical Join/Cut membership,
  material, placements and machining data through edits, copying, undo and reopen. New 3D wires
  record their coordinate frame; legacy wires migrate from known provenance without changing
  coordinates. Broken legacy links with missing provenance can require explicit refresh or relink.
  See [occurrence workflows](docs/occurrence-workflows.md).
- Toolbar drops on panel labels, gaps and margins append commands with insertion feedback;
  button drops keep insert-before ordering.
- Parameters and Configurations have distinct actions in the SOLID toolbar's CONFIGURE panel.
  Parameters opens the user/model variable editor; Configurations opens the configuration table.
  Configure remains available in the panel dropdown.
- Parameter creation supports Enter with validation and continued keyboard entry. Shared
  expression autocomplete supports Up/Down selection, Enter/Tab acceptance and Escape dismissal;
  accepting a suggestion does not submit a parameter or trigger viewport shortcuts.
- Movable window title bars follow the app's dark/light header palette. Radial right-click
  menus are opt-in in Preferences > Navigation; ordinary context menus remain the default.
- Desktop distribution: running Windows windows and executable resources use the app icon;
  packaging validates icon resources. Nix installs launchers, MIME data and generated icons,
  with explicit launch apps and Vulkan-loader/driver discovery. Flatpak feed instructions
  register the required runtime source; release checks exercise desktop modelling on X11/Wayland.
- OpenGL rendering clamps effective anti-aliasing to 1x across UI pipelines, textures and CAD
  callbacks to prevent black frames. Other backends retain the requested sample count, and
  stored preferences remain unchanged.
- Configuration rules: ordered typed conditions can drive parameters and suppressions or
  restrict values. Preview explains effective values without modifying the document; applying
  rules validates rows atomically. Length expressions retain their authoring units across
  document-unit changes. The UI supports structured All/Any/Not conditions and rule ordering.
  Component styles and certified semantic face styles are available; unsupported lineage refuses
  atomically, and stale certified face paint cannot jump to another surface. See [configurations](docs/configurations.md).
- Sheet-metal material: inherited holes, crossing edge notches, overlapping cuts and retained
  islands share the same boundaries in flat preview, DXF and unfolded solids. Detached material
  pieces survive unfold/refold as one logical sheet group, including split flanged sheets.
  Cut-outs made in the flat pattern cross bends at any angle or curved: their edges fold to
  helices and curves on the bend, their walls square to the sheet. Free-form bases use bounded
  sampling.
  Parallel prismatic partial-depth cuts retain pocket side and depth through transfer and
  unfold/refold, with separate pocket contours in preview and DXF. Planar Intersect preserves
  exact holes and retained islands, including the union of multiple selected profiles, and
  records complementary depth layers as pockets while retaining the stock gauge. Unsupported
  tapered or bend-crossing machining refuses atomically instead of restoring material later.
  Sketches attach to a sheet's flat faces (base and flanges, either side) by panel, in the
  flat pattern's coordinates, and follow it through rebuilds and unfold/refold.
  A flange on another flange's far edge (a return) follows that flange's closed end and keeps
  clear of the flange beside it, and the returns of one feature are mitred toward each other
  with the rule's gap where their flanges meet at a corner; returns of separate features, and
  flanges on returns, are not closed toward each other.
- Rolling-ball fillet accuracy: independent offset-cylinder tee references check surface
  distance, actual trimmed tessellation, supporting-cylinder contacts and tangency at two
  scales. The measured trimmed-part error is below 0.0001 mm; this is an analytic geometry
  check. Under-resolved fits return an error.
- Contact driving checks the trajectory between clear endpoints, including linked joints,
  playback and sparse motion exports. Checks use bounded tessellation samples, not exact
  continuous collision detection; local thin walls and small details remain limitations.
  See [contact-set limits and recovery](docs/contact-sets.md).

## 3D sketch bridges

3D sketch drawing snaps to points on other visible sketch planes without flattening them onto
the active plane. Bridge endpoints capture their source positions; associative endpoint links,
construction-plane selection of 3D wires and loft guide rails remain separate work.
Feature dialog header chevrons collapse and restore the form without discarding selections
or edited values, including the Appearance panel.
## Construction on 3D paths

Plane Along Path has a viewport path selector, fractional position, live preview and timeline
editing. Construction planes and points resolve drawn 3D wires without applying their sketch
plane a second time. Curved paths use sampled segments; multi-curve chains, absolute distance
and reference-object placement remain separate work, as do loft rails and centerlines.

## Design properties (#1015)

- Designs carry project, part number, revision, author, organization, description and licence
  in an optional `metadata` object (File → Design Properties…, `document.metadata.*`,
  `inspect_design`). STEP and IGES headers and 3MF model metadata take the fields their format
  has a place for. Other tools keep their own data under a reverse-DNS key, kept as it is
  through open, edit and save and never exported. See [design properties](docs/book/src/guide/files.md#design-properties).
  Per-component properties (a part number per component) can follow with the same type.

## Document-opening performance

- Desktop STEP import prepares consecutive independent solids in bounded batches with up
  to eight workers, retaining serial traversal, result order and persistent source identities.
  The web path remains serial. Original multi-solid fixtures show 64–81% lower complete
  import times; single-solid assembly parts are not scheduled across nodes. See
  [measurements and limits](docs/perf.md#parallel-step-body-preparation-2026-10-12).
- Design containers (#303, phase one) separate and deduplicate STEP, raw images and
  lossless binary mesh arrays while keeping plain JSON as a supported Save As/export
  format. Generated-mesh storage measurements reduce serialization and decoding work;
  a seekable reader defers original blobs until after provisional display.
  See [container format](docs/design-container.md) and [performance measurements](docs/perf.md).
- Warm saves with explicit `cache: true` can include a versioned native B-rep cache through SolidCraft-owned topology
  and geometry records, with a separate truck adapter and bounded binary reader. Exact
  face-source certificates and operation history survive codec round trips. A provisional
  picture can be generated before original blobs are read; editing and measurement wait
  for authoritative reconstruction. Cached metadata binds preview and reconstruction to
  the same document despite concurrent file writes. Default saves omit this cache and never force cold
  evaluation or meshing; explicit `preview: true` remains a separate warm-mesh option.
  Keys, resource budgets and document/revision guards reject incompatible, hostile or
  stale results. A second modelling kernel still requires its own tested adapter.
- Warm saves with explicit `cache: true` can also carry a bounded PNG thumbnail from already cached meshes.
  Home reads it without rebuilding the design when its local picture is missing;
  mismatched or malformed thumbnails fall back to normal generation. The image is
  kernel-independent and saving never forces cold meshing to produce it.

- Cold rebuild profiling separates parsing, geometry, placement, display meshing and
  thumbnails. Straight-line closest-point projection and exact curved-surface evaluation
  reuse reduce repeated work without relaxing geometry checks. Three alternating optimized
  benchmark runs reduce median rebuild time by 26% on the public sensor enclosure and 22%
  on a local case assembly; shape measurements and topology match the original build.
  See [performance measurements](docs/perf.md). Further cold Boolean and import costs remain.
- Prepared assembly geometry survives selection and view changes while real geometry and
  placement changes invalidate it. A Pi assembly edge-selection probe drops from 9.24 s to
  0.57 s; initial opening is unchanged. Actual Windows desktop profiling measures 15.87 s
  median opening, with 82% spent rebuilding geometry; halving full opening remains open.

## Kernel

- Successful Boolean retries report their strategy and numerical adjustments as
  feature warnings and structured inspection data. Diagnostics survive cached
  evaluation, undo/redo and cold reopening; the robustness runner counts retry
  events by strategy and reports any bounded-log truncation. Existing geometry
  acceptance checks remain in force.

- Cylinders of one radius whose axes cross (a second bore across a bore of the same size, a pipe
  tee of equal pipes) touch at two points, where the mesh intersection cannot trace the curves
  and a boolean cannot tell the four arms of the crossing apart. When the ordinary boolean fails
  on such a pair, both faces are split along the line of their cylinder through the touching
  points (the solids are unchanged), the boolean is taken of those, and its history is composed
  back over the operands as given (#788). The result keeps the split lines as edges of the bore
  walls. A crossing the ordinary route fails on and the split faces do not help (a radius of 17.5
  in a 45 mm block with a tool of 80 mm) is still refused.
- Boolean intersection-curve construction refuses point-only, collapsed and non-finite
  polylines without panicking. Direct constructor regressions retain valid open endpoints
  and closed spherical intersections; the original interactive trigger remains unidentified.
- Shell of a stepped, joined planar body no longer creates horizontal subdivisions across
  continuous walls. Fresh axis-aligned Cut walls are stitched using their exact construction
  support and shared topology, preserving real steps and intentional boundaries. The local
  reported case returns from 86 to 49 faces with unchanged material and bounds; regressions
  cover stepped stock, STEP round trips and saved projections from removed pieces.

truck (Apache-2.0) behind our own `kernel` crate, with our own blend operation and boolean
retry/validation; see `plan/adr/0001-geometry-kernel.md` (local) — summary in README.
