# STEP import

SolidCraft reads STEP (ISO 10303-21; AP203, AP214 and AP242 geometry) with its own reader in `crates/kernel/src/step_in` (written from the public standards; no third-party STEP code). A STEP file becomes an **Import** base feature on the timeline: it holds the file's B-rep (the STEP text is kept in the design, so the design reopens without the file) and later features — fillets, holes, cuts sketched on its faces, shells — build on its bodies.

- **Open**: File → Open (`doc.open`) accepts `.step`/`.stp` in any case and starts a new design; `solidcraft part.step`; `solidcraft-cli eval part.step`, `solidcraft-cli run script.json --in part.step`; the MCP `open` tool.
- **Insert** into the current design: File → Insert STEP… or SOLID → INSERT (`file.insert_step {path}`).
- **Drag and drop** a STEP file on the window: inserted when the design has features, opened otherwise.

## Imports are components

Every import (STEP, IGES, 3MF, STL, OBJ; inserted, dropped, opened as a new design, or run as `file.insert_step`, `file.insert_mesh` or `doc.open` from a script, the CLI or MCP) becomes a **new component named after the file** (`bracket.step` → component `bracket`), placed in the active component at the file's origin. The Import feature belongs to it, so the file's bodies never land loose next to your own. Opening a file as a design makes the component too, so the browser shows the file's name. The command's reply reports it: `component` (id), `component_name` and `occurrence`.

A STEP **assembly** keeps its structure: each product under the file's root becomes a component nested under the import's, named after the product, at the place the file gives it. A part (or sub-assembly) the file places more than once becomes **one component with an occurrence per placement**: the Import feature builds the part once and skips the file's later copies (`skip` in the feature, format 22), and the occurrences show it everywhere else. The reply adds `subcomponents` (how many components were made below the import's) and `rigid_joints`.

The Browser reuses body, sketch and construction-plane membership, child lists and resolved placements while the
camera moves. Large assemblies keep their tree open without resolving every part and
folder again on each frame. Geometry edits, assembly changes and uncaptured or reverted
positions refresh those queries; selection, visibility and locking are read live.

Hover picking pauses while the mouse moves the view and resumes when navigation stops.
A 3D mouse moving the view leaves it on: the pointer hand is free, so preselection and
clicks keep working while the other hand turns the view. Picking uses the current camera
and the cached body bounds directly in model order, so finding each body's bounds does not
scan the assembly again.

Hovering an assembly's Browser row retains its whole-body highlight buffers during
rotation, panning and zoom; the GPU transforms those buffers with the camera. The
origin and construction widgets resize independently. Whole-body
highlight generation scans each mesh once rather than scanning it again for every
face. Sketch profile feedback still updates its projected curves and lift toward
the eye as the view changes.

The contents of an imported assembly are **rigid by default**: rigid joints (a rigid group, its glyphs not drawn) hold each part where the file put it in its parent, so moving the import's component moves everything, and an inner part doesn't move on its own; joint degrees of freedom count the whole import as one body. To joint the parts one by one, release them with **Unlock Contents** on the component's browser menu (`component.unlock {component}`), one undo step. Undo takes a whole import away in one step.

Designs saved before imports made components keep their Import where it is (no component is made for it) and rebuild as they did.

Read: manifold solid breps (with voids), faceted breps, shell-based surface models; planes, cylinders, cones, spheres, tori, B-spline and rational B-spline surfaces (with knots, Bézier, uniform, quasi-uniform), surfaces of revolution and linear extrusion, trimmed surfaces; lines, circles, ellipses, hyperbolas and parabolas (as exact rational curves, which export writes back as exact rational B-splines), B-spline curves, polylines, trimmed/surface/seam curves; length and angle units (SI prefixes, conversion-based units such as inches); product names (body names come from the solid's name, else the product's; `names` below chooses); surface colours; assemblies (next-assembly-usage occurrences placed by item-defined transformations, and mapped items) — the product tree is kept in the feature and becomes the import's components (above). Files are untrusted: sizes, entity counts and nesting are capped and every kernel call is guarded. What cannot be read becomes a warning on the feature (a face left out gives an open body); not read yet: offset surfaces, pcurve-only edges.

