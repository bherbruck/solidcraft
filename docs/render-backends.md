# Render backend capabilities (R1)

Native progressive opaque PNG rendering is described in [Progressive product images](render-path.md). It registers alongside the CPU executor.

This page describes the R1 metadata contract. The installed GPU viewport and CPU
product-job hosts extend it through complete profiles; HDR input preparation adds
no lighting capability. See [the integrated phase boundaries](render-integration.md).

`render.backends` discovers render metadata without opening a window, probing a GPU,
evaluating the model or changing the document, camera, dirty state or undo/redo history.
It also leaves a model made stale by a live sketch gesture stale.

```sh
solidcraft-cli exec render.backends '{}'
```

The control channel and MCP `execute` use the same engine command. Parameters must be
an empty object; a null parameter value is normalized to `{}` by the engine. Other
parameters fail with a normal command error.

## Discovery schema

The command returns this JSON object (schema version 1):

```json
{
  "schema_version": 1,
  "default_backend": "builtin",
  "backends": [{
    "id": "builtin",
    "label": "Integrated",
    "availability": {"status": "available"},
    "executors": [{
      "id": "cpu-raster",
      "availability": {"status": "available"},
      "profiles": [{
        "mode": "preview",
        "operation": "image",
        "formats": ["png"],
        "features": []
      }],
      "limits": {
        "max_dimension": 4096,
        "max_pixels": 4194304,
        "max_scratch_bytes": 268435456
      },
      "scratch": {
        "sample_scale": 2,
        "sample_bytes_per_pixel": 8,
        "output_bytes_per_pixel": 8,
        "encoding_bytes_per_pixel": 8
      }
    }]
  }]
}
```

An unavailable backend or executor instead has
`"availability": {"status": "unavailable", "reason": "..."}`. These are runtime
descriptors, not design-file settings. Executor IDs are scoped to their backend.
Backend/executor IDs are 1–64 ASCII bytes: a lowercase letter followed by lowercase
letters, digits, `.`, `_` or `-`. Both construction and deserialization validate IDs.

Without runtime hosts the integrated backend describes the existing CPU raster snapshot
path. Its `preview`/`image` profile exports PNG; it does not claim HDRI, PBR,
refraction, progressive rendering or a final-quality integrator. The existing GPU
viewport registers separately when the UI installs its actual device/pipelines.
The installed GPU product viewport and CPU product-image host support the subsequent
[opaque PBR consumer](render-opaque-pbr.md); this does not alter the static raster
descriptor or enable HDRI/refraction/final rendering. CPU and GPU are execution paths, not separate
user-selectable engine families.

## Pure request resolution

`solidcraft_render::resolve_request` accepts a descriptor slice and a `RenderRequest`.
It validates and resolves metadata only: it does not allocate an image, start a job,
reroute `snapshot` or call a renderer. A request can be deserialized from:

```json
{
  "backend": "builtin",
  "mode": "preview",
  "operation": "image",
  "width": 640,
  "height": 480,
  "format": "png",
  "features": [],
  "policy": "strict"
}
```

`backend` defaults to `builtin` when absent/null. `features` defaults to `[]`, and
`policy` defaults to `strict`. Mode, operation and positive integer dimensions are
required. An image requires a supported format; a viewport requires absent/null
`format`. Unknown fields, enum values, invalid IDs and incorrectly typed values fail
deserialization. Dimensions are checked by the resolver. No floating-point request
fields are introduced.

Modes are `quick`, `preview`, `final`; operations are `viewport`, `image`; formats
are `png`, `exr`. Required features can be `hdri`, `pbr`, `refraction`, `progressive`.
These names define requests, not promises of support. Each executor lists complete
mode/operation profiles with their formats and features. Capabilities are never
combined across executors to create an unsupported profile.

Resolution rejects duplicate backend IDs, executor IDs within a backend and duplicate
mode/operation profiles. Registry sizes are bounded to 32 backends, 16 executors per
backend and 16 profiles per executor; a request allows at most 32 feature entries.
Available executors are considered in lexicographic ID order, and the first executor
that satisfies the entire request is selected. Registry order does not convey priority.
Returned profile formats/features are sorted, so their declaration order does not
change the resolved result. If every candidate fails, the first matching executor's
error is returned; unsupported profile is reported when none has that combination.

The resolved value contains `backend`, `executor`, `profile`, `width`, `height`,
`format`, `estimated_scratch_bytes` and `warnings`. Under strict policy warnings are
empty and no requested setting is downgraded. Errors are typed `ResolveError` values:
invalid IDs/request/registry, duplicate IDs, unknown/unavailable backend, no available
executor, unsupported profile/format/feature, invalid dimensions, exceeded limits or
size overflow. There is no fallback policy or public job API in R1.

## Resource admission policy

The new resolver's default limits are conservative policy values, not measured adapter
capacity: 1–4096 pixels per dimension, at most 4,194,304 output pixels and a 256 MiB
estimated pixel-buffer budget. Each executor supplies configurable `RenderLimits` and
a `ScratchLayout`. Checked arithmetic covers dimensions, supersampling, buffer products,
sums and the target's addressable allocation size, including wasm32.

For the existing CPU raster path the estimate is 48 bytes per output pixel: four
supersamples at 8 bytes each (RGBA8 plus f32 depth), an 8-byte output canvas, and an
8-byte allowance for an RGBA copy and encoded output. At 640 × 480 this is 14,745,600
bytes; at 2048 × 2048 it is 201,326,592 bytes. A 4096 × 4096 request exceeds the pixel
limit despite each dimension being permitted.

This is an admission estimate, not a total process-memory bound. It excludes scene,
texture, projected-geometry, acceleration-structure and codec-internal allocations.
Future execution hosts must enforce their own complete allocation/cancellation policy.
The legacy rasterizer, CLI snapshots and GPU viewport keep their existing behaviour
and limits; none is routed through this resolver in R1.

The follow-on Render workspace, persistent setups, job hosts and optional engines are
tracked in [#620](https://github.com/bherbruck/solidcraft/issues/620). The implemented
foundation is scoped by [#642](https://github.com/bherbruck/solidcraft/issues/642).
