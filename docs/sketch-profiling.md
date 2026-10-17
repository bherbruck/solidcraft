# Sketch solver measurements

Results and priorities: [Diagnostic work and matched before/after results](research/sketch-solver-performance.md).
Drag behavior diagnosis: [A one-degree-of-freedom linkage does not follow the cursor](research/sketch-drag-target-objective.md).

The `solver-profiling` Cargo feature adds opt-in numerical traces and inclusive wall-time
measurements. It does not change residuals, damping, tolerances or the solver's decisions.
Normal builds without this feature compile out the collection sites and clocks.

## Reproduce a measurement

Build once, then run the binary directly so compilation is outside the measurement:

```sh
cargo build --profile ci -p solidcraft-sketch --features solver-profiling --example solve_profile
target/ci/examples/solve_profile --case all --repeats 20 --warmup 2 --out target/sketch-profile.json
```

On Windows the executable is `target/ci/examples/solve_profile.exe`. If another build owns
`target`, add `--target-dir target/lm-research` to Cargo and use the executable under
`target/lm-research/ci/examples/`. Use the same compiler, profile, seed, analysis mode and
machine for comparisons; record the commit separately with `git rev-parse HEAD`.

The six small cases are:

| Case | Purpose |
| --- | --- |
| `distance-zero` | A free point coincident with a fixed point, target distance 10 mm; observes the zero-gradient stall |
| `distance-offset` | The same distance with an initial 0.00001 mm offset; positive control |
| `x-zero` | An unsigned X distance starting at zero X separation; another non-smooth initial point |
| `linkage` | Two triangles with three joining links; leaves a numerical remainder for LM |
| `linkage-edit` | Start with a solved linkage and edit its dimensions; counts retries and continuation if triggered |
| `grid` | An n by n distance-constrained horizontal/vertical grid, perturbed from its solution; shows work eliminated by construction |

Case generation is deterministic for `--seed` (default 99). Each sample starts with a fresh
clone of the same input. Cloning, warmup, comparison, sorting and JSON output are outside the
paired sample timers. The benchmark checks identical geometry and `SolveReport` between each
captured and uncaptured solve, failing with an error if instrumentation changes the result.
Expected unsolved examples are recorded as `Failed`, not treated as benchmark errors.

To compare scale and translation, for example:

```sh
target/ci/examples/solve_profile --case linkage --scale 0.001 --offset 0 --repeats 20 --out target/linkage-small.json
target/ci/examples/solve_profile --case linkage --scale 1 --offset 1000000 --repeats 20 --out target/linkage-translated.json
target/ci/examples/solve_profile --case grid --size 20 --analysis status --out target/grid-status.json
```

`--help` lists bounds and defaults. The requested analysis controls diagnostics, not solving:

| Analysis | Feasibility, placement and rollback | DOF and determined flags | Redundant IDs |
| --- | --- | --- | --- |
| `full` | Yes | Yes | Checked within the existing size limit |
| `freedom` | Yes | Yes, same as full | Not checked; empty list |
| `status` | Yes | Not checked; DOF is a placeholder 0 and flags are empty | Not checked; empty list |

The dimension-edit API always uses full analysis, so `all` and `linkage-edit` require `full`.
An empty list in a reduced report does not certify the absence of redundant constraints.

To time an actual engine caller on the same deterministic grid:

```sh
cargo build --profile ci -p solidcraft-engine --features solver-profiling --example sketch_analysis_bench
target/ci/examples/sketch_analysis_bench --size 14 --repeats 10 --warmup 2 --out target/auto-grid14.json
```

The engine example runs `sketch.auto_constrain` with `dimensions: false` on a fully constrained
grid. Session/fixture preparation and serialization are outside the timer. The measured command
includes its parameter preparation, freedom query, final solve and model evaluation. Each
sample must return solved, DOF 0 and no added constraints; final geometry must match between
samples. Its JSON retains the final sketch, response, full published report and per-sample solve counters/timings for
comparison between binaries. This case exercises the initial freedom query, not successful
constraint additions; acceptance/rejection of those trials is covered by engine regression tests.

For live frames, release and missing-report fallback paths, build `--example sketch_drag_bench`
with the same engine feature, then run `sketch_drag_bench --size 14 --repeats 10 --out
target/drag14.json`. This measures engine commands, not UI frame or mouse latency.

