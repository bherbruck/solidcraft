# Runtime sun and environment lighting

These controls extend progressive image jobs. They do not change the saved design,
its render setups, the viewport lighting, or the legacy default render.

An original artistic sky is available without downloading an image:

```json
{
  "progressive": true,
  "samples": 256,
  "max_depth": 8,
  "environment": {
    "kind": "sky",
    "gradient": {"zenith": [0.16, 0.32, 0.7], "horizon": [0.8, 0.9, 1.0], "ground": [0.12, 0.1, 0.08]},
    "intensity": 1,
    "rotation_rad": 0
  },
  "lighting": {
    "sun": {"direction": [-0.4, -0.6, 0.7], "angular_radius_rad": 0.02, "radiance": [1000, 950, 850]}
  },
  "tone_map": "aces_fitted"
}
```

The gradient is an original horizon-to-zenith/ground interpolation, sampled to
512x256. It is an artistic environment, not a calibrated atmospheric model.
Sun direction points toward the light. Its finite angular disk has uniform linear
RGB radiance and supports radii 0.001..pi/2 radians. Increasing radius at fixed
radiance increases total illumination; the cone's solid angle is
`4*pi*sin(radius/2)^2`. The sun is a separate additive emitter, so a sun already
present in an HDR image is not automatically removed.

For a captured Radiance HDR file, replace the environment object with:

```json
{"kind": "hdr", "path": "original-studio.hdr", "assume_linear_srgb": true, "rotation_rad": 0.3, "intensity": 1}
```

Input is bounded to 16 MiB encoded, 2048x1024, 2:1 equirectangular and the standard
`-Y +X` orientation. Header exposure and channel corrections are applied by the
existing decoder. Declared primaries must be sRGB/D65. A source without primaries
requires explicit `assume_linear_srgb: true`; this is an interpretation choice,
not colour conversion. Other declared primaries are rejected even when that flag
is set. No HDR values are tone mapped before lighting. Intensity is 0..100 and
rotation is a finite bounded angle in radians about Z.

Equirectangular +Z is the north pole, -Z the south pole. Column zero begins at +X;
columns run toward +Y. Positive rotation turns the image about +Z. Horizontal
coordinates wrap; polar rows do not repeat. Map radiance replaces the constant
`lighting.environment` when a map is supplied. Rectangle and sun emitters remain
additive. The camera backplate remains the request's background; environment
radiance appears through surface lighting, reflections and refractions.

The CLI accepts `--progressive --environment FILE.json`; that small JSON file
contains the environment object above. `--lighting FILE.json` supplies the sun
and/or rectangle lighting object. The engine captures bounded file bytes through
its scoped VFS. Queued jobs own immutable shared bytes and never reopen paths.
Preflight reports source SHA-256, dimensions and colour policy. A later start is a
new capture; compare its descriptor if a source file may have changed.

## Sampling and bounded preparation

The worker uses the existing cooperative HDR decoder, then builds an alias table
from luminance times exact texel solid angle. A row's solid angle per texel is
`(2*pi/width)*(cos(theta_top)-cos(theta_bottom))`, evaluated with a stable sine
identity near the poles. Within a selected texel, azimuth and cosine of polar
angle are sampled uniformly. The directional density is texel probability divided
by solid angle. Radiance uses the same piecewise-constant texel mapping. A black
image uses a uniform distribution with zero emitted radiance. Independent random
variates select the alias bucket and coin; sharing one 24-bit variate would bias
large maps.

The sun samples its cone uniformly. Environment and sun are sampled separately,
and each escaped BSDF ray receives each emitter's radiance with its own matching
power-heuristic weight. The existing opaque-branch, delta-event and final-bounce
rules apply. Thin-sheet branch densities multiply the competing BSDF density;
connections that exhaust the remaining depth use NEE only. Enabled straight
solid-shadow connections also use NEE only, with their transmitted emitter hits
suppressed to avoid double counting, as described in `render-glass-shadows.md`.
This extends the original estimator from
[PBRT fourth edition, sections 12.5](https://pbr-book.org/4ed/Light_Sources/Infinite_Area_Lights)
and [13.4](https://pbr-book.org/4ed/Light_Transport_I_Surface_Reflection/A_Better_Path_Tracer);
no PBRT source or images are copied.

GPU storage uses one additional binding, 32 bytes per texel plus a 16-byte header.
Admission conservatively adds retained encoded-buffer capacity, 160 bytes per texel and 64 KiB to the
job's existing estimate for decoded/table/work/staging/device buffers. Device
binding limits and the existing 512 MiB job limit also apply. Preparation checks
cancellation while decoding, generating rows, building tables and staging uploads.
CPU product rendering rejects environment resources explicitly.

Tests use original in-memory RGBE fixtures and the generated gradient. They cover
colour policy, malformed inputs, cancellation, polar/seam lookup, PDF normalization,
unequal-mass sampling frequencies, resource admission, furnace consistency, sun
cone quadrature and immutable engine preflight. No third-party HDRI or material
assets are bundled. Original fixture generators live in `examples/studio/textures`;
planar runtime colour/normal mapping is described in `render-runtime-textures.md`.

The showcase PNG maps and RGBE gradient are generated locally before replay with
`python examples/studio/author_showcase.py`. Image outputs are ignored and are not
committed source assets; the recipes, generators and provenance remain editable.
