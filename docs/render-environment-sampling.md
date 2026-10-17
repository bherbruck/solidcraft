# HDR environment sampling (#1693)

Source implementation and opt-in CPU direct-light consumer are prepared. No
executed HDR-lit PNG, test, full gate or delivered HDRI feature is claimed. The
existing default preview remains unchanged.

`lighting::environment_sampling::EnvironmentPreparation` borrows an existing
`DecodedHdr`. It checks bounded 2:1 dimensions and original pixel allocation
capacity, then validates at most 16,384 pixels per step. The caller supplies a
fallible work/cancellation callback, charged for each inspection. Errors consume
preparation; `finish` refuses incomplete validation. No decoder, cache, CDF,
pixel clone, worker or pool is added. The existing owner and admission lease must
outlive the borrowed view. `retained_pixel_bytes` reports capacity, not length;
the host charges the owner for its retained lifetime. Captured image storage is
released before PNG publication; retained capture metadata has its own Record lease.

## Coordinates and estimator

Directions point away from the shading point. The map is Z-up: +Z is north,
`phi=atan2(y,x)`, `u=wrap(phi/(2*pi))`, `v=acos(z)/pi`. Nearest lookup uses
half-open texel cells centred on `((x+.5)/width,(y+.5)/height)`. Longitude wraps,
the south pole clamps to the last row, and an exact pole chooses longitude zero
before rotation. Positive rotation turns the environment about +Z toward +Y;
lookup subtracts that rotation. There is no extra orientation inherited from an
external application's conventions.

The first proposal samples uniform z in (-1,1) and phi in (0,2*pi). Since
`dOmega=dphi*dz`, its PDF is `1/(4*pi)` per steradian everywhere. The PDF remains
normalized for black maps, without a division by total image brightness. Constant
maps preserve their radiance. This is not importance sampling; small bright
sources can have high variance. Nearest lookup is intentionally not bilinear
filtering or a claim of antialiasing. Floating-point boundary rounding can still
produce a pole, whose lookup is defined.

`direct_sample_linear_srgb` returns an unoccluded `BRDF*cos*Li/pdf` estimate.
The geometric normal excludes incident and view directions crossing the original surface;
the shading normal affects the BRDF and cosine. The consumer owns visibility and
averages exactly once over the requested environment sample count. The helper
does not apply exposure, mapping, sRGB encoding or a sample-count division.

## Colour policy

Decoded HDR is linear RGB in its SOURCE space. RGBE decoding has already divided
by the file's exposure and colour correction; sampling never repeats either.
Source-space lookup is explicitly named `radiance_source_rgb`.

The initial lighting adapter admits only explicit `PRIMARIES` values matching
sRGB/D65: `0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290`. It uses these linear
channels directly, without applying the sRGB transfer curve. Missing primaries
and all other primaries refuse illumination; default Radiance RGB is not silently
relabeled sRGB. Supporting other primaries requires a separately validated gamut
and white-point conversion. No clipping or invented conversion is used here.

## CPU consumer source and remaining integration

The new branch starts from actual upstream history. The separately qualified RT2
source is not a commit parent or an executed validation result for this module.
Integration must use its real prerequisite commit or an explicitly recorded
source overlay before its own gates; no copied build outputs.

1. Capture the admitted existing HDR source and preserve its original encoded allocation.
   Include encoded/decoded retained capacities, request clones and state in the
   existing 384 MiB job and 512 MiB shared admission. Capture metadata keeps its
   Record-owned lease after frame eviction. Refuse unsupported primaries before publication.
2. Incremental preparation calls the SAME cumulative job work budget and cancel
   token. The implemented owned adapter moves `EnvironmentInput` through host work
   into one existing `HdrDecodeTask`. The image task owns its completed `DecodedHdr`
   and creates short-lived borrowed views, without a self-referential borrow or a
   second decoded copy. Decoding validates corrected texels incrementally; internal
   views check shape and capacity without rescanning every texel for each sample.
   Execution and visual acceptance remain pending.