### Initial recorded run (2026-10-17)

Windows x64, Rust 1.96.0, `ci` profile, seed 99, scale 1, offset 0, full analysis,
20 paired samples and 2 warmups, working tree based on `c5992386` with this instrumentation.
Other compilation was active: wall times are illustrative observations, not a controlled
performance baseline. Counters below are from the first measured sample.

| Case | Result | LM outer iterations | Linear trials | Accepted / rejected | Uncaptured median / captured median (µs) |
| --- | --- | ---: | ---: | --- | --- |
| `distance-zero` | Failed, residual 10 | 1 | 25 | 0 / 25 | 44.2 / 59.1 |
| `distance-offset` | Solved | 3 | 3 | 3 / 0 | 17.2 / 23.0 |
| `x-zero` | Failed, residual 10 | 1 | 25 | 0 / 25 | 25.5 / 41.6 |
| `linkage` | Solved | 7 | 7 | 7 / 0 | 95.8 / 117.2 |
| `linkage-edit` | Solved | 14 | 20 | 14 / 6 | 171.3 / 221.0 |
| `grid` (10 by 10) | Solved | 0 | 0 | 0 / 0 | 28419.1 / 28682.4 |

The first `distance-zero` trial records cost 100 → 100, gradient infinity norm 0,
step norm 0, predicted reduction 0, rho null and `rejected`. Its LM stop reason is
`no_improvement`; this distinguishes the stall from geometric inconsistency. The grid has
no LM work; its full-analysis time must not be mistaken for slow LM convergence.
The seed-99 dimension edit succeeds in one LM call, so its continuation counter is zero;
it is not a benchmark of continuation's effectiveness.

Separate smoke runs solved the linkage at scale 0.001 (6 outer iterations) and translated
by 1000000 mm (7), and the 20 by 20 status-only grid without an LM call. An uncaptured
10000-loop linkage workload solved every instance. These are bounded smoke cases, not a
convergence-rate survey across real sketches.

## Read the JSON

`schema` is `solidcraft/sketch-profile/1`. Metadata records platform, package version, seed,
scale, offset, grid size, warmup and analysis. Each case contains raw samples, median and
nearest-rank p95 wall times, plus an uncaptured baseline. With few samples p95 is generally
the maximum: retain the raw samples, and do not call it a stable population estimate.

Each sample has its actual `SolveReport`, work counters and phase times in nanoseconds.
`last_run_trace` retains the final sample's individual LM calls and damping trials:

- `runs`: variable/row counts, initial cost, final maximum residual, and stop reason
  (`residual_tolerance`, `no_improvement`, `iteration_limit`). A stop reason applies to one
  LM call, not the whole sketch, and does not prove geometric infeasibility.
- `trials`: LM call id, outer iteration and attempt (zero based), lambda, before/after cost,
  infinity norm of `Jᵀr`, step norm, predicted/actual reduction, rho and decision. Cost is
  sum(r²). Rho uses the undamped model prediction `-2gᵀp - ||Jp||²`; it is recorded only,
  never used to accept a step. Undefined/non-finite values are JSON `null`.
- Counters count actual work, including rejected trials, linear failures, plain-start
  retries and continuation. `outer_iterations` counts attempted LM outer iterations;
  unlike the legacy report's `iterations`, a stall on the first iteration counts as one.
  Residual-system evaluations count vector requests; residual-block evaluations include
  every constraint/implicit-curve evaluation for finite differences and pinned analysis.
  A block can contribute more than one scalar row.
- `components` includes variable components with no constraint rows. `constructed_variables`
  counts coordinates/radii marked as constructed, not every variable later eliminated.

Timings are **inclusive and overlap**. `solve` contains all work; `seed` covers constructing
the initial placement; `lm` includes residuals, Jacobians, normal equations, linear solves
and diagnostic collection. `analysis` covers final checking, rank/redundancy analysis and
writeback. `jacobian` includes its base residual evaluation. `residual_system` includes a
Jacobian if requested through the full-system path. Never add these fields to estimate total
time. `redundancy` measures dependency classification including its Jacobian and any requested
explanation; `redundancy_checks` counts those calls, including standalone explanations when
captured. Its child phases `redundancy_jacobian`, `redundancy_classification` and
`redundancy_support` separate Jacobian construction, ordered orthogonalization/row storage,
and optional explanation work; they overlap their parent, and the first overlaps `jacobian`.
Reduced solve reports should perform none. Pinned-analysis work before/during LM is visible in counters and its enclosing time;
the phases are not an exhaustive exclusive CPU breakdown.

