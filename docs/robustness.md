# Kernel robustness

Real users hit edge cases the reference tests (docs/oracle.md) never see: faces that coincide
by accident, tangencies, seams lying where another body cuts, walls thinner than a shell. The
robustness runs build random but realistic designs and check every step.

## How it runs

```
solidcraft-cli fuzz --from 0 --count 1000 --jobs 8 --timeout 90 --out runs/ --report report.md
solidcraft-cli fuzz-one <seed> --minimise      # one design, cut down to the steps that matter
```

`--count` seeds are run starting at `--from`. A range that exceeds `u64::MAX` is
rejected before any jobs or output directories are created. The maximum seed
(`18446744073709551615`) is valid with `--count 1`; `--count 0` runs no seeds.

- **Designs** (`crates/engine/src/fuzz.rs`): a seeded generator starts from a box, an extruded
  sketch profile closed by an arc (sometimes notched), or a revolved section (partial or full
  turns, sometimes touching the axis). It then adds up to eight steps: joins and cuts of
  boxes, cylinders and spheres, fillets and chamfers on random edges, shells, holes (simple,
  blind, drilled), and rectangular and circular patterns of the last feature. Placements snap
  to a 2.5 mm grid and often to the body's own faces, so coincident faces and tangencies come
  up all the time, as they do in real parts.
- **Checks after every step:** the command succeeds; every body is a sound solid (each shell
  closed and consistently oriented, every face meshes, positive finite volume:
  `Body::validity`); the volume moves the right way (a cut never adds, a join never removes).
- **Boolean identity** (`crates/kernel/src/fuzz.rs`): for two random primitives on the same
  grid (boxes, cylinders, spheres, tori), union, intersection and cut each give a sound solid
  and V(A∪B) + V(A∩B) = V(A) + V(B), V(A−B) = V(A) − V(A∩B), within 0.2 %.
- Each seed runs in its own process with a time limit, so hangs and hard crashes are recorded
  too. A step the kernel refuses as **not supported yet** (a fillet between two curved faces,
  a shell thicker than a wall) is undone and counted separately, and the design goes on: those
  are known limitations, not failures. Booleans that fail count as failures even when their
  message says "not supported".
- `cargo test` runs a fixed set of seeds that must stay clean
  (`crates/engine/src/tests/robustness.rs`), next to the minimised cases fixed so far.

## Seeded goldens

`tests/robustness/goldens.json` records what each of seeds 0–95 builds (eight steps at most):
its steps, how it ended (ok, or the kind and the start of the error it was refused with), and
for the design it left the bodies' total volume and area (within a relative 1e-6) and their
face, edge and shell counts (exact). `tests::robustness_goldens` fails on any change, better or
worse, so a kernel change shows every seed it moves. Pull requests that touch `crates/kernel`
run every 8th seed; pushes to `main` and the nightly run them all. Each seed is its own test
(`seed_00` … `seed_95`), so the CI shards and their cores share them out.

To record an intended change, give the reason:

```
ROBUSTNESS_GOLDENS_BLESS="why it changed" cargo test --profile ci -p solidcraft-engine --lib robustness_goldens::seed_
```

The changed seeds and the reason are added to the file's `notes`; say why in the pull request too.

## Successful Boolean retries

When an accepted Boolean result needs a retry that can make it differ from the
exact one (a tool nudge, or the unverified shortcut), the feature shows a
warning in the timeline. Every other retry works on temporary geometry or
tolerances and leaves the exact result, so it is recorded but not warned about.
`timeline.inspect` and `doc.inspect` expose every retry in the feature's
`boolean_diagnostics`, including the strategy and the applied translation,
rotation, face offset and tessellation tolerance in millimetres/radians.

