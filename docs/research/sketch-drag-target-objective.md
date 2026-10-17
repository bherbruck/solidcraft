# A one-degree-of-freedom linkage does not follow the cursor

**Implemented:** point dragging now minimizes cursor distance within the design's remaining
freedom when held continuation cannot arrive. The reference linkage reaches its analytic
nearest point in one large event, with lengths and assembly branch preserved. Correcting
previously incomplete motion costs additional computation: the representative captured
8 mm case changed from 0.658 to 1.602 ms. See the matched measurements and UI evidence below.

The pre-fix failure in the supplied four-bar example was the drag problem formulation.
The engine fixes the grabbed endpoint's two coordinates at each cursor interpolation point
and asks for exact constraint satisfaction. A one-degree-of-freedom linkage generally cannot
reach that arbitrary point in the plane. On failure, the fallback solves from the **first
failed intermediate point**, not the current cursor. This produces small, event-dependent
motion even when the mechanism can rotate substantially toward the cursor.

The earlier large open-chain experiment
found a real iteration-limit/continuation failure, but its reachable two-dimensional workspace
does not represent this one-degree-of-freedom example. Increasing LM iterations or scheduling
more retries does not correct an incompatible hard cursor constraint.

## Reproduction matching the reference dimensions

The reference images show one remaining degree of freedom, a 10 mm crank, a 40 mm coupler,
a 20 mm rocker, and a horizontal 42 mm ground link. Reconstructed an equivalent mechanism:

- Fixed origin O = (0, 0), ground endpoint B = (42, 0).
- Grabbed crank endpoint A = (−10, 0), with |OA| = 10.
- Coupler/rocker joint C is an intersection of circles |AC| = 40 and |BC| = 20.
- Ground length and horizontal direction are constrained; crank angle remains free.

For every crank angle, |AB| is between 32 and 52 mm, inside the coupler/rocker's feasible
20–60 mm interval. Thus the entire crank circle is feasible on the same intersection branch.
This gives an analytic oracle for the grabbed point's closest position to a cursor.

Using the existing local SolidCraft 0.14.0 desktop binary, in a dedicated safe-mode instance,
with the XZ plane and front view, drag the endpoint from (−10, 0) toward (−10, 8):

| Quantity | Sketch coordinates (mm) |
| --- | --- |
| Cursor target | (−10, 8) |
| Actual engine command result | (−9.91030137, 1.33638572) |
| Actual UI drag result, snapping suppressed | (−9.91030137, 1.33638572) |
| Analytic closest feasible crank point | (−7.80868809, 6.24695048) |

The pre-fix result remains about 6.6642 mm from the cursor; the attainable minimum is
sqrt(164) − 10 = 2.80624847 mm. Exact arrival at this off-circle cursor is impossible, but
substantial improvement toward it is feasible and should happen in the same gesture.
The engine returns `reached: 0` and `sketch.solved: true`: the final geometry satisfies the
design constraints, while the cursor objective was never minimized.

As a control, targeting (−8, 6), which lies **exactly** on the crank circle, reaches (−8, 6)
with `reached: 1`. A free endpoint dragged through the actual UI to (10, 10) reaches
(10.00000277, 10.00000069), consistent with screen-coordinate rounding. The constrained UI
and engine results agree with snapping disabled. These controls do not indicate a coordinate
conversion error in this reproduction; they do not prove every camera/DPI combination correct.
The screenshots supplied by the user do not encode the pointer trajectory, so this is an
equivalent public-safe reproduction rather than a replay of their exact mouse events.

## Why large movements barely advance

In `crates/engine/src/cmd/sketch_drag.rs`:

1. `plan` adds the endpoint to `Hold.points`; `Path::apply` places it at a point on the
   straight path from its previous position to the cursor.
2. `drag_entity` invokes `solve_with` with that point held, enforcing both X and Y. Even
   intermediate points on the chord between two circle positions usually violate the
   crank's fixed length. Those failures are expected for this imposed hard-pin problem;
   they do not establish that the mechanism cannot move toward the mouse.
