# Ambient occlusion

Enable **Ambient occlusion** in the viewport's **View settings** menu or in
**Preferences → Graphics**. It adds soft contact shading at corners, recesses and
nearby opaque surfaces. It is off by default, applies immediately, and is saved
between runs. The shared **Graphics Preset** selector in both settings surfaces
combines AO with the display-quality and supersampling controls from #1319 and #1321:

| Preset | Ambient occlusion | Display quality | Supersampling |
|---|---|---|---|
| Performance (default) | Off | Normal | Off (1×) |
| Quality | On | Fine | 2×, only while the view is still |
| Custom | Your manual combination | Your manual combination | Your manual combination |

Custom is derived from the current settings, not a third preset that overwrites them.
Individual controls remain editable. Selecting a preset applies all three settings
immediately; MSAA, ground shadow, projection and visual style stay unchanged.
Graphics preferences support Cancel, Apply and Restore Defaults. Adaptive supersampling
keeps #1321's 0.25-second settling delay, texture limit and 384 MiB memory cap;
the requested 2× may be reduced or skipped when the device cannot accommodate it.

The control channel and connected MCP clients can use the same command as the UI:

```json
{"command":"view.ambient_occlusion","params":{"enabled":true}}
```

`view.graphics_preset {"preset":"quality"}` (or `"performance"`) selects the preset.

`ui.prefs {"ambient_occlusion": true}` also sets the preference. The command changes
only display preferences, without adding a modelling undo step or moving the camera.

The GPU viewport uses 32 deterministic hemisphere samples per shaded pixel against
an opaque depth prepass (at most 2048 pixels on its longest side). The radius follows
model size, and depth-range rejection avoids shadows from distant foreground objects.
It supports orthographic and perspective cameras and respects section clipping.
Wireframe, transparent bodies, overlays and surface-analysis colours are unaffected.
Disabling it skips the depth prepass and sampling and releases the full-size depth map.

This is screen-space shading: off-screen and hidden surfaces cannot contribute.
The software fallback and CPU model snapshots do not apply this GPU effect. Capture
the actual window with `ui.screenshot` when reviewing it.

## Metal startup comparison (2026-10-15)

On an Apple M3 Pro (18-core GPU, 36 GiB unified memory), macOS 27.0
(26A428), both unchanged main `0872eb95` and this branch merged with that
revision reproduce the same startup defect: `solidcraft --sample` shows the
plate's edges and ground shadow but no opaque faces. AO is disabled on the
branch and does not exist on main. Both were built with `--profile ci`, launched
separately on Metal with MSAA 4x, and captured at 1200 × 850 logical pixels
(2400 × 1700 physical pixels). Their viewport captures are pixel-identical.
This establishes that the symptom also exists on main; this PR does not fix it.

For the AO comparison screenshots and timings below, the display mesh was
rebuilt by assigning the sample Plate opacity 0.99 and then restoring it to 1.0
(with the same colour, `#a6b0bd`). Measurements use opaque faces, GPU
tessellation and the actual native viewport after that rebuild. The screenshots
are attached in the review conversation, rather than committed as generated assets.

## AO-only rendering performance (2026-10-15)

These measurements predate the combined presets. They isolate AO at Normal
tessellation with supersampling off; they are **not** timings for the combined
Quality preset, which also requests Fine tessellation and adaptive 2× supersampling.

Same M3 Pro / macOS machine and `ci` build profile as above (not a release-build
claim). Window: 2400 × 1700 physical pixels; model viewport: 1900 × 1312.
Orthographic home view, shaded with edges, MSAA 4x, normal display quality,
supersampling off, ground shadow on, grid/origin/sketches hidden. The heavier
design is a rectangular body pattern of the sample: 5 × 5 plates, X spacing
90 mm, Y spacing 60 mm, fitted to the viewport (25 opaque bodies).

These are **completed render-work milliseconds per frame**, not the UI-only
`ui.inspect.frame_ms` or a claimed display FPS. Temporary native instrumentation
measured from the beginning of `paint_and_update_textures` through command
submission and `device.poll(PollType::wait_indefinitely())`. The renderer's
accumulated surface-acquisition/queue-submit timing was subtracted; presentation
and UI layout are excluded. This includes CPU render preparation and waiting
for GPU completion. Continuous repaint was enabled. Instrumentation was removed
from the source after building the measurement binary.

Each design ran three alternating Performance/Quality pairs: 2 seconds warm-up
after each switch, then 3 seconds recorded. No builds ran during measurement.
The table pools the recorded frames; p95 exposes variability on this shared
desktop. It is a local comparative measurement, not a hardware-independent FPS
guarantee or a modelling/recompute benchmark.

| Design | Preset | Frames | Median render ms | p95 render ms |
|---|---|---:|---:|---:|
| sample | performance | 1082 | 2.52 | 2.65 |
| sample | quality | 1077 | 6.35 | 7.27 |
| 25-plates | performance | 1074 | 3.15 | 3.45 |
| 25-plates | quality | 1082 | 6.93 | 7.50 |

Per-trial medians (ms), in chronological order:

- sample-performance: 2.53, 2.47, 2.53.
- sample-quality: 5.82, 4.48, 7.08.
- 25-plates-performance: 3.13, 3.16, 3.14.
- 25-plates-quality: 6.80, 7.02, 6.95.
