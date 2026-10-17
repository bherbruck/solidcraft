# Runtime dielectric overrides

The GPU executor uses the agreed shared CPU runtime contract from #1655 /
#1659, coordinated in Mailbox 9063, 9068 and 9075. Its wire adapter is isolated in
`crates/engine/src/cmd/render/transmission.rs` so the future shared type can replace it.
No document format number, migration, appearance field or persistent material
contract is added. The earlier experimental `pbr.transmission` proposal is removed.

Pass this request to `render.preflight` or `render.start`:

```json
{"progressive":true,"raytrace":{"dielectrics":[{
  "body_index":0,"body_name":"Glass","ior":1.5,
  "absorption_per_mm":[0.022314355,0.005129329,0],
  "boundary":"closed_mesh","thickness_mm":0}]}}
```

At most **eight** overrides are admitted. Both captured body ordinal and exact
name from `render.scene` must match; duplicate ordinals are rejected. An override
means a full dielectric interface, with no fractional transmission-weight field.
It replaces optical metalness only in the captured job. Saved appearances,
engineering properties, meshes, undo history and navigation do not change.

IOR must be finite in [1,3]. `absorption_per_mm` contains finite linear RGB
extinction coefficients in [0,100] per mm. Beer attenuation is
`exp(-sigma * geometric_distance_mm)`, using unshifted geometric hit endpoints,
not the distance after a numerical ray-origin offset. A colour authoring UI can
convert transmittance T at a stated distance d with `sigma = -ln(T)/d`; no such UI
or implicit sRGB conversion is added here. Zero sigma means no absorption.

Boundary is one of:

- `"thin_sheet"` with a sibling `"thickness_mm":3`: thickness in [0,10000] mm. An ideal
  parallel sheet with no interior medium or lateral ray displacement. Absorption
  uses thickness divided by the refracted cosine. Incoherent internal reflections
  are summed separately for s/p polarization, then averaged. Zero thickness is
  admitted and has no absorption, but retains the two-interface optical response.
- `"closed_box"` with `"thickness_mm":0`: strict axis-aligned bounding-plane check (1e-6 mm tolerance),
  followed by the GPU's closed, outward two-manifold topology validation.
- `"closed_mesh"` with `"thickness_mm":0`: **GPU-only** extension for general closed sphere/lens meshes.
  This is not a claim that the current CPU certificate admits those shapes.

Preparation matches topology representatives within 1e-6 mm Euclidean distance,
searching neighboring spatial cells so a rounding boundary cannot split a valid
CAD seam. It does not alter uploaded geometry or join chains beyond a representative's
tolerance. Weld scratch is bounded and budgeted for the largest closed dielectric
body being validated; opaque and thin-sheet bodies allocate none of it.
Preparation requires paired opposite edges, exactly one edge-connected shell, positive outward volume, and requires
one homogeneous IOR/medium over a closed body's faces. Nested or overlapping
closed media are refused conservatively: dielectric body AABBs must be disjoint
when either body is closed. This can reject disjoint curved shapes whose boxes
overlap. Thin sheets can overlap each other's bounds. Camera and the entire lens
aperture must start outside volumes; unmatched exits fail without a published image.
Multiple shells in one body are refused with a connected-component check and a
negative two-shell fixture. Self-intersection of an otherwise oriented manifold
is **not certified**; such geometry is outside the supported input contract and
may not be detected. Edge counts and signed volume are not a proof of embedding.
The hole, inward-winding, disconnected-shell, nested/overlapping bounds and
camera-inside refusal paths have negative fixtures. GPU preflight reports
`dielectric_boundaries: ["thin_sheet","closed_box","closed_mesh"]`; CPU preflight
reports an empty list. The generic Refraction flag alone does not describe this policy.

CPU and raster previews retain independently saved scalar opacity. CPU requests
reject runtime dielectric/lens controls. Scalar alpha is never inferred to mean
physical glass. Old opaque PBR appearances still require opacity one. Unknown keys,
nonfinite inputs, invalid ordinals and out-of-range values are errors.

## Transport and validation

GPU transport uses geometric winding for entering/leaving media, exact s/p Fresnel
including total internal reflection, Snell refraction and Beer attenuation. It
includes the squared IOR ratio for radiance transport. Glass is smooth; rough
transmission and fractional mixtures are absent. Runtime [glass shadow connections](render-glass-shadows.md)
are enabled by default for thin sheets and opt-in for solids; the latter are explicitly
straight-through NEE-only approximations, not caustics. The maximum path depth remains 16.
The existing per-path stack storage is retained, but nested-entry events are errors.

`crates/render/src/dielectric_oracle_tests.rs` is an original self-contained f64
fixture module prepared for sharing with #1659: normal/oblique/Brewster s/p,
Snell and critical-angle/TIR cases, Beer attenuation, lossless and absorbing slabs,
and zero-thickness sheets. Its provenance and numerical tolerances are explicit.
GPU tests additionally cover sphere furnace, actual-mesh ball-lens focus, and
absorbing slab transport with deliberately large ray offsets to detect undercounted
absorption distance. No test tolerance was relaxed for a backend.

Secondary-ray epsilon remains max(1e-5 mm, scene extent * 1e-7). Intersections use
original f32 dominant-axis projection and conservative bounds; they do not claim
universal exact watertightness. The shared-edge regression at pixel171, sample357,
seed1948 remains covered by the furnace. The general coordinate-transform idea is
from [Woop, Benthin and Wald (2013)](https://jcgt.org/published/0002/01/05/paper.pdf);
no external shader code was copied. See [thin-lens notes](render-thin-lens.md) for
CPU draft convention differences. The production environment is still the studio;
a uniform environment exists only in numerical/evidence tests, not as HDRI support.