3. At an opaque hit, each environment direct sample creates one positive-minimum
   secondary visibility ray from the shared geometric offset. Reuse the existing
   resumable traversal cursor, charging every sample and traversal unit. Add visible
   estimates in linear space and divide by environment sample count once.
4. Primary misses remain equal to the requested display background. The
   `radiance_linear_srgb` API is available for later secondary-miss transport;
   reflection continuation is not connected in this consumer. Combining environment
   next-event estimates and rough-specular paths that miss requires matched MIS
   or a documented partition of the two terms; until implemented, refuse that
   combination. Do not count the same environment specular energy twice.
5. Add environment illumination to existing direct lights without adding another
   ambient term. Exposure, Reinhard and sRGB transfer remain a single final step.
   Image/task/host cancellation must publish no partial successful frame.

Original analytic tests generate RGBE bytes at runtime: constant map and PDF,
single bright cell solid angle, black map, axes/rotation/seam/poles, cancellation
and shared-budget failure, decoder corrections, unsupported primaries, hostile
options/directions and an independent white-conductor hemispherical integral.
They have NOT RUN. No external assets, dependencies or persistence fields added.

Mathematical references only; no code or figures copied:
[PBRT, Infinite Area Lights](https://pbr-book.org/4ed/Light_Sources/Infinite_Area_Lights)
for radiance and solid-angle estimators (its image mapping is different), and
[W3C CSS Color 4, sRGB](https://www.w3.org/TR/css-color-4/#predefined-sRGB)
for the sRGB/D65 primary coordinates and the distinction from encoded sRGB.

## Opt-in request (not yet executed)

`render.preflight` / `render.start` accept an `environment` object alongside
`integrator:"raytrace"`, for example:

```json
{"integrator":"raytrace","raytrace":{"lights":[]},"environment":{"path":"original-srgb.hdr","rotation_radians":0,"intensity":1,"samples":64}}
```

The input path uses the existing scoped, bounded VFS reader and `HdrSource`.
The captured source is passed separately to `CpuHost::submit_with_environment`,
never serialized or cloned into job status. A separate fixed-size `EnvironmentInfo`
records the SHA-256 digest, numerical header, controls and explicit colour policy in
preflight and status, without the source path or pixels. Encoded capacity, exact decoded storage,
scanline storage and bounded state are added before the shared reservation;
allocators that exceed admitted scanline/decoded capacities refuse. Direct-light
visibility uses the existing positive-minimum ray and resumable cursor. Primary
misses retain the exact requested background. Samples use deterministic z/phi
stratum centres (one sample uses a fixed interior point); finite-count quadrature
can be biased and is not a convergence or unbiased-rendering claim.

The standalone HDR checkpoint has no reflection continuation. This isolated
combined source connects secondary environment misses and explicitly selected
metallic rough reflectors or ideal mirrors; see
[the integration contract](render-hdr-reflections-integration.md). Mixed/diffuse
rough HDR combinations and unknown optics fields refuse. No MIS, transmission,
texture, emissive geometry, GPU or saved HDR environment is advertised. Existing
saved environment controls remain refused; runtime controls are explicit above.

Seven consumer source regressions complement the nine analytic/input regressions:
an image-task PNG oracle and deterministic/background checks, decode/visibility
cancellation and work exhaustion, native captured-source ownership, four-unit
quanta, all traversal-to-shading transitions, engine rendering after source-file
removal compared with unlit geometry, and capture-metadata lease release after
PNG eviction. All 16 tests remain NOT RUN.

Consumer review repairs: environment cursor continuation charges traversal without
an additional shading debit; each pixel quantum remains at most four shared work
units. Both request resolvers require `Hdri` when a source is captured. Only the
native feature-on raytrace descriptor advertises this narrow environment path.
Preflight and job status retain fixed-size SHA-256 bytes, bounded header numbers,
controls and explicit colour policy; neither the input path nor image bytes are
retained there. Fixed metadata is charged to the Record-owned lease independently
of the PNG, while captured image storage stays under the work lease. Preflight
includes retained request metadata in its combined per-job estimate. A regression
withdraws the actual HDR file and compares lit geometry against the same unlit job.
All tests remain NOT RUN.