3. On the first failed step, the code records `failed = Some(t)`. If later held solves also
   fail, that first fraction remains recorded. At the end, `path.apply(sk, t)` places the
   endpoint at that early position and returns `Left::MovedFrom(last.pos)`.
4. `edit_solved` in `sketch.rs` performs an ordinary solve with `Hold::default()`, using the
   last accepted geometry as a reference. The mouse coordinates are no longer an objective
   or a constraint. The solver obtains a valid pose close to the small intermediate edit.

For a 10 mm crank, the usual continuation step is at most a quarter of that size, so the
fallback often receives only about 2–2.5 mm of an arbitrarily larger cursor movement.
The 64-step cap does not truncate the held path's final target, but keeping the **first**
failed target effectively limits the edit used by this fallback. Another mouse event starts
the process again. The direction and division count change with each event, which explains
motion that feels disconnected from the grab-to-cursor displacement.

The normal solver is a feasibility solver. Its underconstrained solutions are not specified
to minimize Euclidean mouse distance. Its coordinate-scaled LM damping can further affect
which free pose it chooses; no isolated damping experiment has yet established the magnitude
of that effect here. It is unnecessary to blame damping or iteration limits to explain the
incompatible hard pin and discarded cursor objective.

## Correct objective and implementation priorities

Keep design constraints mandatory and use the cursor as an objective:

```text
minimize   ||grabbed_position(x) - cursor||²
subject to design_constraints(x) = 0
```

This lets the available freedom reduce the cursor error while preserving lengths, fixed
points and other design constraints. For the crank it projects to the circle; for a linkage
it searches its feasible motion, retaining the existing assembly branch and arc safeguards.

1. Add a cursor objective operating in the freedom left by the design constraints, through
   a constrained or hierarchical solve. Simply appending equally weighted mouse residuals
   would allow length errors and is not equivalent. A large arbitrary weight is not a proof
   that design tolerances remain satisfied.
2. Preserve the current target through failed trial steps. Adapt the step in feasible motion;
   do not substitute the first failed cursor interpolation point as the final goal.
3. Then add bounded continuation across UI frames where numerical progress is unfinished,
   with cancellation/replacement and explicit no-progress handling. Retrying an inconsistent
   exact pin indefinitely would still be wrong.

Regression tests should compare cursor distance against the analytic crank oracle while
checking every design constraint and branch. Include off-locus targets in all directions,
one large event versus smaller moves to the same target, stationary held frames, different
views/zooms/DPI, snapping on/off, and the fully fixed case. The prior open-chain arrival tests
remain useful for mechanisms whose endpoints have a two-dimensional reachable workspace.

The measurements above describe the frozen pre-fix implementation. The four-bar benchmark
records distance to the cursor and distance to the analytic nearest point separately; exact
arrival at an off-circle cursor is impossible.

## Feasible point-drag implementation

