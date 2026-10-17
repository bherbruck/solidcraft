# CPU ray visibility and hard shadows (RT2)

## Current combined integration boundary (2026-10-17)

The sections below describe the original RT2 direct-image stage. In the combined
source, [HDR/reflection integration](render-hdr-reflections-integration.md) adds
selected ideal/rough-specular reflections and certified planar anisotropy.
[Environment sampling](render-environment-sampling.md) describes the earlier
standalone direct estimator; the combined page specifies the connected reflection
path and its disjoint HDR estimators. [HDR input](render-hdr-input.md) remains the
bounded decoder route. Only explicit sRGB/D65 primaries are admitted for lighting.
HDR plus rough reflections requires fully metallic selected bodies and all
effective face responses. HDR plus anisotropy requires the supported rough
reflection combination; direct anisotropic HDR and anisotropic delta mirrors
refuse. Those additions do not imply MIS, GI or unrestricted material transport.

The opt-in `solidcraft-engine/raytrace-cpu` feature forwards the render feature.
The native host selects `cpu-raytrace` only for explicit
`integrator: "raytrace"`; raster remains the default, and cooperative/browser
hosts refuse ray tracing. Use the same preflight/start/status/cancel/export
commands and job identity described in [product-image jobs](render-jobs.md).
Preflight does not reserve a queue slot or memory; start repeats admission.

The CLI `render` wrapper accepts settings, output and timeout options, but no
`--integrator` or `--raytrace-options` flags. It submits settings and transparency
through the default raster route, waits for the job and exports within that
process. Jobs do not survive the host process exiting. Ray tracing commands can
instead use a persistent native MCP session with the compiled feature and an
installed image host, or the optional desktop image controls described below.

Primary rays default to fixed pixel centres. Explicit `primary_samples` values
of 4 or 16 use bounded pixel-box quadrature in [primary AA](render-primary-aa.md).
Reflection samples and viewport MSAA are separate controls. The retained raster
scratch estimate does not mean ray tracing performs raster supersampling.
The [light union](render-light-union.md) connects bounded rectangle and point/spot
sources to this same `ImageTask`; continuous LED strip recipes lower to a rectangle
before capture. These source integrations require their own revision-specific
qualification. Texture sampling and glass/refraction/transmission remain outside
this consumer. Output remains opaque Preview/Image/PNG, without EXR or GPU ray
tracing.

Existing shared-host limits remain: 262,144 triangles, 128 MiB retained source
geometry, 384 MiB job admission and a 512 MiB shared pool. These are not total
CAD-process RSS limits. HDR buffers, reflection selectors, light options and
actual retained capacities join the same ownership/accounting; the common
cumulative work cap remains 1,000,000,000.

### Desktop CPU/AA controls

The optional desktop feature `solidcraft/raytrace-cpu` forwards the engine feature
and enables the Render image-method choice when the installed image host supports
ray tracing. Raster remains the default. CPU **Edge samples** offers exactly
1× (the original fixed centre), 4× or 16×. Preset lighting, cast shadows and
exposure are temporary per-document editor choices. More samples cost more work
and can exhaust the unchanged limits.

Check and Start use the same request builder, native image host and captured-job
commands. CPU images require an opaque background. Missing, stopped, cooperative
and compiled-out CPU hosts refuse before image scene capture; the selector does
not install a fallback or another host. A stale model must be explicitly refreshed
or its modelling gesture finished before Check or Start.

The current Check badge is advisory. It invalidates on document identity,
revision, visibility, camera/options or host changes. Start rebuilds the current
request and repeats admission. The global status message is not a current Check
identity. Export uses the retained completed job selected when its file picker
opens; changing options does not rerender that image. A failed or cancelled new
job preserves the previous successful frame.

Runtime method, AA, preset, shadows and exposure are not persisted by Save Setup
or added to document undo. Switching to Raster discards CPU options and resets
the next CPU choice to 1×; a new or reopened document starts with Raster.
Existing Record and navigation routes are unchanged. This panel does not expose
HDR, reflection selection, explicit directional/area/punctual lights, LED recipes
or anisotropy authoring. It rejects non-panel raytrace options instead of silently
forwarding them. Engine/MCP lighting support is a separate interface.

### Qualification and historical evidence

Qualification belongs to a specific source revision and build configuration.
Report the exact source, feature-on/off checks, wasm and full CI results, native
executable identity and visible evidence in the accompanying PR. Earlier scoped
CPU/AA results belong to their recorded source snapshot; they do not certify a
later merge with main or changes to lighting and host contracts.

For this UI consumer, evidence must exercise the actual method and AA controls,
Check, a new Start/completion for both 4× and 16×, and native PNG export. Check
that those requests reach the same host, the exported bytes belong to the selected
completed frame, and repeated export does not rerender it. Preserve document
geometry, physical properties and history during rendering; save and reopen the
design to verify that the transient CPU/AA choices are absent. Include refusal,
preflight invalidation and cancellation coverage, with the CPU feature both on
and off. These are acceptance criteria, not a claim that this combined revision
has passed them. Broader HDR/reflection/light changes require their own evidence.

The combined path includes the exact-coordinate-plane depth correction. On an
earlier isolated source, the Rust witness failed before the correction and passed
afterwards; that source passed 222 feature-on and 147 feature-off render tests
plus scoped Clippy/wasm. Those historical results do not qualify later main
integration, primary-shading reuse or reflection-sequence changes. General oblique
and near-edge robustness remain outside the coordinate-plane correction's scope.

