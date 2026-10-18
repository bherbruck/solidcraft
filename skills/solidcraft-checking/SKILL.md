---
name: solidcraft-checking
description: Use alongside solidcraft-modeling and solidcraft-editing whenever you build, change or port a SolidCraft design. Checks for errors SolidCraft does not report (parameter ranges, every allowed value, exact STEP comparison), regression tests, designs kept in git, porting from build123d, CadQuery, OpenSCAD or STEP.
---

# SolidCraft checking

`solidcraft-modeling` and `solidcraft-editing` cover the commands, plane axes, ids, edge picks and the MCP workflow: load the one that fits first. This skill adds checks for errors SolidCraft does not report, and the work around a design kept in a repo.

`$SKILL` below is the folder this file is in (`~/.claude/skills/solidcraft-checking` for a user install). Written against SolidCraft 0.8.0. `scripts/tests/test_facts.py` checks every trap and convention below, and the facts behind the corrections in `solidcraft-modeling`. After a SolidCraft upgrade, run the tests and fix the text where they fail:
`PYTHONDONTWRITEBYTECODE=1 uv run --no-project --with pytest --with build123d pytest -p no:cacheprovider $SKILL/scripts/tests`

## Scripts

An MCP `batch` and a command script (`{"commands": [...]}`, run by `solidcraft-cli run`) take the same command list. Build with a script when the design goes into a repo or must be rebuilt later: a script rebuilds the design from scratch on every run. Subagents share the session's one MCP design, so agents that run in parallel use `solidcraft-cli` only.

In `$SKILL/scripts/`:
- `python3 $SKILL/scripts/run.py script.json [--in <design>] [--save <name>.json]` runs a script and prints `FAIL`, a `WARN` line per feature warning, a `SKETCH` line per sketch above dof 0 and a `BODY` line per body, for the end state and for each `document.inspect` inside the script.
- `scb.py` gives a generator a `Script` with sketch helpers that come out at dof 0 (`hexagon`, `circle`, `rounded_rectangle`, `place`) and `cmd` for any other command. It rejects repeated sketch, feature and body names. Import it with `sys.path.insert(0, "<$SKILL>/scripts")`.
- `uv run --no-project --with build123d python $SKILL/scripts/compare_step.py actual.step expected.step` gives the exact volume of the difference between two STEP files and where it sits. It and the tests run OpenCASCADE through build123d as a separate tool, the way `tools/freecad-oracle` runs FreeCAD: SolidCraft does not use, link, vendor or ship any of it.
- `solidcraft-cli snapshot <design or script> --out shot.png --view iso` renders a PNG to look at.

## Steps to add

1. **Parameters and features.** Each parameter comment states the allowed range ("whole number, 4 or more"): the user reads it in the Parameters table, and SolidCraft enforces no range. Derive sizes from measured values, so that a count parameter changes how many features there are, not how big they are. An expression that more than one sketch or feature uses becomes a named parameter. Add the derived numbers the user needs (parts to print) and mark the driving parameters favourite (`parameters.favorite`). Give each thing the user thinks of (a port, a row of standoffs) its own feature, named after it, so it can be found and changed on its own. Choose the origin so that no sketch dimension reaches 0 for any allowed value (trap 1).
2. **Every sketch at dof 0.** A sketch with dof left can solve to a different shape after a parameter change, without an error. `run.py` lists them.
3. **Every allowed value.** One script of `parameters.change` commands walks each driving parameter to both ends of its range, one change after another, and back to the default, as edits in the app do: trap 1 fails only on the change after the one that reaches 0. Put a `document.inspect {measure: true}` after each end of a range and run the walk with `run.py walk.json --in <design>`. Every state must be clean: no `FAIL`, no `SKETCH`, and `BODY` volumes and centres of mass that match the values you compute for that state. A boolean retry can leave the geometry wrong with only a `WARN` line, and the volume shows it (trap 4); a mirrored sketch (trap 1) keeps the volume and moves the centre of mass.
4. **Exact check** when a port has a reference, or a volume gap leaves doubt: export STEP (`file.export {path, bodies}`) and run `compare_step.py`. Before porting, read `$SKILL/reference/porting.md`.
5. **Kept in a repo.** Save as `<name>.json` with `run.py build.json --save <name>.json`: a `.json` design stays plain JSON when it is saved again, so git diffs stay readable. Saving over a file leaves `<name>.json.bak`, so ignore `*.bak`. From then on the design file is the source, and changes go into it with `--in <design> --save <design>`. Copy `$SKILL/templates/test_design.py` next to the design, fill in its cases, set its tolerance as its comment says, and run it until it passes. Expected values come from an exact reference when porting, from hand calculation for simple shapes, or from a build the user has reviewed. After a change, update frozen values only where the change is meant to change them.

