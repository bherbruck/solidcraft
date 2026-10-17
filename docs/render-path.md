# Progressive product images

This integration is a draft. Final combined CPU, wasm and GPU verification and
full-resolution showcase acceptance remain incomplete. Test descriptions below
state intended coverage, not a claim that every backend has passed.

Native applications can register `solidcraft_gpu::path::host()` alongside the
existing CPU image host. `render.backends` then exposes `builtin/gpu-path` with
Preview, Image, PNG, Pbr, Progressive and Refraction. The browser keeps its CPU image host.
Device discovery follows `SOLIDCRAFT_GPU` and `WGPU_BACKEND`; a device which cannot
create the compute pipeline does not register an available path executor.
The native UI opens its independent job device on the viewport's selected adapter,
so software fallback and safe mode do not rediscover a failing hardware driver.
An incompatible explicit GPU policy leaves progressive rendering unavailable there.

In the Render workspace, enable **Progressive rendering** and select samples per
pixel before checking or starting an image. These controls are transient image-job
intent. Existing saved setup/camera/material formats are unchanged.
The interactive canvas keeps its existing product preview; this first image-job
integration reports sample progress and exports the completed PNG, without displaying
partial accumulation buffers on the canvas.

```json
{"command":"render.start","params":{"settings":{"width":800,"height":600,"exposure_ev":0},"progressive":true,"samples":64,"max_depth":8,"seed":0}}
```

`render.preflight` accepts the same input. It resolves the Progressive capability
to the installed GPU host and performs admission without reserving memory. Start
checks again and returns the existing session/serial/generation identity. Progress
reports complete samples per pixel; a partially computed sample is not counted.
Cancel, status, retained-image ownership and atomic PNG export use the same host
protocol as CPU jobs. Defaults are 64 samples, depth 8 and seed 0; the bounds are
1–4096 samples and 1–16 bounces. Sampling controls require `progressive: true`.

Runtime request-only [glass overrides](render-transmission.md) and an optional
[thin-lens camera](render-thin-lens.md) and [clear coat](render-clear-coat.md) are available through those same commands.
Neither adds saved document fields. Positive aperture requires perspective.

```sh
solidcraft-cli render examples/render-path-studio.json --out enclosure.png --progressive --samples 64 --max-depth 8 --seed 0 --width 800 --height 600
```

The CLI explicitly fails if a progressive request has no suitable device. Ordinary
CPU requests retain their prior behavior. GPU PNG export averages linear radiance,
applies saved exposure and the selected tone map (Reinhard by default), then
encodes sRGB once. Runtime `aces_fitted` is described in
[studio lighting](render-studio-lighting.md). A
transparent background uses straight alpha; background color is independent of
illumination.

The original integrator uses software BVH traversal, opaque GGX/Smith/Schlick and
the symmetric diffuse attenuation defined by `render::surface`. Each hit evaluates
the three existing directional studio lights with shadow visibility. Continuation
samples a mixture of cosine-weighted diffuse and GGX normals with the combined
PDF; Russian roulette begins after four hits. There are indirect bounces, but no
environment illumination in the default legacy lighting profile. All legacy opaque colors use
linear-sRGB base color, metallic 0, roughness 0.5, IOR 1.5. Explicit face appearances
take precedence over body appearances.

Explicit optical appearances can now select smooth dielectric reflection,
refraction, Beer–Lambert absorption and ideal thin sheets; see
[the transmission contract](render-transmission.md). Preview opacity remains a
separate scalar-alpha approximation. Opacity below one without an explicit
transmission record is rejected by the path executor. Runtime
[area lights](render-studio-lighting.md), [sun/HDR environment lighting](render-environment.md)
and [opaque planar color/normal maps](render-runtime-textures.md) are available
through explicit job inputs. They do not add saved document fields. Denoising,
texture LOD/mipmap selection, EXR and Final mode remain unimplemented by this
executor; a path-traced preview does not declare those capabilities.

Each job builds a balanced median-split BVH over its immutable captured meshes;
there is no cross-document cache to confuse scene generations. Positions are
rebased around the camera target before f32 upload. The upload packs indexed 32-byte position/normal vertices, 16-byte triangle records
(including material IDs), 64-byte resolved materials, four-byte planar patch IDs
and 32-byte BVH nodes. Geometry arrays share one buffer. The progressive executor
requires five storage buffers per stage for geometry, BVH, accumulation,
environment and textures; raster device requirements remain unchanged.
Geometry must be within 1e7 mm of the camera target. Opaque and thin-sheet ray
offsets use max(1e-5 mm, extent × 1e-7); closed glass instead retains an anchor
and local displacement with strictly positive intersections, as described in
[closed-glass boundaries](render-glass-boundaries.md). These are bounded preview
precision policies, not a robust CAD intersection oracle.

Each GPU dispatch computes at most 262,144 pixel paths at one sample index. The
worker waits for that dispatch before checking cancellation and issuing another.
A driver wait has a 30-second timeout; cancellation cannot preempt a stalled GPU
dispatch. Encoding yields every four rows. Invalid indices/normals, nonfinite
geometry, excessive dimensions and sampling controls are errors.

The GPU host admits at most 1,048,576 input triangles, 4096 pixels per side and
4,194,304 pixels in all. The actual storage-buffer limit may lower these caps.
The source geometry cap is 128 MiB and the per-host pool is 512 MiB, including
retained PNG leases. Conservative estimates account for CPU BVH construction,
serialized and device geometry, accumulation, readback, RGBA and PNG buffers.
Allocator/driver bookkeeping is not an OS quota. CPU and GPU workers have separate
pools and each retains the established two-entry queue bound; a document still
has at most two live jobs across both executors.

Validation lives in `gpu::path::tests` and engine render job tests. Set
`SOLIDCRAFT_REQUIRE_GPU=1` to make absence of an adapter fail device tests rather
than skip them. BRDF parity is checked against the f64 oracle with 0.3% relative
plus 2e-5 absolute tolerance. The near-white furnace allows 0.07 absolute error:
the existing single-scattering Schlick model is not a perfectly conserving pure
Lambertian surface, even at IOR 1. Same-adapter determinism compares the actual
linear buffers bit-for-bit; cross-adapter bitwise identity is not promised.

The ignored `path_performance` test builds a 1,048,576-triangle procedural sphere
and measures 1920×1080 dispatches, BVH construction and upload. Select sample count
with `SOLIDCRAFT_PATH_BENCH_SAMPLES` (default 1). Run it explicitly with a clear GPU
timing window; it is excluded from normal CI.
