# Runtime studio lighting

`render.preflight` and `render.start` accept `lighting` and `tone_map` when
`progressive: true`. They affect the captured job only: saving a design or render
setup does not persist them. Omitting lighting retains the three legacy directional
lights and their sample stream. Omitting tone_map retains Reinhard.

```json
{
  "progressive": true,
  "samples": 256,
  "max_depth": 8,
  "lighting": {
    "environment": [0.5, 0.5, 0.5],
    "area_lights": [
      {"center": [-200, -150, 300], "u": [100, 0, 0], "v": [0, -80, 0], "radiance": [8, 8, 8]}
    ]
  },
  "tone_map": "aces_fitted"
}
```

All radiance is linear RGB, bounded to 0..1000000. Environment defaults to black;
use equal RGB components for white illumination. The camera background remains
the requested backplate. The environment illuminates surfaces and is visible in
reflections/refractions. An empty lighting object makes an unlit studio.

Each of at most four rectangles uses world-space millimetres. `u` and `v` are
orthogonal half-edges, each 0.0001..1000000 mm long; the rectangle spans center
plus/minus each half-edge. Its emitting side faces `u cross v`. Centers are finite
and bounded to +/-1e12 mm. Rectangles are opaque, black on their back sides, visible
to camera and reflected rays, and occlude other emitters. No lights are added to
the model or product-scene body list.

The CLI accepts `--progressive --lighting FILE.json --tone-map aces_fitted`.
The file is the lighting object alone (at most 64 KiB); `reinhard` is the other
accepted tone map. Exposure remains `--exposure EV`.

## Estimator and output

Every opaque hit samples each rectangle once in area and the constant environment
once on the sphere. Rectangle density in solid angle is distance squared divided
by area times the emitting cosine. The environment density is 1/(4*pi). These
are combined with the BSDF continuation by the power heuristic on both direct
samples and emitted radiance reached by continuation. Each light is sampled, so
there is no light-selection probability. The actual opaque sampling density
includes its (1 - transmission) branch probability. Delta events receive emission
weight one. At the final allowed surface bounce, direct samples have weight one
because there is no continuing BSDF sample. Shadow intersections stop at the
sampled emitter; geometry beyond it cannot shadow it. See [glass shadow connections](render-glass-shadows.md) for thin-sheet MIS and
the opt-in solid straight-through direct-light approximation.

Accumulation stays linear. Only final output divides by coverage, applies exposure,
then one selected tone map and sRGB encoding. `aces_fitted` uses Narkowicz's published
rational fit; it is not the full ACES colour-management pipeline. The backplate
compositing contract is unchanged.

## References and validation

The implementation is original from the estimator mathematics in
[PBRT fourth edition, section 13.4](https://pbr-book.org/4ed/Light_Transport_I_Surface_Reflection/A_Better_Path_Tracer)
and the published coefficients of
[Narkowicz's 2016 curve](https://knarkowicz.wordpress.com/2016/01/06/aces-filmic-tone-mapping-curve/).
Tests compare rectangular illumination to independent f64 surface quadrature,
exercise blockers before and beyond emitters, compare continuing and final-bounce
MIS estimates with a white furnace, validate bounded requests, and check that
preflight leaves the document unchanged. GPU tests require an actual supported
adapter when SOLIDCRAFT_REQUIRE_GPU is set; no visual milestone is implied by these
numerical tests.
