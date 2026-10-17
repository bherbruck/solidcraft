# GPU depth of field

`render.preflight` and `render.start` accept an optional runtime lens:

```json
{"progressive":true,"samples":256,
 "lens":{"aperture_radius_mm":0.5,"focus_distance_mm":120}}
```

Use a perspective camera in `settings.camera` or the current view. Focus distance
is measured from the eye along the forward axis, to a plane perpendicular to that
axis. It is not distance along each ray. Aperture is the physical radius in mm,
not an f-number. Units follow the scene; a radius of 0.5 mm is small for a room and
large for a 3 mm object. No saved camera or render setup fields are added.

A uniform-area circular aperture uses `r = radius * sqrt(u)` and `phi = 2*pi*v`.
Each primary ray starts at that aperture point and aims at the pinhole ray's
intersection with the focus plane. The image is the average of these radiance
samples: changing aperture changes blur, not an aperture-area exposure factor.
Exposure remains the existing explicit EV control. Circular bokeh has no blade
shape, diffraction, lens distortion or chromatic aberration model.

Omitting the lens or choosing radius zero preserves the existing pinhole or
orthographic image bit-for-bit on the same adapter, including its random stream.
Positive aperture requires perspective; orthographic requests fail admission.
Radius is finite in [0, 1e7] mm and focus distance in [1e-6, 1e9] mm. Active lenses
also require camera distance >= 1e-6 mm and tan(fov/2) in [1e-6, 1e6] to keep the
f32 basis and focal point bounded. Existing image and scene caps still apply.
The whole aperture must remain outside glass volumes, as must the pinhole camera.
No UI lens editor or dedicated CLI flags are included; use the shared render
commands through the control protocol, scripts or MCP.

Tests read rays from the production WGSL camera function and compare against
f64 geometric focus and defocus oracles, including an oblique rolled camera.
At axial depth z, the displacement from a pinhole ray is
`aperture_point * (1 - z/focus_distance)`: zero at focus, opposite-signed beyond it.
Other tests cover strict admission, zero-aperture image identity and fixed-seed
repeatability with an active lens. Glass evidence is an original procedural scene,
with test-only uniform environment lighting, not a claim of HDRI implementation.

## CPU draft coordination

Read-only comparison: [CPU integration draft #1698](https://github.com/tangency-labs/solidcraft/pull/1698),
head `463a3e0daaa6b84f169ba5f8f215fba4642c194a` (2026-10-17).
No source from that draft is copied into this change.

- `raytrace/reflection.rs` uses the existing opaque surface's Schlick Fresnel,
  GGX NDF sampling and solid-angle PDF `D(h)*n.h/(4*v.h)`. Our opaque lobe already
  shares that surface oracle; the GPU mixes GGX and cosine samples with equal
  probability. CPU sample sequences differ, so comparisons are statistical.
- The draft has no dielectric transport or thin-lens implementation to use as an
  oracle. GPU glass retains the f64 Fresnel, Snell, slab and ball-lens checks.
- Both intersections use dominant-axis projection, closed edges and two-sided
  triangles. CPU uses f64 with outward representable bounds; GPU uses f32 after
  target rebasing and conservative epsilon bounds. CPU secondary rays use a
  component-wise barycentric error bound and outward representable steps; GPU
  still uses max(1e-5 mm, scene extent * 1e-7). This precision difference is
  explicit; no CPU/GPU intersection identity is claimed.
- `lighting/environment_sampling.rs` defines +Z north, longitude atan2(y,x),
  latitude acos(z), and inverse positive +Z rotation for lookup. The future GPU
  environment adapter should use these conventions. CPU samples the sphere
  uniformly with PDF 1/(4*pi); a GPU luminance CDF would use its own correctly
  normalized solid-angle PDF and MIS. Neither is implemented by this lens change.
- CPU accepts only explicitly declared sRGB/D65 HDR primaries. GPU HDR input
  must honor that boundary or provide a separately validated gamut conversion.

The agreed shared dielectric fields are IOR, linear RGB extinction per mm and
thin-sheet thickness vs certified closed boundaries, identified by ordinal and name.
Persistence belongs to the shared contract follow-up, not this GPU executor slice.