The original combined source's full CI ended incomplete at its fixed deadline.
Known modelling golden failures remain recorded separately; do not attribute
their cause to rendering or carry them over as a verdict on another revision
without paired baseline evidence. An earlier optional 960x540 request exhausted
the billion-unit cap without producing a PNG; smaller drafts do not qualify that
request. No cap, assertion, tolerance, ignore or golden-baseline change is implied.

The stage-specific descriptions below record the original direct-image scope,
not a current pass/fail report. This opt-in slice of #1368 follows the phased
render design in #620 and connects SolidCraft's original RT1 triangle/BVH core
to immutable product captures and the existing image-job host. The
`raytrace-cpu` Cargo feature stays off by default.

## Original RT2 runtime contract

`render.preflight` and `render.start` accept `integrator: "raster"` (the default) or
`"raytrace"`. Optional `raytrace` settings contain `lights`, `exposure_ev` and
`shadows`, plus an optional `preset` (`neutral`, `warm_cool`, `key_rim`).
A preset and an explicit lights list are mutually exclusive, including an empty
list. Presets use original hard directional-light compositions, not calibrated
color-temperature claims. These values belong to the request, not the saved document or render
setup. A build without the feature refuses raytracing before geometry capture.
The initial image executor is native-only; no browser performance is claimed.

Each light has a world-space `direction: [x,y,z]` pointing **towards the light**,
linear-sRGB `color: [r,g,b]` and relative `intensity`. Directions are normalized;
all numbers must be finite. At most eight lights and eight retained slots are
accepted, colors are in [0,1], intensity in [0,64], exposure in [-16,16] EV.
Omitting lights uses the existing three-light studio. An empty list is darkness.
These are relative radiances, not measured lux or watts. Background color remains
independent of illumination and exposure.

The scene must be opaque, including face overrides; transparent backgrounds are
refused. Captured world positions are used once, preserving occurrence placement.
Scalar colors become linear-sRGB opaque dielectric surfaces (roughness 0.7, IOR
1.5); explicit opaque surfaces retain their metal fraction, roughness and IOR.
This scalar fallback differs from the legacy raster shading and is not a measured
material fit. Density and engineering properties do not change.

The camera uses the existing pixel rays and the same scene-radius near/far policy
as product rasterization. Primary rays start on the near plane; shadow rays have
independent finite intervals. There is one center sample per output pixel, so edge
quality can differ from the supersampled raster image. Shading uses interpolated
normals with a geometric-normal fallback and hemisphere checks.

Per-light BVH visibility supplies hard shadows to the existing opaque BRDF. Linear
contributions are summed, exposure is applied once, then Reinhard and the piecewise
sRGB transfer are applied once. Shadows can be disabled without changing the BRDF.
No SSAO is multiplied into this output. No indirect lighting, soft shadows,
reflection transport, texture sampling, HDRI lighting, refraction, absorption,
emission transport or denoising is implemented here. IOR controls opaque Fresnel;
it does not make glass. Unsupported requested capabilities must fail explicitly.

## Work and memory

The same native queue, worker, cancellation token and 512 MiB host reservation are
used. There is no additional worker pool. Admission retains the conservative raster
geometry/pixel/codec estimate and adds BVH input/nodes/order/build stack, per-triangle
surface/normals/vertices and 8 KiB for task/light/traversal state. Total admission
must remain within 384 MiB including the checked sum of work and retained request
allocations. Original directional-light capacities keep a separate lease from the
same pool for the job-handle lifetime, even after PNG eviction. Actual buffer capacities may not exceed the planned
capacities; the builder's actual reservation is checked before continuing.

One cumulative maximum of one billion work units covers preparation, BVH build,
primary and all shadow traversal. Builder operations are debited into that same
budget. Exhaustion and cancellation are typed failures, never truncated visibility
or a partial successful image. Each traversal step visits at most one node or
primitive; each preparation step examines at most one face override. A task call
performs at most four such units, or four PNG rows. These are work bounds, not a
hard real-time or operating-system memory guarantee.

Shadow origins are reconstructed from triangle barycentrics. Component-wise
floating-point rounding bounds projected onto the geometric normal and one outward
representable step replace a fixed world-space epsilon. Secondary intervals require
a strictly positive representable distance to exclude signed zero after subnormal
interpolation. This avoids displacing a
Z-plane merely because X is large. Floating-point geometry still has finite
resolution; thin blockers, self-intersection and translated scenes require tests.

PNG encoding reuses the bounded cancellable row encoder. Captured geometry,
acceleration data, pixels and light buffers are released before successful output
is returned; the host drops task state before shrinking to the retained PNG lease.
Cancelled or failed jobs preserve the previous successful frame.

## Provenance and acceptance

RT1 was copied, with its owner's authorization, from the existing isolated
`rt1-raytrace-core` contribution. Its original source remains untouched. The new
resumable cursor and image consumer are original Rust additions; the source receipt
records the exact adopted hashes. No external rendering engine, shader source,
commercial CAD asset or new dependency is introduced.

Regressions cover typed cumulative-work exhaustion, resumable traversal parity,
near/far camera policy in both projections/up axes, translated thin occluders,
self-shadow avoidance, runtime refusal and captured-resource release. Lighting tests
use analytic Lambert values and independently masked colored lights. Actual test
counts, compiler results and original-scene PNG review must be reported only after
execution. Qualification of the integrated revision requires those gates and the
PR evidence described above.

Beyond the original direct-image stage, HDR/reflection connections are described
at the top of this page. Qualified texture resources, role-correct material maps,
progressive transport and explicit glass/medium semantics remain separate work.
Community feedback is welcome on bounded latency,
normal handling, energy loss in the preview BRDF and reproducible acceptance scenes.