The engine retains held continuation, including paths across unreachable stretches of an
otherwise reachable move (the hole around a two-link arm's shoulder). If that cannot arrive,
it restores the last feasible pose and passes the **whole current cursor target** to
`solidcraft_sketch::move_point_towards`. It no longer substitutes the first failed fraction
for that target on this path. Curve-body/radius dragging keeps its existing behavior.

The sketch module finds the grabbed point's connected variables, forms an orthonormal basis
for the design Jacobian's null space, and computes a cursor step in that freedom. Each trial
is retracted onto the design constraints by the existing LM implementation, without any mouse
residuals. A candidate is kept only when it satisfies the design tolerance, improves cursor
distance and passes the engine's existing arc-sweep guard. Initial feasibility is refined when
a previous held solve was accepted between the ordinary `ACCEPT` and stricter `TOL` thresholds.
No mouse constraint is persisted and no design format change is needed.

Step sizes depend on the smallest driving length in the affected geometry. Adaptive gain and
backtracking accommodate locus curvature. If a first-order cursor step cannot improve the
pose, bounded probes in both tangent directions distinguish a local minimum from the nearly
stationary maximum reached by dragging across a pivot. Comparing the gradient to exactly zero
would miss that case after numerical rounding. Work is bounded to 400 connected variables, 96
accepted steps and 256 LM correction iterations (16 per trial). Exhausting the work budget
retains the best feasible pose; this is a local constrained optimizer, with no claim of a
global nearest point for arbitrary nonlinear mechanisms. Oversized or unsolved inputs retain
the existing fallback. These limits are separate from any future scheduling across UI frames.

The engine returns `Left::Solved` for accepted feasible motion, so an ordinary unheld solve
cannot discard its cursor objective afterwards. Published full diagnostics retain the existing
live-drag policy; release performs normal model evaluation. `at` reports the actual point and
`reached` on this path projects its displacement onto the requested move, clamped to 0..1.

## Matched before/after measurements (2026-10-17)

Frozen `sketch_target_bench` executables, both native `ci` profile with `solver-profiling`,
on Windows x86_64, Intel Core Ultra 7 258V, Rust 1.96.0. Each pass uses two warmups and ten
measured fresh sessions per case; two passes in before/after/after/before order give twenty
samples per version per case. Builds and the diagnostic app were stopped before measurement.
Fixture construction and JSON serialization are outside the timer. Capture/trace overhead
is included. These are synchronous command wall times, without UI event pacing or rendering.

The fixture and targets are identical between versions: A starts at (−10, 0), ground O–B is
42 mm and links are 10/40/20 mm. The numerical fixture uses the lower C intersection; the
UI reproduction uses its reflection to match the supplied screenshot's orientation. This
does not change the crank's circle oracle. Error below means distance from the analytic
nearest **feasible point**, rather than the unavoidable distance from an off-locus cursor.

| Cursor target (mm) | Events | Before median (ms) | After median (ms) | Before max nearest-point error (mm) | After max error (mm) |
|---|---:|---:|---:|---:|---:|
| (−10, 8) | 1 | 0.658 | 1.602 | 5.341 | 1.27e-7 |
| (−10, 8) | 16 | 6.202 | 12.291 | 1.468 | 1.96e-7 |
| (−10, −8) | 1 | 0.751 | 1.866 | 5.930 | 1.15e-7 |
| (−10, −8) | 16 | 5.352 | 10.740 | 2.629 | 1.91e-7 |
| (20, 20) | 1 | 1.900 | 3.278 | 17.190 | 6.31e-7 |
| (20, 20) | 16 | 12.062 | 14.933 | 4.644 | 4.59e-7 |
| (20, 0) | 1 | 1.671 | 1.776 | 8.95e-4 | 0 |
| (20, 0) | 16 | 21.943 | 10.394 | 18.190 | 3.33e-8 |
| (−8, 6), on the circle | 1 | 0.369 | 0.363 | 0 | 0 |
| (−8, 6), on the circle | 16 | 4.719 | 8.433 | 0 | 0 |

Sixteen-event times are totals for all sixteen commands. Their intermediate cursors can
be off the circle even when the final target is on it. The after version satisfies every
measured final length/fixed-position check within 1e-9 mm, keeps the assembly branch, and
records zero cursor-budget stops. The largest observed one-event wall time was 7.590 ms;
wall-time outliers do occur, so the medians are not a frame deadline guarantee.

This fixes arrival accuracy; it is not a general speedup over the earlier, incomplete motion.
For the (20, 20) one-event case, inclusive median LM time is 2.561 ms. Held solves account
for 2.355 ms; cursor optimization accounts for 0.836 ms, including 52 LM retractions and
35 accepted cursor steps. Null-space construction accounts for only 0.026 ms. These nested
timings must not be added together. The main remaining costs are repeated held feasibility
attempts and LM corrections, rather than null-space construction or redundancy analysis.

Next priorities, if more latency reduction is needed:

1. Avoid repeated hard pins only when their infeasibility is established. A failed intermediate
   pin alone is insufficient: prematurely switching to the local optimizer broke the existing
   arm-through-an-unreachable-hole tests. Held continuation is retained and those tests pass.
2. Reduce repeated Jacobian/correction work, with fresh feasibility validation before accepting
   a pose. Optimize null-space algebra only if larger-component measurements justify it.
3. Retain/revisit the latest goal across frames for mechanisms that exhaust the bounded local
   search. No measured reference-linkage case required this; arbitrary nonlinear mechanisms
   still have local minima and the documented work/component limits.

Raw files under ignored `target/lm-research/`: `target-{before,after}-pass{1,2}.json`,
`target-comparison.json`, frozen `sketch_target_bench-{before,after}.exe` and matching PDBs.
The before executable SHA-256 starts `1B2F320ABA97A42D`; after starts `5F49666B55884154`.

## Actual app verification

Ran the app's `ci` build through its control channel with an isolated recovery folder,
front view of XZ, camera distance 100, and Ctrl suppressing snapping. The starting geometry
matches the supplied second image: A = (−10, 0), C = (27.538462, 13.815350), B = (42, 0).
One pointer-move event pulls A upward to cursor (−10, −16).

| Observation | Before | After |
|---|---:|---:|
| A after one event (mm) | (−9.875777, −1.571313) | (−5.299989, −8.479983) |
| Distance from analytic nearest point (mm) | 8.286589 | 1.64e-7 |
| Remaining cursor distance (mm) | 14.429222 | 8.867962 |

The theoretical minimum cursor distance is 8.867962 mm. The after version reaches it without
waiting for another mouse movement. A sixteen-event drag reaches the same pose within 1e-6 mm.
A subsequent one-event direction reversal also reaches its nearest point. Repeating at camera
distance 200 gives nearest-point error 3.27e-6 mm. All four lengths, fixed positions and the
assembly branch remain valid; maximum measured constraint error is below 1e-9 mm.

Own app renders and saved designs are under ignored `target/lm-research/ui-target/`:
`before-result.png`, `after-result.png`, `after-split-result.png`, `after-reverse-result.png`,
`after-zoom-result.png`, corresponding JSON designs/inputs, and `verification.json`.
The visible change is the left crank endpoint rising to its circular locus's closest point,
instead of staying just above its starting position. `ui.render` captured the actual model
because the hidden diagnostic window did not present frames; its renderer reported
`gpu-software`. The instance was closed. These checks establish pointer-to-geometry behavior,
not GPU frame pacing or whole-window input latency. Generated evidence is not committed.

## Regression coverage

The engine tests exercise large targets (including (1000, -1000)), one versus sixteen events,
reverse and stationary targets, release/undo, and the other joint's rocker arc and limits.
Sketch tests check 24 directions at five radii, correction of accepted constraint roundoff,
a fixed axis, rejected candidates/hostile input, reversing across a nearly stationary maximum,
a cursor at the pivot, and the other joint's motion limits. Existing arm-through-an-unreachable-hole
and arc-wrapping regressions remain applicable.

Two independent reviews found and corrected two additional regressions before publishing:
live text edits must request Full when publishing without evaluation, and feasible cursor
motion must refresh driven dimensions before publishing its pose. Regression tests reproduce
the missing redundant ID and stale reference value, and cover both stored and live model geometry.
The ordinary solver and cursor finalization now share the driven-measurement helper.

See [Issue #1976](https://github.com/tangency-labs/solidcraft/issues/1976) and
[PR #1977](https://github.com/tangency-labs/solidcraft/pull/1977) for public app renders,
the measured cursor/oracle comparison plot, review resolution and current validation status.
Generated screenshots, benchmark dumps and local investigation scripts are not committed.
