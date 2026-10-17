# Combined CPU direct lights

Source-only integration: compilation, tests, native PNG inspection, Clippy,
wasm and full CI have **NOT RUN** on this revision. Historical component
results do not qualify this combination.

The existing native `render.preflight` / `render.start` engine commands, also
available through persistent MCP, accept these transient `raytrace` fields:

| Source | Field | Maximum sources and retained slots |
|---|---|---|
| Directional | `lights` or an existing `preset` | 8 |
| Point or spot | `punctual_lights` | 8 |
| Rectangle, including lowered LED strips | `area_lights` | 4 |

The combined maximum is 20 sources. Omitting directionals keeps the existing
studio; `lights: []` disables them. Presets and custom directional lists remain
mutually exclusive. All source positions and orientations are world-space;
distances and full rectangle edges are millimetres. Colours are linear sRGB.
Intensity is illustrative relative radiance, not watts, lumens, lux or measured
colour temperature. There is no source mesh or primary-visible emission.

A point has `position_mm`, `color`, `intensity`, `reference_distance_mm` and
`minimum_distance_mm`. An optional `cone` gives a spot's `emitted_axis`
(pointing from the source), `inner_half_angle_radians` and
`outer_half_angle_radians`. The existing inverse-square reference-distance
response and smooth cosine feather are retained.

A rectangle has `center_mm`, `edge_u_mm`, `edge_v_mm`, `color` and `intensity`.
The cross product of its full edges gives its emitting side. `area_samples`
is exactly 1, 4, 16 or 64, default 16, per whole rectangle per shading hit.
`seed` is an optional u32. Explicit area sampling controls require a nonempty
rectangle list. Each sample retains the existing
`BRDF * Le * cos(surface) * cos(emitter) * area / (distance² * N)` weight;
changing N does not multiply emitted power. Finite shadow rays end before the
sampled source; geometry beyond the source cannot occlude it.

An optional top-level `led_strip` has `center_mm`, `length_mm`, `width_mm`,
unit `length_axis`, unit `width_axis`, `color` and `intensity`. Top-level
`led_samples` is 1/4/16/64, default 16, and requires the recipe. One continuous
strip lowers to one existing rectangle before scene capture, without retaining
the recipe or adding a second renderer. A strip and other rectangles share N;
conflicting values refuse. Discrete pads, pitch, spectra and calibrated CCT
are unsupported.

At maximum N, one hit can attempt 8 directional + 8 punctual + 4*64 area
samples, or 272 analytic direct-source attempts. This is **not** a work-debit
or total-ray bound. Traversal, HDR preparation/sampling, primary AA, reflection
attempts and bounces add work under the same cumulative 1,000,000,000 limit.
The existing four-unit continuation quantum, cancellation, 384 MiB job limit,
512 MiB host pool and retained Record request lease remain.

All direct terms use the current captured hit and opaque/qualified planar
anisotropic BRDF, multiply by path throughput, and join the existing radiance
sum before its display transform. Delta mirrors skip these non-delta direct
terms. Each secondary hit resets area and punctual continuation state.
The existing HDR/reflection estimator partition and `primary_samples` 1/4/16
policy remain; this adds no glass, MIS, GI, denoiser or material persistence.

The ordinary CLI `render` wrapper still selects the raster path. Optional
desktop CPU/AA controls are described in [CPU ray images](render-raytrace-image.md);
they do not expose explicit light authoring. This integration adds no CLI
lighting flags or UI light editor. A persistent native MCP session with the
existing `raytrace-cpu` build and installed host is the command route for these
lights. Feature-off and unavailable hosts refuse before capture.

Written tests cover combined counts/capacities, recipe preservation and atomic
refusals, N-independent area weights, mixed radiance, continuation partitioning,
272 attempts, cancellation/work exhaustion, preflight and Record lease lifetime.
They remain **NOT RUN**. Combined anisotropic/reflection/HDR fixtures, CLI
workflow parity, exact-build layout and presentation acceptance remain pending.
No external assets, dependencies or license changes are introduced; the existing
SolidCraft implementations and their contribution licenses are retained.
