# Product-image jobs (R4a)

Native progressive opaque PNG rendering is described in [Progressive product images](render-path.md). It registers alongside the CPU executor.

R4a adds bounded CPU preview images to the Render workspace. The installed `builtin` /
`cpu-product` executor supports Preview, Image and PNG. It does not promise refraction,
HDRI, exposure, final mode or EXR; those requests fail explicitly. The subsequent
[opaque PBR consumer](render-opaque-pbr.md) adds opt-in direct-lit surfaces through
the same host. Legacy scalar looks and the Design snapshot renderer retain their
existing shading.

## One command path

`render.preflight` and `render.start` take `setup` (a saved name) OR `settings` (a
RenderSetup object; omitted fields use its documented defaults), plus optional
`transparent_background` and `replace` booleans, both false by default. Without either
setup/settings, the current requested camera is used, or the camera is fitted to the
product. Saving a setup is independent of starting a job. Unknown fields are errors.

Both commands reject stale geometry without settling a live modelling gesture. Finish
the gesture or use the explicit `render.scene` boundary first. Capture uses R3's
authoritative visible meshes, placements and appearances, without selection, edges,
axes, grids, sections or analysis overlays. A cold capture can prepare world geometry
and meshes synchronously; it is not a promise of bounded CAD evaluation latency.
Preflight validates capabilities and estimates admission, but does not reserve a queue
slot or the host memory pool. Start checks those resources again.

Start returns a typed `job: {session, serial, generation}`. Keep all three fields when
calling `render.status`, `render.cancel` or `render.export`. IDs are never accepted by
another document session, scratch copy, or a newly opened/recovered document. Status
does no rendering and never waits for completion. It reports queued, preparing,
running, encoding, cancelling, cancelled, completed or failed, with phase-local
`done/total`, structured job errors and completed frame metadata. Progress restarts
when a phase changes; it is not a time estimate.

The captured `scene_generation` is the session revision at start. Later edits, undo,
camera changes and tab switches do not mutate the snapshot. An image continues to
describe that captured scene; the UI labels a revision mismatch conservatively (view
cache invalidation can also advance a revision). Runtime state is outside the document,
undo, checkpoints and the design format. Closing/replacing a session cancels its work;
the worker cannot publish a cancelled frame. Direct host-side document replacement
preserves the installed image service while allocating fresh job ownership; old IDs
and completed images never cross into the new document. `replace: true` cancels retained active
and pending jobs before submitting the new request. A cancelled generation cannot
publish late. A successful older generation never replaces a newer successful result.

Each document retains at most two live jobs, four recent terminal status records and
one last successful image. Cancellation/failure preserves that image. A new successful
image releases the previous image; old terminal metadata can still report completion,
but it does not promise that its bytes remain exportable. Export requires an explicit
retained completed job ID; it never silently exports a different/newer job.

## Native and browser execution

Each application installs one CPU host, shared by its documents. Native execution uses
one worker and at most two queued inputs across documents; it runs without UI polling.
The browser uses the same incremental task, pumped by UI frames, without threads.
Preparation, stable transparency sorting, clearing, pixel tests and downsampling check
cancellation between small units. A step admits at most 16384 units; PNG encoding
additionally yields after four rows. The UI pumps 1024 units per frame. These are work
bounds, not hard real-time guarantees: allocation, codec compression and OS scheduling
still have platform-dependent latency.

Admission caps output at 4096 per dimension and 4 million pixels; pixel scratch is
estimated at 48 bytes per output pixel (2x supersampling). Other limits: 262144
triangles/face overrides/edge polylines, 128 MiB retained source geometry, 384 MiB total
estimated memory per job, 512 MiB shared host admission including retained PNGs, and
128 million projected pixel tests. The projected-work limit is checked during bounded
preparation and may fail an admitted job. Encoded PNG bytes have an additional
`8 * output_pixels + 65536` cap. Allocator and codec internals are estimates, not an
operating-system memory quota. Limits fail with errors rather than allocating unbounded
buffers. Completed frames keep their memory reservation while bytes are retained.

The CPU path shades in linear light, blends sorted triangle alpha approximately, then
encodes 8-bit sRGB with straight alpha and an sRGB PNG chunk. Transparent shared edges
use a consistent fill rule. Intersecting transparent surfaces and triangle sorting
remain raster approximations. This is not a glass/refraction renderer. Native GPU
preview and CPU export can differ in shading and transparency.

PNG uses the already-transitive Rust `png` crate (MIT OR Apache-2.0), now an explicit
render dependency. No external render engine or third-party art is introduced.

## Export and automation

`render.export` takes `job`, `path` ending in `.png`, and `overwrite` (default false).
The destination is checked against the caller's existing file-access scope before
reading the retained frame or calling either native writer. Disallowed folders and
traversal are refused in both overwrite modes. Native no-clobber export writes/syncs a temporary beside the target and publishes it
with an atomic hard link; an existing destination wins even in a race. Filesystems
without that operation return an error. Explicit overwrite uses the existing atomic
writer without reading or backing up the previous target. Failed writes preserve the completed
image and CAD state. Browser export uses the existing virtual file/download service;
no filesystem paths or base64 payloads are persisted in the design.

The Render panel provides Check Render Request, Start Render, phase progress,
Cancel Render and Export PNG. Replace existing file is an explicit opt-in. A failed or
cancelled new render leaves the previous successful export available.

Native PNG export in the combined integration uses the document-bound asynchronous
picker from #1114. The requested job ID and overwrite choice are captured before the
dialog opens: cancellation writes nothing, closing the originating document drops
the answer, and an evicted image fails instead of substituting a newer result. A tab
switch does not redirect the export to another document.

Controls and phase messages use stable offline catalog IDs in English, Japanese, Chinese,
German and Korean; command IDs and scenario handles remain unchanged.

```sh
solidcraft-cli render examples/render-studio.json --out product.png --width 800 --height 600
solidcraft-cli render design.solidcraft --setup Studio --out product.png --transparent
```

The CLI follows preflight/start/native wait/export, with a default 120-second wait
limit (`--timeout 1..3600`). Timeout requests cancellation. `--overwrite` is required
to replace an existing target. A persistent headless MCP session installs the same
host: use the ordinary `execute` tool with the five commands and returned job ID.
Closing the CLI/MCP process ends its session; a one-shot `exec render.start` is not a
durable background service. In a browser, leave the app running so frames can pump work.

Validation evidence and responsiveness measurements are recorded in the R4a issue/PR;
passing native tests or a wasm compile does not establish browser visual performance.
