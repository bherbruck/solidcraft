# Design loading

## Edges made by booleans: each point evaluated once (2026-10-16)

A boolean meshes both operands with truck-meshalgo, which first divides every edge into a
polyline (`bounded_polyline`, vendored). An edge that an earlier boolean made is an
intersection curve. Each of its points is found by a grid search on each of the two surfaces,
51 × 51 evaluations of a NURBS surface each, before Newton. The division evaluated both ends of
every piece again, so 2 of its 4 evaluations per piece repeated a point it already had.
Profiles of `main` (01b1b705) on the machine below put the division at 42 % of the CPU time
of opening the rover, 58 % of `bench-agent` and 82-85 % of a drilled hub or a union of tubes.

Each piece now carries the points of its ends, and its midpoint is both the probe and an end of
both halves: 2 evaluations per piece. The fallbacks for samples that do not evaluate (#1364) are
kept: a midpoint that fails is NaN as a probe and the curve's approximation as an end, exactly as
before. The polyline is the same, bit for bit: `edge_polyline_tests` compares it with the
previous division (points, order and number of evaluations) on the intersection curves of a
drilled cylinder and on a circle.

Release `solidcraft-cli` from `main` (73493b94) and the same with the change, 3 alternating
pairs (before/after, after/before, before/after), each run a new process timed with
`/usr/bin/time -v`, started once the 1-minute load average was below 2.5. Medians, with the
range of the 3 runs. 2018 laptop: Intel Core i7-8565U (4 cores / 8 threads, 15 W), 7.4 GB,
Ubuntu (Linux 7.0), on mains power:

| Run | Evaluation | Meshes | Wall time | CPU time | Peak RSS |
|---|---:|---:|---:|---:|---:|
| `bench-open`, `examples/acceptance/sensor_enclosure.json` | 3,203 → 3,089 ms | 6 → 6 ms | 3.2 → 3.1 s | 8.7 → 8.5 s | 91 → 91 MB |
| `bench-open`, a hub barrel with one radial hole cut through it | 966 → 817 ms | 878 → 481 ms | 1.9 → 1.4 s (-24 %) | 4.4 → 2.8 s (-37 %) | 27 → 30 MB |
| `bench-open`, the frame of the `bench-agent` bicycle (one union of tilted tubes) | 8,204 → 6,277 ms | 1,711 → 893 ms | 9.9 → 7.2 s (-28 %) | 33.3 → 20.6 s (-38 %) | 44 → 46 MB |
| `bench-open`, a rover (3,324 features, 1,395 bodies) | 26,783 → 22,792 ms | 7,960 → 6,046 ms | 35.7 → 29.8 s (-17 %) | 129.0 → 101.9 s (-21 %) | 2,076 → 2,079 MB |
| `bench-agent` (build 33.5 → 30.2 s, -10 %) | 10,553 → 8,565 ms | 2,778 → 2,081 ms | 47.6 → 41.1 s (-14 %) | 124.0 → 91.0 s (-27 %) | 1,428 → 1,432 MB |

`bench-agent`'s evaluation and meshes are those of reopening the saved design. CPU time is the
steadier measure on this 15 W processor: its before and after ranges are apart for the four
cases with boolean edges (the hub 3.0-4.4 s before, 2.5-2.9 s after; the rover 113.6-133.5 s,
101.0-102.5 s), while the first pair's "before" run, which started on a cool processor, overlaps
the "after" wall times of the two short cases. The sensor enclosure does not change: its time goes
to grid searches in face meshing, which this does not touch. An earlier series on `main`
(8535d229), on a quieter machine, gave -38 %, -35 %, -18 % and -16 % of wall time for the same
four cases. The CPU time saved is close to half the division's share in the profile. The mesh
phase gains as well, so these display meshes also call the reference mesher somewhere (where is
not traced here; `tess::edges`, the display plans' own edge division, already evaluated each
point once). In the rover, the second shell (which names every body's faces, so meshes them all)
goes from 7.8 s to 5.7 s.

No model changed. `solidcraft-cli eval` of the five designs (the bicycle as `bench-agent` saved
it) is the same with both binaries apart from the per-feature milliseconds: bodies, volumes,
areas, centres, bounding boxes, topology counts and warnings. `bench-open` meshes the same
triangle counts, and the bicycles both binaries save differ only in the document's name.

## STEP import: bounded surface boundaries and periodic projections (2026-10-15)

Generated closing curves share the edge sampler's 24-level and 20,000-point limits,
preventing runaway subdivision across rational surface poles. Periodic trim parameters
use the nearest equivalent to the previous point, avoiding thousands of unnecessary turns.

Boundary projection accepts local and cached answers within the caller's chord tolerance
and surface domain before global retries.
Global retries reuse a lazy 51×51 surface sample grid across each face's loops, avoiding
repeated B-spline/NURBS evaluation to choose a seed. Failed cached searches retain the
surface's native bounded projection and continuation fallback. Projection writes directly
to the trim buffer without per-point allocations or copying the whole input loop.

Release CLI measurements from October 14–15, 2026, on the same machine, before projection caching:

| STEP model | Phase | Before | After |
|---|---|---:|---:|
| 9.47 MB, 29 bodies | Import, evaluation and display meshing | Still importing at 54 s | 5.16–5.41 s (3 runs) |
| 63.86 MB, 1,058 bodies | Import and body preparation | Stopped while importing at 181.6 s | 89.9 s (1 run) |

The assembly comparison used upstream `e4ef6cfe2946`, one run per version.
The corrected run included temporary profiling output; its subsequent evaluation
took 0.26 s and display meshing took 11.57 s.

## STEP assemblies: a repeated part is meshed once (2026-10-14)

The reader already converts each B-rep item once and keeps it for the instances that list it
again. Every instance still became a body of its own geometry (it has to: features cut and join
it where it is), and each of those bodies made its own display mesh: tessellation, seam and
tangent flags, the most expensive step of opening a large assembly. A board with 60 of each
resistor meshed the same resistor 60 times.

A later instance of an item now shows its first instance's display mesh, placed by the
difference of the two placements: the same triangles, faces and edge flags, moved. The mesh is
made once, by whichever instance is drawn first. Only a rotation and translation is shared
this way (anything else meshes itself), and anything that changes what a body shows (split
lines; any operation, which makes a new body) drops the link. Measurements, booleans and export
read the instance's own geometry as before, so `solidcraft-cli eval` of an assembly is unchanged.

Two generated assemblies (SolidCraft designs with placed copies of components, exported as STEP
AP214 assemblies), `solidcraft-cli bench-open`, release build, shared 16-core Linux box:

| file | bodies | mesh phase before | after | CPU before | after | peak RSS before | after |
|---|---:|---:|---:|---:|---:|---:|---:|
| 4 parts, 500 instances (bolts with a torus ring, brackets) | 901 | 2,215 ms | 139 ms | 28.4 s | 4.1 s | 918 MB | 749 MB |
| 10 filleted parts with 8 pins, 600 instances | 5,401 | 1,028 ms | 85 ms | 16.7 s | 6.0 s | 1,095 MB | 783 MB |

Both import to the same triangle counts (4,404,412 and 4,316,412). Each instance's display plan
(the GPU viewport draws bodies from plans) is still made per instance; sharing those the same
way is the next step.
## Designs built by agents: work per command that grew with the design (2026-10-14)

A design an agent builds over MCP is made of many small commands: a cylinder per spoke, a box
per chain plate, one part built at the origin and its component moved into place. Each command
did work over the whole design, so the 1,000th command cost far more than the first and a
1,300-body design took minutes to build and to show. `solidcraft-cli bench-agent` builds such a
design (a generated bicycle: about 1,260 commands, 1,280 bodies, 1,250 features, wheels built at
the origin and moved), timing each command and then what a window needs to draw its result (the
world state and its display meshes), and finally opens the saved design like `bench-open`. The
script is generated, so it needs no fixture; `--scale` changes the counts and `--script-out`
writes the commands for replaying elsewhere (`engine.execute` one by one over the control
channel reproduces the desktop case).

What grew with the design, per command:

- **Moved components were placed again after every change.** `world_state` transformed every
  body of a component whose occurrence is moved and gave it a new, empty display mesh, so every
  command meshed all those bodies again (the two wheels: 150 bodies and two tori, 130-190 ms per
  command). It now keeps the bodies it placed and reuses each one whose definition (same body,
  same display cell) and placement are unchanged.
- **A free `dN` name searched the design once per number** (`next_d_name`), quadratic in the
  number of named inputs, for every new feature input.
- **Every expression evaluated every parameter** (`Document::eval`), several times per
  feature command (each coordinate, each size). Parameters are now evaluated only when the
  expression names one.
- **The parameter table's `used_by` searched all parameters for each one** (quadratic); it is
  one pass now. The desktop also stops gathering expression completions (eleven parameter
  tables) on every change while no text box has focus.

`solidcraft-cli bench-agent`, release build, 16-core Linux shared with other builds (load
average 15-20), one run each:

| per command, by design size | before: command | before: display | after: command | after: display |
|---|---:|---:|---:|---:|
| 200 commands (227 bodies) | 6.0 ms | 129 ms | 1.8 ms | 2.7 ms |
| 600 commands (621 bodies) | 19.9 ms | 169 ms | 4.9 ms | 2.4 ms |
| 1,000 commands (1,021 bodies) | 45.1 ms | 187 ms | 9.8 ms | 4.5 ms |
| 1,200 commands (1,221 bodies) | 56.8 ms | 194 ms | 12.5 ms | 5.6 ms |
| whole build (1,260 commands) | 241 s | | 16.3 s | |

The command column still grows (the timeline's fingerprints and parameter values are walked
once per command), but slowly. `solidcraft-cli eval` output for the design corpus, the examples,
the regression scripts, the generated bicycle and a 3,300-feature agent-built design is
unchanged apart from the timings.

The viewport rebuilt its whole model scene after every change and uploaded all of it again
(every body's triangles and edges, 60 MB for the bicycle where bodies are built on the CPU). It
now builds the scene in blocks of a few bodies, cut where a body's own key says so that adding
or removing a body changes only its block, and keeps each block's bytes and GPU buffers while
the block comes back unchanged. The bodies' boxes (fit, picking, the ground shadow) are kept per
mesh the same way. Built command by command over the control channel (`engine.execute`, then
`ui.inspect`) in the desktop app under Xvfb with software GL:

| desktop, bicycle built over the control channel | main | with #1148 and this |
|---|---:|---:|
| UI work per change at ~1,000 bodies (`frame_ms`) | 1,374 ms | 18 ms |
| model scene build at ~1,000 bodies (`build_ms`) | 41 ms | 1.4 ms |
| whole build | 1,409 s | 210 s |

Most of what is left there is software rendering of 770k triangles per frame.

## STEP import: shells and item solids without copies (2026-10-13)

A file that lists one product definition many times (a bolt used 70 times, a panel in several
assemblies) is imported item by item, and every item's B-rep was deep copied twice before it was
handed on: once by the caller of `Solid::try_new`, which clones the shells so that the failure
path still has them, and once by the reader's own cache, which kept the solid for a later item
that might list the same definition.

`read_item` now runs those four checks (non-empty, connected, closed, no singular vertices) on
borrowed shells, so a shell that passes is moved into the solid instead of cloned. The reader
keeps each prepared solid in an `Arc`, and an item counts as *once* when one representation
lists it and exactly one product-definition walk reaches it: that item's solid is moved out of
the cache with `Arc::try_unwrap` instead of copied, because no later item can ask for it. The
walk follows `MAPPED_ITEM` targets too, so an item reached through a mapping in another product's
node is still converted once - a second conversion would repeat that item's warnings. A file
whose walk exceeds the work budget (`MAX_CLOSURE_WORK`, 65 536 nodes) falls back wholesale to
the copying path, as does any file with component occurrences, which walks a definition again per
instance.

Nothing observable changed. Fifteen fixtures (primitives, a void, a fillet, a revolve, colours,
centimetre units, 70 duplicates of one definition, a broken file with mappings, a two-instance
assembly and an expanded one; 99 bodies) give the same import dump - name, path, closed flag,
file faces, face origin, topology and merged counts, bounding box, volume, area, centroid, paint,
warnings and tree - apart from the per-feature milliseconds, and all fifteen STEP in/out round
trips are byte-identical.

Two generated flat STEP files, timed like the table above (same binary pair, no other work
running); four alternating runs each, two for the 31 MB file:

| File | Baseline peak RSS | Patched peak RSS | Import |
|---|---:|---:|---:|
| 2500 boxes, 9.5 MB | 150.1 MB | 145.9 MB | 1.81 s → 1.74 s |
| 2500 filleted boxes, 31 MB | 428 MB | 433 MB | 70.3 s → 71.0 s |

The first row's ranges do not overlap (150.0-150.6 MB against 145.0-146.0 MB), and the saving is
the shell copy that is no longer made for every item. The second file's runs land between 422 MB
and 444 MB either way, so two runs each do not resolve it: there the conversion of the geometry
dominates the per-item copy. An item that keeps every conversion - an assembly, or a file over the
work budget - is unchanged.

## Display tessellation plans (#303, 2026-10-12)

`Body::display_mesh` meshes from a `TessPlan` (`crates/kernel/src/tess`): every edge is
sampled once and both faces use those nodes exactly (watertight by construction); planar
faces are a constrained Delaunay triangulation of their edge nodes alone; faces bounded by
their parameter box are structured grids zipped to the edge nodes; other four-cornered faces
are Coons grids; only genuinely trimmed curved faces (holes, more than four corners) use
spade's constrained Delaunay with sag and Ruppert refinement. A body the planner cannot
handle is meshed by truck-meshalgo as before, which also still meshes for modelling
(booleans, measure, provenance). Evaluating a plan (surface points and normals) runs on the
CPU (`TessPlan::evaluate`) or in a compute shader (`solidcraft-gpu`).

Release build, Apple M4 Max, medians of five. 400-hole plate (200 × 200 × 5 mm, 806
faces), `cargo test --release -p solidcraft-gpu bench_plate -- --ignored --nocapture`:

| Tolerance | Reference mesher | Plan | CPU evaluation | GPU evaluation | Triangles (reference / plan) |
|---|---:|---:|---:|---:|---:|
| 0.3 mm | 23.3 ms | 11.9 ms | 1.1 ms | 1.1 ms | 20,812 / 20,812 |
| 0.03 mm | 54.2 ms | 31.6 ms | 2.5 ms | 1.9 ms | 76,812 / 52,812 |
| 0.006 mm | 118.9 ms | 65.1 ms | 5.3 ms | 3.1 ms | 302,412 / 104,012 |

Planning (edge sampling, parameter projection, triangulation) dominates; evaluation is a
few milliseconds either way, so GPU evaluation saves little until planning is cheaper. The
GPU timing includes upload and read-back.

## Parallel STEP body preparation (2026-10-12)

Profiling an original 48-cylinder STEP fixture before changing the importer measured
3.17 ms parsing, 6.06 ms reconstructing B-reps and 45.58 ms preparing/orienting bodies.
The five complete imports had a median of 59.21 ms. The expensive orientation mesh,
rather than the text parser, was the first parallelization target for #287.

Desktop import now prepares consecutive independent solids in a representation with
up to eight workers. Each batch contains at most 64 solids. Parsing, B-rep reconstruction,
assembly traversal, first-use unit resolution, naming and result collection remain ordered.
Mapped items end a batch. Fewer than four solids use the serial path; wasm always uses it.
Thread creation failure falls back to serial work, and kernel/worker failures remain
per-body errors reported by the importer.

Windows x64, AMD Ryzen AI Max+ 395 (16 cores / 32 logical processors), optimized `ci`
profile, no concurrent build: three fresh test processes per fixture, five alternating
serial/parallel pairs in each process. These are complete uncached kernel imports,
including parsing and body preparation; fixture generation/export and later desktop
display meshing are outside the timer. Both paths use the same input text and code.

| Original generated fixture | Bodies | Serial median | 8 workers median | Reduction |
|---|---:|---:|---:|---:|
| Cylinders, radius 5 mm, height 12 mm | 48 | 57.54 ms | 17.38 ms | 69.8% |
| Cylinders, radius 5 mm, height 12 mm | 512 | 896.31 ms | 313.06 ms | 65.1% |
| Tori, major radius 20 mm, minor radius 4 mm | 48 | 269.05 ms | 51.90 ms | 80.7% |
| Boxes, 10 × 15 × 12 mm | 512 | 247.97 ms | 90.15 ms | 63.6% |
| Boxes, 10 × 15 × 12 mm | 4 | 1.49 ms | 1.02 ms | 31.5% |
| One cylinder (serial fallback) | 1 | 1.22 ms | 1.29 ms | serial fallback |

There are 15 samples per cell. The single-cylinder modes run identical serial code; their
0.07 ms median difference is within the overlapping 1.11–1.57 / 1.13–1.52 ms ranges. The 512-cylinder runs ranged from 567.40–983.61 ms
serial and 197.72–345.51 ms parallel; scheduling/thermal variation is material. These
synthetic results do not establish a speedup for every assembly or complete desktop Open.
In particular, parts containing only one solid are not parallelized across assembly nodes.
Cross-part scheduling, parallel B-rep reconstruction and later display meshes remain open.

Reproduce after compiling, without other builds running:

```sh
cargo test -p solidcraft-kernel --profile ci benchmark_independent_step_solids --no-run
SOLIDCRAFT_STEP_BENCH_COUNT=512 SOLIDCRAFT_STEP_BENCH_KIND=cylinder \
  cargo test -p solidcraft-kernel --profile ci benchmark_independent_step_solids \
  -- --ignored --nocapture --test-threads=1
```

Use `torus` or `box` for the other fixtures and repeat in three fresh processes.
`SOLIDCRAFT_STEP_BENCH_FIXTURE` optionally writes the original STEP for native inspection.
No timing threshold runs in CI. Regressions compare serial and threaded imports with
different worker counts: exact display meshes, measures, body/face source identities,
names, colours/opacity, units, rotated assembly placements, duplicates and partial failures
across the batch boundary. The public analytic/FreeCAD reference suite also checks STEP
export/import; its current results are reported in docs/oracle.md.
## Design storage: container and binary payloads (#303, phase one)

Design containers avoid formatting/parsing imported mesh coordinates as JSON numbers.
The metadata serializer replaces embedded fields before creating JSON values, without
cloning the full document or duplicating its schema. Binary coordinates retain `f64`
precision, indices retain `u32`, and SHA-256 deduplicates identical payloads. The reader
checks archive budgets, CRCs, hashes, exact lengths, finite coordinates and index bounds.

Reproduce the storage measurements with an optimized `ci` IO test executable:

```sh
cargo test -p solidcraft-io --profile ci benchmark_ -- --ignored --nocapture --test-threads=1
```

The benchmarks generate an original 100,000-vertex/100,000-triangle mesh and a STEP
containing 512 disjoint 10 mm boxes. Fixtures are written under `CARGO_TARGET_DIR`
(or the crate's `target` directory). Each compares nine encoding/decoding samples;
atomic-save samples include encoding, temporary-file flush, replacement and `.bak`
creation and alternate JSON/container ordering. Documents are compared after decoding.
These are storage measurements, separate from timeline evaluation or first-frame latency.

On Windows x64, based on upstream `e6e0a7e` before STEP parallelism (#312), three
consecutive optimized runs without concurrent compilation gave
the following medians of the three nine-sample medians (milliseconds, fast mode):

| Fixture / operation | JSON | Container |
|---|---:|---:|
| Mesh size, bytes | 15,139,115 | 3,600,828 |
| Mesh encode | 23.336 | 6.094 |
| Mesh decode | 57.175 | 2.962 |
| Mesh atomic save | 52.924 | 14.856 |
| STEP size, bytes | 1,998,793 | 1,950,056 |
| STEP encode | 1.507 | 1.473 |
| STEP decode | 1.479 | 1.318 |
| STEP atomic save | 7.213 | 7.087 |

Three alternating fresh-process `solidcraft-cli bench-open` runs of each STEP encoding
returned the same 512 bodies and 6,144 triangles. Median read-plus-parse was 2.274 ms
for JSON and 2.151 ms for the container; median evaluation was 218.559/236.915 ms and
display meshing 203.160/197.790 ms respectively. Geometry dominates and these variable
results do not establish an end-to-end STEP opening improvement. Fresh processes do
not imply a cold operating-system file cache, and these are not desktop first-frame times.

Fast mode stores STEP and mesh payloads directly. Explicit `"compression":"compact"`
compresses them at Deflate level one; it is a space/CPU tradeoff. In the initial
512-box STEP experiment, compression reduced 1,998,793 JSON bytes to 578,659 package
bytes but increased median encoding from 1.68 to 5.02 ms and decoding from 1.76 to
4.18 ms. That regression is why compression is not the default. PNG/JPEG are stored
directly in either policy; small parametric metadata uses Deflate level one.

Plain JSON remains a supported Save As/export format; `.json` saves keep JSON and
scripts may explicitly select either format. The CLI detects packages before attempting
JSON parsing. Desktop/browser recovery and browser downloads also use the container.
See [the versioned container format](design-container.md) for layout and bounds.

This phase does not restore evaluated B-reps, skip STEP import, or make cold geometry
rebuilds instant. A matching cache key is not enough to trust unvalidated geometry.
The native B-rep increment described below supplies provisional display, not editable
session restoration. It is separate from #287's import parallelism.

## Opt-in mesh preview measurements (#303, phase one)

The phase-one desktop implementation can show an experimental saved picture while the file worker rebuilds geometry.
It requires an explicit `preview: true` engine Save/Save As call. Ordinary saves omit
display meshes to avoid the measured overhead; these measurements predate native B-rep caching.
The cache contains only already warm evaluated display meshes, capped at 8 MiB, 1,024
bodies and 100,000 triangles. Larger or cold models save without it. Cached geometry is
never used for editing, measurements or picking. Source/document fingerprints and
payload validation reject stale or malformed caches; cancellation and document/revision
checks prevent a background result from replacing newer work.

The cached picture is prepared on the file worker from the same file snapshot as the
normal Open command. It adds work before reconstruction, rather than bypassing it, so
measure preview readiness and full editing readiness separately. Cached display also
adds file size and save work; the storage-only results above exclude that optional data.

For an engine-level save benchmark, first generate the original 512-box STEP fixture
with the ignored IO benchmark, then run the warmed-session benchmark:

```sh
cargo test --workspace --profile ci benchmark_step_storage -- --ignored --nocapture --test-threads=1
cargo test --workspace --profile ci benchmark_warm_design_save_with_preview -- --ignored --nocapture --test-threads=1
```

Set `CARGO_TARGET_DIR` to the workspace's absolute target directory in both commands.
The engine benchmark alternates nine JSON/container atomic saves, includes the optional
display cache, and verifies both preview decoding and authoritative document identity.
Three runs on Windows, with no concurrent build or test suite, measured median atomic
JSON saves of 7.679/7.610/7.630 ms and container-with-preview saves of
10.319/10.633/10.541 ms. Preview decoding took 0.964/0.961/1.130 ms; the container
was 2,510,490 bytes. This optional preview costs save time on this STEP fixture.
An actual desktop sensor-enclosure opening on e8a8e0f prepared its saved picture in
24.320 ms; geometry reconstruction, display warmup and thumbnail preparation took
8,913.212 ms. The same preview experiment is now explicitly opt-in; full geometry
readiness still requires reconstruction.
For actual desktop opening phases, launch with `SOLIDCRAFT_PROFILE_OPEN=1`: `ready_ms`
reports saved-picture preparation, while the normal phase log reports reconstruction,
display warmup and thumbnail completion. Neither log measures the exact GPU first frame.

## Native cache opening (#303)

The portable native B-rep cache is opt-in with `cache: true` on warm container
saves, without duplicating display meshes. Ordinary saves and browser downloads
default to `cache: false`: the measurements below do not justify imposing the
additional save time and file size on every design. Desktop Open reads metadata and
optional geometry through a seekable file, produces a bounded provisional picture,
then resolves original sources and reconstructs the complete session before editing.
This improves the opportunity for early display, not the cost of authoritative feature
evaluation. An absent, invalid or incompatible cache uses the original reconstruction.
Browser OPFS/downloads share the format; browser opening still rebuilds synchronously.

The reproducible engine benchmarks separate atomic saving, provisional mesh readiness,
reconstruction and final display-mesh readiness:

```sh
cargo test -p solidcraft-io --profile ci benchmark_step_storage -- --ignored --nocapture --test-threads=1
cargo test -p solidcraft-engine --profile ci benchmark_native_cache_save -- --ignored --nocapture --test-threads=1
cargo test -p solidcraft-engine --profile ci benchmark_native_cache_open -- --ignored --nocapture --test-threads=1
```

Use an absolute `CARGO_TARGET_DIR`. The save benchmark defaults to the generated
512-box STEP document and accepts `SOLIDCRAFT_BENCH_FILE` for another public design.
It alternates nine saves of each encoding, including flush/replacement/backup, and
verifies document identity and native-cache presence. It never times initial fixture
evaluation as save work. Open runs once per process; `SOLIDCRAFT_BENCH_ENCODING`
selects 0=JSON, 1=container without cache, 2=native cache, 3=compact native cache.
Changing only `cache/key.json`'s application version provides a cache-miss control.
Fresh processes do not imply a cold operating-system file cache. The engine callback
time is provisional mesh readiness, not a rendered desktop frame; use the desktop
profile log and actual screenshots to check the saved picture and completed model.

Measured 2026-10-13 on Windows 11, Ryzen AI MAX+ 395 (16 cores/32 threads), `ci`
profile, with the neutral codec revision `75381fa`, envelope `SCBREP03`, producer
`solidcraft-truck-adapter/2`, and opening integration revision `3243422` on upstream `2880525`
(0.6.0). These replace the earlier prototype measurements. They compare encodings
on the same code and do not attribute upstream modelling improvements to the cache.
Runs started after local builds and the complete CI suite finished; ordinary
desktop activity remained. Save values are medians of nine alternating atomic
saves. Open values are medians of five fresh processes per encoding, alternating
forward/reverse order. Memory is each process's peak working set from Windows
`GetProcessMemoryInfo`, sampled every 10 ms, then the median across those five
processes; it excludes a desktop window/GPU.

The original STEP fixture is a 32 by 16 array of 10 mm cubes, 12 mm apart, exported
by SolidCraft: 512 bodies and 6,144 final triangles. Its JSON embeds about 2 MB of
STEP. Every encoding reopens without feature errors and with those same counts.

| STEP encoding | Bytes | Atomic save, ms |
|---|---:|---:|
| JSON | 1,998,826 | 12.616 |
| Container, no cache | 1,950,084 | 10.516 |
| Native cache, fast | 4,694,035 | 47.755 |
| Native cache, compact | 837,067 | 44.015 |

| STEP encoding | Provisional mesh, ms | Reconstructed, ms | Final meshes, ms | Peak working set, MiB |
|---|---:|---:|---:|---:|
| JSON | none | 175.842 | 748.911 | 40.102 |
| Container, no cache | none | 175.885 | 722.306 | 40.504 |
| Native cache, fast | 62.099 | 228.794 | 747.976 | 40.883 |
| Native cache, compact | 65.981 | 235.403 | 762.120 | 40.918 |
| Native cache miss | none | 169.039 | 647.617 | 40.453 |

The public `examples/acceptance/sensor_enclosure.json` script, saved as a JSON
design, has 21 features, two bodies and 4,364 final triangles on this revision.

| Sensor enclosure encoding | Bytes | Atomic save, ms |
|---|---:|---:|
| JSON | 52,281 | 5.243 |
| Container, no cache | 5,545 | 4.969 |
| Native cache, fast | 379,095 | 12.646 |
| Native cache, compact | 72,603 | 14.111 |

| Sensor enclosure encoding | Provisional mesh, ms | Reconstructed, ms | Final meshes, ms | Peak working set, MiB |
|---|---:|---:|---:|---:|
| JSON | none | 3,979.063 | 3,984.593 | 70.438 |
| Container, no cache | none | 4,038.127 | 4,042.006 | 71.789 |
| Native cache, fast | 241.541 | 4,269.124 | 4,274.272 | 70.465 |
| Native cache, compact | 236.739 | 4,271.254 | 4,275.159 | 70.148 |
| Native cache miss | none | 4,136.517 | 4,140.903 | 69.941 |

Native display is available earlier, at the cost of encoding, file size and work
before reconstruction. It does not make the full design editable earlier, and
these results do not establish a memory reduction. Final-mesh readiness varied
from 611 to 856 ms for STEP and 3.84 to 4.60 s for the enclosure; small differences
in full-opening medians should not be read as modelling speedups. Compact storage
reduces this STEP example's size and atomic-save time, but costs slightly more save
time on the enclosure. The default `cache: false` retains the phase-one save path;
`compression: "compact"` is an explicit space/CPU choice. Larger or unsupported
geometry can exceed the documented budgets and reopen without a provisional image.

A separate desktop check used a fresh 0.6.0 application process for each enclosure
encoding, with no local compilation or test suite running. The native cache's
provisional picture was prepared at 429 ms and captured visibly during opening;
the completed model was adopted in 6.48 s. JSON had no provisional picture and
completed in 6.08 s. These are single visual checks, not repeated timing medians;
the worker reported 6.10/5.66 s respectively, and external observations include
UI scheduling and screenshot requests. During native opening, 94 control-channel
inspections had a median round trip of 20.8 ms and a maximum of 106.5 ms (JSON:
95 inspections, 20.3/36.9 ms). The provisional picture is explicitly labelled as
being checked before editing; the final frame shows the rebuilt Case and Lid and
their editable feature timeline. This demonstrates early feedback, not faster
completion of feature evaluation or a worst-case UI-latency guarantee.

## Analytic extrusion source preparation

New-body extrusion certificates now prepare the sketch's bounded analytic segments once
per certification pass instead of allocating segments and resolving their points for each
generated cap boundary and side face. Saved designs run this same code during cold rebuilds.
Exact containment tolerances, ambiguous-source rejection and the certification work budget
are unchanged. Prepared segments borrow an immutable evaluated sketch and are not retained
across edits.

The reproducible microbenchmark is:

```sh
cargo test -p solidcraft-doc --profile ci benchmark_prepared_extrude_sources -- --ignored --nocapture
```

Three Windows x64 optimized `ci` runs, during other compilation, compared the previous
segment reconstruction loop and the prepared matcher in the same process. For 1,000
analytic lines and 1,000 generated face segments, median time fell from **45.90 ms to
12.30 ms** (73% less, about 3.7x faster). Preparation is included and every returned source
identity is compared. This measures certification only, not complete extrusion, file-open
latency or Boolean operations; it does not establish an end-to-end loading speedup.
Remaining cold geometry rebuild and import costs are tracked in
[#255](https://github.com/bherbruck/solidcraft/issues/255); certification work is
[#254](https://github.com/bherbruck/solidcraft/issues/254).

## Cold geometry rebuilds after 0.3.1

Three alternating cold-process comparisons on the same Windows machine and optimized
`ci` profile, with no concurrent build, compare the unchanged 0.3.1 source with the
straight-line projection and exact surface-refinement reuse changes:

| Saved design | Before, median evaluation | After | Reduction |
|---|---:|---:|---:|
| Public sensor enclosure, 21 features, 52 KB | 11.96 s | 8.85 s | 26.0% |
| Local Pi case assembly, 25 features, 2,990 bodies | 14.75 s | 11.57 s | 21.6% |

These are geometry rebuild times, not complete desktop opening times. The public fixture
can be generated with `solidcraft-cli run examples/acceptance/sensor_enclosure.json
--save sensor.solidcraft --quiet`, then measured with `solidcraft-cli bench-open
sensor.solidcraft`. Each benchmark starts a new process; this does not measure a warm
timeline-cache hit. The local assembly is private and is not part of the repository.

Straight segments now use guarded algebraic closest-point projection instead of sampling
65 points and running Newton iterations. Other curve types retain their existing behavior.
Curved-face refinement reuses derivative and midpoint evaluations with exact finite input
keys and bounded, local caches. It keeps the original tolerance, seam and singularity
checks, refinement passes, and triangle limits.

Identity assembly placements retain the definition body's shared display-mesh cache.
Changed placements continue to build separate meshes in their own coordinates. Face-name
preparation remains eager during placement: deferring it merely shifted expensive work to
thumbnails and the first face query, so that approach was excluded.

Both saved fixtures retain exactly the same body names, face/edge/vertex/shell counts,
bounds, volumes, and surface areas. Focused tests compare cached and uncached refinement
coordinates and normals bit for bit on periodic, singular and transformed surfaces.

For desktop phase profiling, set `SOLIDCRAFT_PROFILE_OPEN=1` before launching the app.
Interactive Open reports geometry, placement, tessellation, thumbnail and total worker
times to stderr without logging the source path. Thumbnails were about 0.1 seconds of
the slow assembly's open and were not the main bottleneck. Further cold Boolean and
import costs remain; these changes do not make all documents open instantly.

### Prepared assembly geometry and selection

Prepared world geometry is now keyed by the document, evaluated model and exact placement
context instead of the session revision. Selecting a face or edge, changing visibility,
or moving the view no longer discards already prepared assembly bodies and meshes.
Geometry edits, pending occurrence moves, authoring context changes, undo/redo and timeline
rollback still invalidate the cache. Scratch sessions share prepared geometry while their
inputs match, and adopting a computed session retains its warm cache.

On the private 2,990-body Pi assembly, a matched Linux desktop control probe reduced an
edge-selection round trip from 9.24 s to 0.57 s. Initial opening stayed essentially unchanged
(33.54 s to 33.32 s); this fixes a separate selection stall, not cold geometry evaluation.
Three fresh Windows desktop runs with the corrected cache measured a median 15.87 s to
ready: 13.04 s geometry, 2.20 s placement, 0.53 s meshing and 0.07 s thumbnails. These used
the optimized `ci` profile and actual Vulkan rendering, with isolated settings and caches.
Linux and Windows timings are separate measurements and must not be compared as a speedup.

`solidcraft-cli bench-open design.solidcraft` reports JSON lines for file reading,
JSON decoding and migrations (including input-name repair), timeline evaluation, individual
features, and world placement/display meshing. `--parse-only` stops before geometry evaluation.
The parse timer includes document validation and repair, not just the JSON parser.

Windows x64, optimized `ci` profile, measured during other builds:

| Design | Parse/repair | Evaluate | Display meshes |
|---|---:|---:|---:|
| 19-feature enclosure, 34 KB | 1.2 ms | 14.9 s | 6 ms |
| 25-feature assembly, 15.2 MB, 2,990 bodies | 21 ms | 12.5 s | 2.1 s |
| Generated 1,000 independent boxes, 185 KB | 5 ms | 0.82 s | 0.15 s |
| Generated 5,000 boxes in five grids, 10 features | 0.45 ms | 17.6 s | 0.81 s |

The enclosures spend most of evaluation in joins and cuts, not parsing. Designs save feature
definitions; opening them rebuilds their geometry. Those times remain after the repair fix.

A separate generated timeline with missing input names reproduced minutes in the reader:

| Timeline (geometry rolled back) | Parse/repair before | After |
|---|---:|---:|
| 2,000 boxes, 6,000 unnamed inputs | 78.1 s | 10–20 ms |
| 5,000 boxes, 15,000 unnamed inputs | interrupted after 131 s, still repairing | 26–46 ms |

Previously, each missing input restarted the `d1`, `d2`, … search, scanning the document for
every candidate. This approaches cubic work as unnamed inputs accumulate. Loading now reserves
names once and assigns missing names with an advancing cursor. Existing names, names on later
features, parameter collisions and reuse of surplus names retain the previous behavior.
Regression tests compare the old and bulk repairs and load a 5,000-feature unnamed timeline.

# Viewport performance

Measured with the control channel (`ui.inspect` `frame_ms`: CPU time of one UI frame, which
includes picking under the cursor) on the sample plate patterned 20 × 20 (400 bodies, about
600k triangles), 1600 × 1000 window under Xvfb, release build. Hover: 60 cursor moves across
the model; orbit: a right-drag.

| | hover median | hover p90 | orbit median |
|---|---|---|---|
| before | 18.7 ms | 37.5 ms | 21.0 ms |
| after | 1.9 ms | 2.4 ms | 2.6 ms |
| after, Browser hidden | 0.4 ms | — | 0.4 ms |

What changed:
- **Picking culls by bounding box.** Per-body boxes are cached until the model changes. The
  face raycast skips bodies whose box the ray misses or enters behind the nearest hit so far.
  Edge and vertex tests skip bodies whose box, projected on screen, is not under the cursor.
- **Picks are reused** while the cursor, camera and model are unchanged, and **no picking runs
  while the view moves** (orbit, pan, zoom).
- **The model extent is cached** (it sets the view's depth range and was recomputed from every
  mesh several times per frame).

With 49 bodies the frame was already 2.8 ms (hover) before these changes. The remaining frame
time with the Browser shown is mostly the Browser listing every body row. GPU ID-buffer picking
was not needed at this size.

## Browser

Same model and method (400 bodies, Bodies folder open, 60 hover moves over the viewport):

| | median | p90 |
|---|---|---|
| Browser shown, before | 1.95 ms | 2.71 ms |
| Browser shown, after | 0.72 ms | 1.19 ms |
| Browser hidden | 0.30 ms | 0.72 ms |

What changed: rows scrolled out of view only reserve their height. They register no widgets
and paint nothing (no text layout, icons or eye/fold buttons), so the cost follows the rows on
screen rather than the rows in the tree. Scrolling, Find in Browser and drag and drop still see
every row's place.

# Recompute performance

`solidcraft-cli bench-edit <script> <param> "(<old>) * 1.02"` builds a design from its script
(timing the build), then times a parameter edit, setting the parameter back to its old
expression, undo and redo. Each cell is before → after the evaluation cache, with the number of
features recomputed in parentheses. Release build, on a machine shared with other build jobs (load average about 45; the
times vary by ±50 % run to run, so compare the recompute counts and the zeros). "Early" edits a parameter of the
first sketch or feature, "late" one of the last features.

| Design | Edit | Build | Edit | Set back | Undo | Redo |
|---|---|---|---|---|---|---|
| sample plate, 20 × 20 pattern | early (`width`) | 7587 ms (10) → 1764 ms (10) | 7969 ms (10) → 1691 ms (10) | 7557 ms (10) → 0 ms (0) | 6534 ms (0) → 0 ms (0) | 4570 ms (0) → 0 ms (0) |
| sample plate, 20 × 20 pattern | late (`d9`) | 7870 ms (10) → 1699 ms (10) | 3944 ms (1) → 812 ms (1) | 4039 ms (1) → 0 ms (0) | 1464 ms (0) → 0 ms (0) | 1398 ms (0) → 0 ms (0) |
| oracle 58 phone stand | early (`d1`) | 163 ms (6) → 140 ms (6) | 228 ms (5) → 140 ms (5) | 171 ms (5) → 0 ms (0) | 158 ms (0) → 0 ms (0) | 130 ms (0) → 0 ms (0) |
| oracle 58 phone stand | late (`d4`) | 144 ms (6) → 195 ms (6) | 118 ms (1) → 118 ms (1) | 145 ms (1) → 0 ms (0) | 149 ms (0) → 0 ms (0) | 187 ms (0) → 0 ms (0) |
| oracle 59 enclosure base | late (`d8`) | 11582 ms (11) → 11179 ms (11) | 5772 ms (1) → 5742 ms (1) | 5606 ms (1) → 0 ms (0) | 6023 ms (0) → 0 ms (0) | 6119 ms (0) → 0 ms (0) |
| oracle 60 pipe flange | early (`d1`) | 8137 ms (7) → 8348 ms (7) | 7569 ms (6) → 10143 ms (6) | 8354 ms (6) → 0 ms (0) | 8607 ms (0) → 0 ms (0) | 10034 ms (0) → 0 ms (0) |
| oracle 60 pipe flange | late (`d6`) | 7661 ms (7) → 11990 ms (7) | 67 ms (1) → 361 ms (1) | 69 ms (1) → 0 ms (0) | 84 ms (0) → 0 ms (0) | 66 ms (0) → 0 ms (0) |
| oracle 61 shaft keyway | early (`d1`) | 1723 ms (6) → 3016 ms (6) | 1785 ms (5) → 2745 ms (5) | 1760 ms (5) → 2323 ms (5) | 1667 ms (0) → 0 ms (0) | 1719 ms (0) → 0 ms (0) |
| oracle 61 shaft keyway | late (`d4`) | 1905 ms (6) → 2247 ms (6) | 1734 ms (1) → 2101 ms (1) | 1682 ms (1) → 0 ms (0) | 1719 ms (0) → 0 ms (0) | 1876 ms (0) → 0 ms (0) |
| oracle 62 heat sink | early (`d1`) | 382 ms (6) → 643 ms (6) | 362 ms (5) → 697 ms (5) | 233 ms (5) → 568 ms (5) | 245 ms (0) → 0 ms (0) | 360 ms (0) → 0 ms (0) |
| oracle 62 heat sink | late (`d5`) | 479 ms (6) → 688 ms (6) | 270 ms (1) → 657 ms (1) | 321 ms (1) → 0 ms (0) | 413 ms (0) → 0 ms (0) | 283 ms (0) → 0 ms (0) |
| oracle 63 hinge knuckle | early (`d1`) | 1305 ms (8) → 1671 ms (8) | 1329 ms (7) → 2397 ms (7) | 1971 ms (7) → 0 ms (0) | 1705 ms (0) → 0 ms (0) | 1321 ms (0) → 0 ms (0) |
| oracle 63 hinge knuckle | late (`d4`) | 1430 ms (8) → 2072 ms (8) | 1387 ms (1) → 2073 ms (1) | 1480 ms (1) → 0 ms (0) | 1192 ms (0) → 0 ms (0) | 1035 ms (0) → 0 ms (0) |

What changed: every feature evaluation is cached by a key made of the feature's own inputs
(definition and parameter values), the document context it may read (construction planes,
sheet metal and plastic rules, components) and a digest of the model it starts from, in which
each body is identified by the evaluation that produced it (bodies a feature passes through
unchanged keep their identity). The same feature on the same inputs is never evaluated twice:
setting a value back, undo, redo, a preview followed by the same command, and the repeated
evaluations while a script builds the design all reuse earlier results. The cache is shared by a
model and its copies (previews, scratch sessions), and keeps the last 4096 results within 64 MiB.

An edit still recomputes the edited feature and every later feature whose starting model
changed; in these parts every later feature works on the edited body, so early edits recompute
almost everything, as they must. `cached_evaluation_matches_from_scratch` checks that cached and
from-scratch evaluation give the same models (bodies, volumes and areas to 1e-6, topology,
errors) through edits, suppression, roll-back, undo and redo. Setting a sketch dimension back
can still recompute (oracle 61, 62): solving the sketch moves its points by rounding noise, so
its inputs are not exactly the old ones. Oracle 59's first parameter edit fails (a recipe
expression the bench's `* 1.02` can't scale) and is left out.


## Prism pairs, body measurements and the evaluation cache budget (2026-10-13)

A design whose features repeat one shape (a hole pattern, a plate cut by many tools, a part built
by one growing body) pays several per-feature costs that a rebuild was paying again and again.

**A body's size is measured once, not once per call.** Every kernel tolerance is a fraction of
the body's bounding-box diagonal, and `Body::size()` walked every vertex of the solid to work it
out. `Body::new` already reads those vertices to orient the body, so it keeps the diagonal it
measured there; a body built by hand, or a mesh body, still measures once.

**The same volume is not measured twice.** A boolean measures both operands' volumes to bound its
result, and the body it returns becomes the next feature's operand, so a timeline of *n* cuts
meshed every intermediate body twice. `Body::volume_at(tol)` keeps the last measurement for the
solid, shared by its clones (they share the solid), and the bounds ask at the same tolerance every
time.

**A prism pair is decided in 2D before either operand is meshed.** The general boolean builds a
refined mesh of both operands before it can decide anything, and that mesh is most of its cost: a
pattern, or a sketch with several profiles, rebuilds the whole target's mesh once per tool. A
plate and a tool through it are prisms along one direction, which `prism::prism_boolean` decides
exactly from their 2D sections - and that route ran *after* the meshes. A cut or an intersection
now asks it first and returns without building them. A pair the route cannot answer takes the old
path unchanged, meshes and all; a union still asks after the sampled shortcut. One answer is sent
back to that shortcut on purpose: when the 2D route returns one of its operands (a tool that
misses its target, or covers it whole), the sampled path returns that very body, so an untouched
body keeps its identity in the timeline (`ops::prism_fast_path_tests`).

**A placement reads its orientation from its matrix.** Placing a body (a pattern instance, an
aligned copy) used to mesh the whole copy to decide which way its faces point and to count the
faces its coarse mesh misses. A rigid placement leaves both as they were; a mirrored one (a
negative determinant) turns the solid inside out. Both are now read from the matrix, so only the
geometry is transformed.

**The evaluation cache is bounded by the bytes it keeps.** It held the last 4096 results, and a
result is as big as the model it holds, so a long editing session kept every superseded state:
nothing dropped the entries nothing referred to any more. It now also holds at most 64 MiB, oldest
first, and always keeps the newest result, so a single evaluation is never thrown away in the same
breath.

Release `solidcraft-cli`, `eval <design> --quiet`, wall time of the whole process and peak RSS
from `/usr/bin/time -v`, alternating the baseline and patched binaries, median of 12 runs each
(8 for the 400-hole plate and the vent plate), no other build or test running (Fedora Linux, AMD
Ryzen 7 5700X, 8 cores / 16 threads, 32 GB):

| Design | Baseline | Patched | Change | Baseline peak RSS | Patched peak RSS |
|---|---:|---:|---:|---:|---:|
| 400 × 400 × 6 mm box, one sketch with 100 circles cut through it | 1.24 s | 0.83 s | -33% | 151 MB | 119 MB |
| The same plate with 400 circles in the sketch | 5.67 s | 4.04 s | -29% | 556 MB | 481 MB |
| Vent plate, a 10 × 8 pattern of one cut | 0.10 s | 0.08 s | -20% | 22 MB | 22 MB |
| 14 × 14 pattern of a filleted box (196 copies) | 6.70 s | 6.52 s | -2.6% | 67 MB | 67 MB |
| 60 sketches, 60 extrudes into one growing body | 8.80 s | 8.88 s | +0.9% | 225 MB | 225 MB |
| `examples/acceptance/sensor_enclosure.json` | 3.81 s | 3.30 s | -13% | 198 MB | 196 MB |

The hole plates are what the prism route is for: one boolean whose tool is a prism with 100 or 400
sections, in a sketch with that many circles, so the tool's mesh alone is what the route saves.
A variant of the patched tree with the route asked in the old order - after the operands' meshes -
timed the 100-hole plate at 1.27 s and 157 MB (range 1.22-1.30 s), the baseline's own numbers
(1.25 s and 156 MB over the same runs, range 1.21-1.33 s). The two plate rows, and the peak they
lose, are the prism route.

The last three designs are the other side: their booleans are not that case. The 196-copy design
and the 60-sketch design are within 2.6% and 0.9% with overlapping ranges. The sensor enclosure's
medians differ by 13%, but its runs land anywhere between 3.0 s and 5.0 s and the two ranges
overlap (3.01-4.97 s against 3.00-4.89 s), so its row is not a result either way and is left as
measured. The body measurements, the placement and the cache budget are not what these rows
measure: they stop work the corpus repeats too rarely to see (a hidden vertex walk, an operand
meshed twice for one number, an orientation mesh per placement, a cache with no byte bound).

No model changed. The evaluation dump of 21 designs (10 to 400 circles in a plate, 16 × 16 to
31 × 31 box grids with and without fillets, 2500-body grids, assemblies of 40 to 500 bodies, 30 and
60 sketches, a 196-instance pattern, the bundled sample and the sensor enclosure) is the same model
before and after - bodies, volumes, areas, centres, bounding boxes, topology counts and warnings -
apart from the per-feature milliseconds. The
kernel's test suite passes, including four new tests for the prism route and its identity rule.

Reproduce after compiling a release build, without other builds running: time
`target/release/solidcraft-cli eval <design> --quiet` with `/usr/bin/time -v`, alternating the
baseline and patched binaries, and compare the middle of the runs. The corpus (20 generated designs:
hole plates, box grids, assemblies, many-sketch parts) and the measuring scripts are in
https://github.com/sahenjp/solidcraft-perf under `perf/`.

## Desktop loading and recent thumbnails (2026-10-10)

The Arduino Uno enclosure (19 features, 34 KB) and Raspberry Pi 5 case (24 features,
61 KB) each decode in about 1 ms. A release build spends roughly 12.1 s and 8.2 s,
respectively, evaluating their geometry on this Windows machine. Changing the document
encoding does not eliminate these boolean operations.

Interactive Open, recent-file clicks, drag/drop and STEP/mesh insertion now execute on a
worker, including preparation of the world-space bodies and display meshes. The existing
design stays available until the result is ready; a failed or cancelled job cannot replace
it. Insertion preserves the file, saved state, earlier undo history and view settings.
Timeline copies share their lazy mesh cell, and adopting a prepared session keeps its warm
world-space cache instead of transforming and tessellating its components again on the UI.
A STEP body's missing-face check reuses its orientation tessellation when the tolerances
match and its orientation was not reversed.

Live desktop tests (`ci` profile, with other builds/tests running) opened the Arduino and
Pi case in 15.2 s and 10.5 s. During those jobs, median `ui.inspect` round trips were 20.5 ms
and 8.3 ms; the slowest were 33.7 ms and 28.6 ms. This fixes the window freeze, not the
remaining cost of cold geometry evaluation.

Recent thumbnails are 240 × 150 PNG files in `SolidCraft/thumbnails` under the platform
cache directory (`LOCALAPPDATA` on Windows). Their keys include the source path, file size
and modification time. Open and Save render from the model already in memory. Cached
recents decode the PNG without reading or rebuilding a document, and run before cold
recents and sample thumbnails. Missing or invalid pictures fall back to a background rebuild.
No thumbnail or base64 image is added to the design JSON. A regression test verifies the
source bytes remain unchanged, a cached picture works even without a parseable source
file, and changing the source invalidates the cache.

`solidcraft-cli bench-open` also accepts STEP/IGES. For exchange files its `parse` phase
includes constructing and checking the imported B-rep, not merely decoding STEP syntax.

The live test loaded both cached recent cards in 21 ms and inserted the 2 MB lightweight
Pi STEP (27 bodies) into the case in 279 ms; undo restored the original two bodies. The
full 77 MB board STEP (2689 solid definitions, 45,689 faces) still had not completed after
300 s. Its window continued handling inspection and Cancel, and cancelling preserved the
case. Complex STEP construction remains an outstanding kernel performance problem.

# Sketch solver and drags (2026-10-11)

Unknowns the constraints pin down one after another from the constants (each one's own rows
of full, well-conditioned rank, reading nothing but constants and unknowns pinned before it)
are eliminated exactly: values the constructive seed placed and whose constraints hold there
stay constants while Levenberg–Marquardt solves the rest, and the rank analysis (degrees of
freedom, determined geometry) runs on the remainder only. A component tied to no constant takes
one point as a constant first (its translation). All the held solves of one live drag frame
share a 1M decomposition work budget (about 16 ms; a solve's own, 20M, is about 0.3 s), and a
drag's steps are a quarter of the smallest held size instead of an eighth, except once a seed
ran out of budget: the rest of that drag goes in eighths, as before, and continuation keeps
the branch the seed no longer places.

`cargo run --release -p solidcraft-sketch --example solve_bench`, macOS arm64, load average
45–75 (best of 5):

| Grid (origin corner fixed, rows horizontal, columns vertical, every edge a distance) | Start | Before | After |
|---|---|---:|---:|
| 10 × 10 | at solution | 0.36 ms | 0.21 ms |
| 10 × 10 | ±1 mm | 0.34 ms | 0.21 ms |
| 15 × 15 | at solution | 1.41 ms | 0.41 ms |
| 15 × 15 | ±1 mm | 1.39 ms | 0.40 ms |
| 20 × 20 | at solution | 4.06 ms | 0.72 ms |
| 20 × 20 | ±1 mm | 4.44 ms | 0.73 ms |

The seed constructs every grid point, so LM gets no variable (a test counts them). Before, the
time was the dense rank analysis of the whole grid. A 100-rectangle strip with nothing fixed,
solved whole from a moved corner, still costs 20–27 ms: nothing is constructed there and LM
iterates on every point with dense normal equations (a sparse solve would be the next step).

Drag frames, `sketch.drag` + `world_state()` per frame (60 frames of 0.5 mm, best of 5; the
"before" column is one run of the same bench on the previous code):

| Sketch | Points | Scripted ms/frame before → after | Live ms/frame before → after | `drag_end` ms before → after |
|---|---:|---:|---:|---:|
| 100 rectangles | 401 | 2.77 → 2.37 | 1.13 → 1.02 | 1.44 → 1.37 |
| 100 rectangles, rolled back under an extrude | 401 | 2.49 → 2.33 | 1.13 → 0.97 | 1.42 → 1.28 |
| 100-rectangle connected strip | 401 | 100.9 → 2.32 | 1.32 → 1.20 | 53.6 → 1.41 |
| 100-rectangle strip, rolled back under an extrude | 401 | 203.8 → 2.23 | 1.31 → 1.18 | 54.0 → 1.42 |
| rigid truss of 40 triangles, pinned at one end, dragged at the other | 42 | — → 6.8 | — → 6.3 | — → 0.59 |

The strip floats (nothing fixed), so its full solves (scripted frames, `drag_end`) ran the
dense rank analysis on 800 variables; with one corner taken as a constant all of it is
eliminated. A live frame's held solve was already cheap: the seed places the strip and LM
returns at once. Of a live strip frame, the seed reads the constraints in about 0.15 ms, so a
per-gesture cache of that reading is not worth its invalidation (its signs depend on the
reference positions, which change every step).

## Portable thumbnail save cost (#303)

Portable thumbnail generation requires explicit `cache: true`. Ordinary saves and
browser downloads default to `cache: false`, so they incur none of the thumbnail
work or added bytes measured below.

Measured 2026-10-13 on the same Windows machine and `ci` profile as above, but
with **SolidCraft 0.7.0** on both sides: `d3d215b` before the thumbnail change and
thumbnail revision `e180cdc` after it. These measurements precede the integration
of upstream `9f88537`; later geometry changes are not included in these timings.
A preserved executable of the baseline
and the new executable ran the existing `benchmark_native_cache_save` on the same
STEP and sensor documents. Three paired batches alternated before/after order;
each batch performed nine alternating atomic saves per encoding. The values below
are medians of the three batch medians, not a percentile of all individual saves.
No compilation, test suite or desktop model was running during these measurements.

| 512-cube STEP encoding | Before PNG (ms) | After PNG (ms) | Container bytes before → after |
|---|---:|---:|---:|
| Plain JSON | 10.334 | 9.676 | 1,998,826 → 1,998,826 |
| Container, `cache: false` | 8.850 | 9.013 | 1,950,084 → 1,950,084 |
| Native cache, fast | 45.323 | 53.473 | 4,694,035 → 4,720,385 |
| Native cache, compact | 42.739 | 48.643 | 837,067 → 863,417 |

The PNG itself is 25,916 bytes; its key and ZIP records bring the increase to
26,350 bytes. Fast-cache batch medians ranged from 41.585–47.779 ms before and
53.212–56.556 ms after. Compact ranged from 36.672–43.040 ms before and
47.688–51.724 ms after. The approximately 8 ms fast-save increase is a measured
cost on this example, not a general latency bound. JSON bytes and document/source
blob bytes were compared across both implementations and remained identical.

The sensor enclosure deliberately receives **no portable PNG**: its projected
triangle boxes total 4,293,457 supersampled pixels, above the four-million work
budget. The native fast container remains 379,099 bytes. Its save batch medians
are 14.714 ms before (12.187–14.956) and 13.728 ms after (13.439–14.648); these
vary within the observed run ranges and do not establish a speedup. Home retains
its existing thumbnail-generation fallback for this case. A small triangle count
alone does not guarantee eligibility, because large overlapping faces cost more.

Actual desktop check: copy the STEP container to a new public temporary path,
start SolidCraft 0.7.0 with that path in Recent and an empty local thumbnail cache,
and show Home without opening the design. The portable picture was ready in one
observed 47.6 ms check; no local PNG was created and the active document remained
empty. The screenshot shows the transferred file's picture while built-in sample
pictures are still loading. This is one functional visual check, not a first-frame
benchmark. The separate counting-reader regression proves source-blob deferral.

## Extrude drag preview delivery

On macOS with Metal, editing a 15 mm radius cylinder's 10 mm extrusion and dragging its
arrow upward by 150 screen points (120 input steps, Ctrl held) exposed preview starvation.
A completed worker result was discarded whenever the pointer had already changed the
requested distance. The arrow also drew the previous frame's distance.

Main's tool-only preview already delivers intermediate Join/Cut/Intersect geometry. New Body
extrusions (including feature edits) and Offset Face use full-result previews and still discard
completed distances during a moving drag. Those paths now accept successful intermediate
distances from the same setup. Selections, operation, extent mode, feature identity, revision
and colours remain part of the setup identity. Errors still apply only to the exact requested
inputs, and the final distance is computed normally. Tool previews and background Apply retain
their existing paths. The arrow and value-box anchor use the current frame's pointer value.

Release-build control-channel checks against upstream `4dc63810` with the arrow fix alone,
then with the distance fallback, used the same 150-point, 120-step drags. The cylinder used
the setup above; planar-face Press Pull (Join) and Offset Face used a 30 × 30 × 10 mm box.
The samples captured 25–27 distinct requested distances per drag:

| Preview updates before reaching the final distance | Main + arrow fix | With distance fallback |
|---|---:|---:|
| Edited New Body extrusion | 0 | 24 |
| Planar-face Press Pull (Join) | 22 | 24 |
| Offset Face | 0 | 23 |

Join already streams on main; the small count difference reflects sampling, not a change to
that path. Applying the fixed New Body extrusion produced the requested 19.585 mm height;
applying the 11.672 mm face offset produced a 21.672 mm tall box.

These are sampled preview deliveries (`ui.inspect.preview.ms` changes), not every rendered
frame or a geometry-kernel speed benchmark. With main's preview code unchanged, the New Body,
edited New Body and Offset Face drag regressions fail while Join passes. With the fallback,
all six distance-preview regressions pass, covering intermediate geometry, exact final
convergence, unchanged document/undo, obsolete errors and setup changes. A separate regression
checks the arrow's position on each held input frame.