## Traps

1. **Zero dimension.** A sketch dimension driven to 0 loses its side. On the next parameter change it either fails with `its constraints would conflict` (a point placed by dimensions from the origin) or, with no error, puts the point on either side (a slot that other constraints also held came back mirrored). Check every distance expression against the whole allowed range of each parameter in it, counts included; the walk in step 3 catches the failure, and only the volume or the position catches the mirror. For a point on an axis, pass `x=None` or `y=None` to `place`: it aligns the point instead of dimensioning it. For a position that does reach 0, such as the first of a centred row of one, dimension it from a sketch point placed outside the part: a point at x = -100 mm, then the distance `100 mm - (n - 1) / 2 * pitch` from it.
2. **Counts.** Solid patterns take counts 1..1000; a count that reaches 0 fails the rebuild. A fractional count rounds to the nearest whole number without an error (6.5 gives 7 copies), while every other expression uses the exact value, so a centred row shifts. Nothing in SolidCraft enforces a parameter range during a parameter change (configuration rules apply only when a configuration row is activated): choose a range that keeps every count at 1 or more, write "whole number" and the range in the comment, and use `round(n)` wherever the count also positions something.
3. **Circumscribed polygons.** `sketch.polygon.circumscribed` with a size dimension and one vertical side still has 1 dof: its corners can alternate in and out. `sketch.constraint.polygon` makes it regular; `scb.hexagon` adds it.
4. **Boolean retries.** A `Boolean retry: ...` warning means the result may be wrong (see "Boolean warning" in the `solidcraft-modeling` reference), so check the volume of that state. `temporary coincident-face separation` comes from faces that coincide. A tapered tool whose start or end lies on a face of a plain block, or a straight cylinder joined onto the floor of a filleted, shelled box, does not log it; a `tool displacement` warning shows up when a join and a cut share one location, as in a standoff built as a joined cylinder with a cut pilot hole, patterned together. That case reports the warning but its volume agrees with the analytic growth, and a single ring profile removes the warning. Starting cutters about 1 mm past the faces they pierce and sinking joins about 1 mm into the body remain good practice on a single feature. Check the volume of every state either way.
5. **Measurement accuracy.** The `solidcraft-modeling` reference gives how far measured volumes stray from computed ones. Centre of mass and bounding box come out up to about 5e-5 of the part size off. Set test tolerances as the template says.
6. **Expressions.** They have `+ - * / ^ %`, `sin cos tan asin acos atan atan2 sqrt abs min max floor ceil round mod trunc exp ln log log10 pow sign hypot` (`log` is base 10), `pi` and units; no `if` and no comparisons. Build steps from `floor`, `mod`, `min`, `max` and `sign`, which gives 1 at 0. `asin acos atan atan2` return angles, so a unitless parameter cannot hold them.
7. **Snapping.** A point created exactly on an existing point or the origin becomes coincident with it (`connect` defaults to true).
8. **Rectangle symmetry.** Opposite sides of a rectangle run in opposite directions, so `sketch.constraint.symmetry` on two sides conflicts with the rectangle; make two corner points symmetric instead.
9. **Repeated body names.** A repeated `body_name` is renamed (`Body1`) without an error, so `targets` reach only the first body.

## Conventions

- `taper` positive widens along the extrude direction, negative narrows it, also with `direction: negative`.
- `sketch.polygon.circumscribed` with 6 sides and `angle: 0` puts vertices up and down and flat sides facing ±x.
- A pattern of a join or cut feature repeats that join or cut. Put every join before the cuts that must stay open: a later join fills them.
- When a timeline feature fails, `solidcraft-cli run` writes nothing and the error names the feature. `run --in` and `snapshot` also accept a command script in place of a design.
- To build in the user's open app, see the bridged mode in [docs/mcp.md](https://github.com/bherbruck/solidcraft/blob/main/docs/mcp.md): the app and the server both start with extra flags, so ask the user.