Traces retain at most 1024 LM calls and 10000 trials per capture. `dropped_runs` and
`dropped_trials` indicate truncation; aggregate counters continue. Native clocks are enabled;
on wasm `timings_available=false`, all times are zero and counters/traces remain usable.

The captured measurement includes clocks, TLS checks, trace allocation and the extra model
prediction calculation. The uncaptured baseline avoids collection and model prediction but
still has the feature's inactive collection checks. For absolute production overhead compare
with a build without the feature, e.g. the existing `solve_bench` example. Busy-machine wall
times are observations, not evidence of a speedup.

## Sample CPU stacks

### Matched four-bar cursor benchmark

```sh
cargo build --profile ci -p solidcraft-engine --features solver-profiling --example sketch_target_bench
target/ci/examples/sketch_target_bench target/four-bar.json 10
```

The fixture uses 10/40/20/42 mm lengths and one angular degree of freedom. Each target is
replayed as one event and as 16 events. Ten measured repetitions follow two warmups; construction
and JSON serialization are outside the timing. The output includes constraint error, assembly
branch validation, distance to the cursor, distance to the analytic nearest point and captured
solver work. Freeze the executable before changing the algorithm; run both versions with the
same profile/features on an idle machine, preferably in both orders.

`timings_ns.cursor` includes feasible point optimization, `cursor_null_space` its basis
construction, and `lm` includes retractions as well as ordinary solves. These times overlap.
`cursor_calls`, `cursor_steps`, `cursor_retractions` and `cursor_budget_stops` distinguish target
optimization from exact held solves; retractions do not increment `solve_calls`.

### External sampler

`--cpu-loops` repeatedly solves fresh copies **without** capture, retaining only counts and
total elapsed time. Cloning is included in this mode. This supplies a sustained workload for
an external sampling profiler; it avoids growing a trace or serializing JSON inside the loop.

```sh
cargo build --release -p solidcraft-sketch --features solver-profiling --example solve_profile
perf record --call-graph dwarf -- target/release/examples/solve_profile --case linkage --cpu-loops 10000 --out target/linkage-cpu.json
perf report
```

On Windows, launch the same executable with those workload arguments under a native CPU
sampling profiler. Build before recording and choose enough loops to obtain useful samples.
The repository's release profile retains line tables. Keep compilation and output serialization
out of the selected time range; solver CPU samples and inclusive phase wall times measure
different things.

Windows Performance Recorder may require a system profiling policy/privilege. On the initial
machine `wpr -start CPU -filemode` failed with `0xc5585011` (could not enable the policy to
profile system performance). No ETL was produced and no system policy was changed. The phase
measurements and CPU workload above do run without that privilege.

## Measure an existing caller

Enable the feature for `solidcraft-sketch` in the developer build, then wrap the actual API:

```rust,ignore
let (report, profile) = solidcraft_sketch::profiling::capture(|| {
    solidcraft_sketch::solve_after_value_edit(&mut sketch, &old_values)
});
// Serialize `profile` outside the measured closure.
```

Capture is per-thread; work spawned on another thread is not included. Nested captures are
isolated and restore the outer collector, including on unwind. Use a normal example/program
for production measurements: unit-test builds perform additional differential solver checks,
and the counters correctly include that additional work.

## Validation

Profiling tests cover observational equivalence, zero-gradient diagnosis, rho for a locally
linear residual, nesting/unwind restoration, bounded trace storage and non-finite values.
Analysis-level tests compare geometry and the requested report fields. Command integration
tests cover report publication, live text changes, offsets and parameter application. Run the
feature-enabled suites explicitly as well as the default-feature repository gate:

```sh
cargo test --profile ci -p solidcraft-sketch --features solver-profiling --all-targets
cargo test --profile ci -p solidcraft-engine --features solver-profiling --test sketch_analysis --test sketch_drag_target
cargo check --profile ci --target wasm32-unknown-unknown -p solidcraft-engine --features solver-profiling
cargo xtask ci
```

Current run results and integration status belong in the linked PR, rather than being inferred
from these reproduction commands.
