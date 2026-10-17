# Sketch diagnostic work and measured improvements

The measured large-sketch bottleneck was repeated diagnostic work, especially ordered
redundancy classification, rather than Levenberg–Marquardt iteration. Removing unnecessary
reports and exploiting exact row sparsity reduces that work while preserving the full
published report. This is separate from the [cursor objective correction](sketch-drag-target-objective.md):
less diagnostic work cannot make an incompatible exact mouse pin feasible.

## Analysis requested by each consumer

`Analysis::Full` retains the existing public solve contract: status, DOF, determined flags,
and ordered redundant constraint IDs within the existing component-size limit.
`Freedom` omits redundant IDs; `StatusOnly` also omits rank/DOF and determined flags. Reduced
reports do not certify that their empty diagnostic lists or placeholder DOF are meaningful.
Every level retains geometry placement, feasibility checks, dimension application, continuation
and rollback. Existing public convenience APIs default to Full.

The engine uses these levels according to the report's consumer:

| Consumer | Analysis |
| --- | --- |
| Held drag feasibility trial | StatusOnly |
| AutoConstrain preparation, accepted/rejected constraint trial, ordinary command response | Freedom |
| Intermediate offset solve before generated geometry is refreshed | StatusOnly |
| Final evaluated model report | Full |
| Live drag fallback or live text edit published without model evaluation | Full |

AutoConstrain reuses its preparation report's DOF only on the same solved state. After adding
or rejecting a constraint it obtains the appropriate new result. There is no new persistent
cache or reuse of numerical rank based solely on unchanged topology. Successful live drag
steps retain the pre-existing policy of carrying forward a complete evaluated report; release
evaluates the model. Live text changes must not publish a reduced report.

## Remaining redundancy classification

The full check keeps input row order, ownership, norm calculation, two modified Gram–Schmidt
passes and the existing `left > 1e-6 * norm` decision. It retains coefficients in column order
and drops only exact zeros. Sparse dot/subtract operations skip zero arithmetic; fill-in is
retained. Dense row original storage and the optional explanation system are only allocated
when explanation support is requested. This is not a structural-rank approximation or a
different tolerance policy.

A frozen dense reference is compared against the optimized implementation on 300 deterministic
matrices, including rank deficiency, duplicates, small coefficients and sparse fill-in. Sketch
tests also compare redundant IDs, explanations and ordinary solve reports. The 400-variable
redundancy cap is unchanged; above it, an empty ID list remains unchecked.

## Matched measurements

Native Windows x64, Intel Core Ultra 7 258V, Rust 1.96.0, `ci` profile, `solver-profiling`, seed 99,
scale 1 and translation 0. Each comparison used two passes with reversed version order, two
warmups and ten measured samples per pass (20 samples per version/case). Compilation was
stopped before timing. Fixture construction and JSON serialization are outside the command
timer. Captured timing includes tracing overhead; it is not UI frame time.

For the earlier caller-deduplication comparison (common base `c5992386`), the 14×14 AutoConstrain
command fell from 332.243 to 117.009 ms, four solves to three, and three redundancy checks to
one. The remaining report still supplies all published diagnostics. This is a separate paired
comparison, not a stage to multiply together with the newer timings below.

The subsequent classification comparison rebuilt both versions on common base `9c07bdd8`:

| Grid / AutoConstrain | Before median (ms) | After median (ms) | Ordered redundant IDs |
| --- | ---: | ---: | --- |
| 10×10 | 18.815 | 4.322 | Same 162 IDs |
| 14×14 | 106.564 | 9.287 | Same 338 IDs |
| 15×15 | 8.968 | 8.910 | Unchecked above cap |
| 20×20 | 15.309 | 16.074 | Unchecked above cap |

All 160 measured responses, final sketches and full published reports match across versions
and passes. Each command performs three solves; below the cap it performs one redundancy
check, above it zero. These cases have zero LM iterations. The 14×14 speedup is **91.3%**;
its redundancy phase falls from 99.420 to 1.671 ms, and classification from 98.290 to 1.075 ms.
Its redundancy Jacobian changes only from 0.557 to 0.506 ms. Classification accounted for
99.07% of the original redundancy phase, so it was the right optimization target.

The 15/20 cases do not demonstrate a speedup. For 20×20 the two orders move in opposite
directions; the small pooled difference does not establish an algorithmic regression either.
Performance depends on sparsity, and dense fill-in can retain much of the original cost.

Separate 14×14 drag-command measurements, same paired protocol:

| Operation | Before median (ms) | After median (ms) | Redundancy checks |
| --- | ---: | ---: | ---: |
| Successful live frame, floating grid | 2.616 | 2.642 | 0 |
| Live attempt to move a pinned point | 103.006 | 4.623 | 1 |
| Release after a floating-grid frame | 100.874 | 19.987 | 1 |
| Frame deliberately missing an evaluated report | 196.735 | 38.381 | 2 |

All 160 operation samples preserve responses, reports and geometry. Successful ordinary
frames already bypass redundancy and show no improvement. The missing-report case is an
explicit fallback test, not every first mouse movement. The floating release has a denser
direction basis than the pinned AutoConstrain case, explaining its greater remaining cost.

## Reproduce and next priorities

Build/run `sketch_analysis_bench` and `sketch_drag_bench` as documented in
[Sketch solver measurements](../sketch-profiling.md). Freeze each executable before changing
its algorithm, retain raw JSON and binary hashes, run both orders on an idle machine, and
compare full sketches/reports in addition to time. The guide describes clocks, inclusive
phase overlap and instrumentation overhead.

After this work, ordinary successful frames spend more time outside the solver than inside
it, including command/state handling and live profile publication. For difficult off-locus
point drags, repeated exact held attempts and LM retractions dominate the new cursor work;
null-space basis construction is small on the reference linkage. Profile those paths before
changing damping or adding a larger sparse solver. No measurements here establish whole-window
frame pacing, GPU latency, or a convergence guarantee for arbitrary mechanisms.