A common shift of both operands is reversed on the result. Coincident-face
separation and bore offsets describe temporary working geometry; they are not
claims about the final model's error. A coincident face is separated only
where the space it moves through holds what the Boolean leaves there anyway
(nothing of the target for a cut's face, the target's material for a join's);
a face with no such room stays, and the exact planar fallback is tried first.
A tool nudge changes the relative operand
placement and reports both its translation magnitude and rotation angle. When it
fails, or leaves a sliver face (narrower than 1/200 of the part), it is tried the
other way too: one way can push a face into a curved face it only touched (a cut
whose end lies tangent to a round side) and leave a sliver of it, where the other
way parts them (#897, #827).
Exact planar and piecewise fallbacks report zero displacement. When a conical
cutter meets a body with earlier conical faces (pointed bores copied in a pattern),
two finer, unshifted passes are tried first, at 1e-5 and 1e-6 of their size (never
below 1e-6 mm). A coarse first cut can leave tiny junctions that later copies cannot
intersect, so refining only the failing copy is too late. Every candidate must pass
the volume, membership and spatial checks; a rejected candidate continues to the
next tolerance, then the ordinary routes. The existing acceptance checks still
apply to all results.

A join or a cut that needed any retry is then held to what it can do at all
(`crates/kernel/src/boolean_guard.rs`, #616): the result is closed, a join
never loses volume and a cut never gains any, and outside the tool's box the
result's surface is the target's and the target's is the result's (a join or
a cut can only change the target where the tool is). A cut may come out empty
only when the tool covers the target. A result that fails is refused with an
error naming what was wrong and where, rather than returned: a wrong result
that looks right is worse than an error.

Reports belong to the evaluated feature and survive cached rebuilds and
undo/redo. Cold reopening regenerates them from the embedded design. Failed or
discarded retry branches do not contribute successful-retry events. Reports
are capped at 1024 events per feature and explicitly identify truncation.

Fuzz JSON and the text report count successful retry events by strategy for
designs and Boolean identity checks separately, including nested operations.
These are event counts, so a single Boolean or seed can contribute several.
Truncated reports are counted separately rather than implying complete totals.

## Results

1000 seeds (0–999), 8 processes on a heavily loaded machine (load average 60–80), 90 s per
seed. A "timeout" there is mostly a design whose booleans are very slow (seed 20 finishes in
2 min 8 s alone), not a hang. The "after" run had more of them: the machine was busier still
(test suites ran alongside), and failing booleans now try more fallbacks before giving up;
fewer designs fail outright (27.1 % against 36.3 %). Both figures are from before the last two
fixes below (sound face pushes, no panics in the boolean's final assembly).

| Designs | before | after |
|---|---|---|
| clean | 49.8 % | 53.8 % |
| failed | 36.3 % | 27.1 % |
| invalid body | 0.7 % | 0.9 % |
| volume the wrong way | 0.2 % | 0.2 % |
| over 90 s | 13.0 % | 18.0 % |

| Boolean identity | before | after |
|---|---|---|
| holds | 85.4 % | 80.8 % |
| a boolean failed | 1.6 % | 1.2 % |
| not run (the design ran out of time) | 13.0 % | 18.0 % |

### Fixed

- Back-to-back contact joins preserve intentional Split Face boundaries, including on
  faces away from the contact. The glue pass retains their lines during healing and in
  the resulting body for subsequent operations. Partial circular-contact regressions
  cover offsets, rotation, operand order, volume, area and history; the seed-16 pattern
  also runs in the ordinary engine suite with analytic volume and area checks.
  Imprinting visits a shared analytic support only once: the two side faces of a
  cylinder must not recut the same outline into redundant arcs.

- **Shell corners where a curved face meets another at an angle** (the biggest cause, 215 of
  the 363 failed designs before). Moving a vertex took one linear step, exact only for planes;
  a cylinder meeting a plane at an angle (a shallow arc side) landed off the moved surface.
  Now refined by Newton steps onto the moved surfaces.
- **Shell edges that are neither lines nor arcs** (a hole's wall meeting a curved side): the
  edge is rebuilt through its moved points.
- **Shells of bodies in several pieces** ("this shell is not connected"): one shell per piece.
- **Cylinder seams on the grid.** A cylinder's seam lines lay on its frame's axes, exactly
  where faces placed on the same grid cut it, and booleans failed along the seam. Seams now
  start at an odd angle: cylinders tangent to box faces from inside or out, and crossing
  cylinders touching along a line, now work.
- **Partial revolves of profiles touching the axis** (a quarter or half turn of a rectangle
  from the axis): the edge on the axis no longer sweeps a zero-area face, which every boolean
  attempt failed on.
- **Booleans on bodies in several pieces** (a join that left two lumps): when the whole fails
  or comes out empty, a piece at a time.
- **Pushing a face apart** (coincident faces) checks its result: the exact rebuild could leave a
  cylinder unsound, and then the general offset is used. A boss standing on a plate joins again.
- **No panics in the boolean's final assembly.** Natively a panic inside the kernel is caught,
  but the web build aborts on any panic; the Plastic Enclosure sample panicked 8 times building
  its bosses and crashed the start page in the browser. The vendored shapeops now returns
  `None` instead, and a test builds every sample with a panic counter. The runs count caught
  panics per design ("Caught panics" in the report), since each one would crash the web build.

### The first tutorial (box, fillets, shell)

Found by QA on new users' first steps, each with an engine test against the analytic volume
(`crates/engine/src/tests/robustness.rs`):

- **Shell of a filleted box** (vertical edges r5, bottom loop r3, 2 mm, top open): the bottom
  round runs round the corners as spindle tori (the tube wider than its circle), which the
  offset now takes. Volume within 1e-5 of the analytic one.
- **Shell thicker than or equal to a round** (r2 rounds shelled 2 mm): the round's inside
  collapses to a sharp edge, as in Fusion, instead of "a cylinder shrinks to nothing".
- **Shell thinner than a round tangent to the open face** (1.5 mm): every face moves in
  (tangencies keep) and the opening is the open face's copy swept out through the body.
- **Shell after a drilled hole** (its tip a cone).
- **Filleting a box's edges one at a time**: an edge that runs into an earlier round takes
  that round off and rounds both together, with the corner where they meet (`blend_corner.rs`,
  any radii).

### Speed

- Measuring a body of 5000 faces (a modelled thread): 13 s to 0.8 s.

### Known limitations (counted as "not supported", with a clear message)

- A shell thicker than a wall or a step: the moved faces would cross and the cavity turn
  inside out. Fusion fills such places solid; we need a topology-changing offset.
- Fillets and chamfers between two curved faces, or ending at faces that aren't planar.

### Still failing

- Partial unions of a cap and a face (a cylinder's end lying partly on a box face): pushing
  one face into the other adds material where they don't overlap.
- Shells of partial annuli (a half ring) whose cavity truck's boolean rejects although the
  same cavity built another way cuts cleanly.
- Very slow booleans on bodies with many patterned features.