## Files from other CAD systems

Files from different systems differ in which way is up and in what they call their solids. `doc.open`, `file.insert_step` and the MCP `open` tool take two options for STEP and IGES files; without them a file comes in as written.

- **`up`**: `"z"` (the default) keeps the file's axes. Many systems write Y up, so such a part lies on its back in SolidCraft's Z-up space. `"up": "y"` turns the import's component a quarter turn about X (+90°: Y becomes Z), as the window's "Stand it up" offer does, so an assembly's parts and instances turn together. The import and the turn are one undo step, the reply names the turned occurrence as `stand_up`, and only the file's component turns, not what is already in the design. The window offers to stand a part up after a STEP file opens unless the call said which way is up (`up: "z"` as well as `"y"`).
- **`names`**: what the bodies are called. `"solid"` (the default) is the name of the solid in the file, and the product's name where the solid has none. Some systems fill a solid's name with the last feature that made it (`Cut-Extrude3`), so every body of an assembly has a name that says nothing about its part. `"product"` names each body after the product (the part) that holds it, and `"instance"` after the assembly occurrence (`Post-2`), or the product where the body is not in an occurrence. A name the file leaves out, or fills with a placeholder, falls back to the next one (instance, product, solid). Repeated names are numbered as any body names are (`Post`, `Post (2)`). The names are stored with the Import feature, so the design reopens with them. The default stays the solid's name because SolidCraft's own STEP export writes a body's name there.

```json
{"command": "file.insert_step", "params": {"path": "frame.step", "up": "y", "names": "instance"}}
```

## Imported appearances

Imported body colours and face colours are retained through later features. Body
colours remain the fallback under explicit design appearances and materials; face
colours follow persistent face names. A removed face has no colour assignment to
display on the resulting body.

## IGES

IGES 5.3 files (`.igs`/`.iges`) open, insert and drop the same way: `crates/kernel/src/iges_in.rs` (written from the published specification) restates the file as STEP and the STEP reader reads it, so the feature holds STEP text. Read: manifold solid B-reps (186) with their shells, faces, loops, edge and vertex lists; trimmed (144) and bounded (143) surfaces, and bare B-spline patches, sewn into a solid by their shared edges (an open body when they do not close); lines, circular arcs, ellipses, composite curves, rational B-spline curves; planes (108, 190), cylinders, cones, spheres, tori, rational B-spline surfaces, surfaces of revolution and tabulated cylinders; transformation matrices; units; colours (314 and the predefined numbers); body names (name properties, else labels). Not read yet: ruled and offset surfaces, curves on surfaces given only in parameter space. Export (`file.export` to `.igs`) writes each body as a manifold solid B-rep on the same surfaces a STEP export uses, with its name and colour.

## FreeCAD and Rhino

FreeCAD `.FCStd` and Rhino `.3dm` files open, insert and drop the same way, in the desktop app and the CLI. They are read with [cadmpeg](https://github.com/cadmpeg/cadmpeg) (Apache-2.0, `crates/io/src/native_cad.rs`): cadmpeg detects the format from the bytes, decodes it into its neutral model under resource caps, checks that model and writes it as STEP AP214, which the STEP reader then reads. The feature holds that STEP text, so the design reopens without the original file. The file's solids come in; its feature history, sketches and parameters do not. Losses cadmpeg reports become warnings on the feature, and a file without an importable solid is refused with a message. cadmpeg runs inside the panic guard. How much of a file is read follows cadmpeg's support for that format ([format support](https://github.com/cadmpeg/cadmpeg/blob/main/docs/format-support.md)).

Only codecs for open formats are compiled in: the workspace turns off cadmpeg's default features and enables `fcstd`, `rhino` and `step` (the writer). cadmpeg's codecs for commercial CAD formats are not built, and neither is its `sat` codec, which decodes ShapeManager streams with ACIS as one dialect, so ACIS `.sat`/`.sab` files are not read yet. `crates/io/src/native_cad.rs` is the one place cadmpeg is called; a cadmpeg-based STEP reader could be added there later behind a setting, to compare with the STEP reader above without changing it. The browser app does not read these formats yet: cadmpeg compiles the zstd C library, which the wasm build leaves out.
