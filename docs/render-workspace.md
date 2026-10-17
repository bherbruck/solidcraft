# Integrated Render workspace (R3)

The native image panel also offers [progressive opaque rendering](render-path.md) when its compute executor is available.

R4a builds on this viewport contract with a separately advertised CPU product-image
executor and Check/Start/Cancel/Export controls. See [render jobs](render-jobs.md) for
the asynchronous protocol, image limits, captured revisions and PNG export. The legacy
`ui.render` snapshot remains a separate route.

Choose **Render** in the workspace switcher after finishing a modelling operation.
The current integrated GPU renderer shows a separate product scene. Return to Design
restores its camera, projection preference and existing viewport drawing policy.
No second engine is installed or selectable.

## Product scene

`engine::product_scene::extract` consumes authoritative evaluated geometry, reusing
the document's world/occurrence placement and mesh Arcs. Body, component, material,
face and imported appearances retain their existing precedence and opacity. Source
geometry is never transformed a second time or altered to serve a renderer.

The shared L1 `ProductScene`/`ProductMesh` contract contains world-space meshes,
resolved sRGB colours, opacity and face appearance overrides. Face indices are the
existing mesh-to-B-rep mapping, resolved through document appearance references;
they are not new persistent names or triangle-derived face identities.

There are no grid/axis/edge lines, sketches, construction objects, selection or hover
colours, section cuts, analysis shading, active-component fading, canvases or decals
in this initial product policy. Browser visibility still controls which bodies appear.
The existing Design viewport and CPU snapshot routes keep their current policies.

Extraction refuses a stale live-drag model. The explicit model-reading `render.scene {}`
command follows the normal engine evaluation boundary, then returns bounded metadata:
body names, triangle counts, resolved colours/opacity, face-override counts and bounds.
It does not return mesh payloads, change the document or add an undo step. Product
extraction admits at most 16,384 visible bodies.

## Settings and camera

The right panel stages a named setup's background and camera changes. **Save Setup**
records the current existing `Camera` snapshot through `render.settings.set`, with
one undo step; the other R2 fields retain their values. **Reload Saved Settings**
discards staged settings. Draft settings are separate for each open document.

**Restore Saved Camera** runs `render.settings.apply {name}`, preserving target,
distance, yaw, pitch, roll, FOV and up axis as view state. It neither edits the document
nor settles a stale model. The UI honours its up axis in Render; the Design camera
is restored when returning. The camera moves exactly as in Design: the navigation preset,
Orbit around, zoom to the cursor, the SpaceMouse, the middle double-click, F6 and the axis
view or navigation cube all work (nothing is picked or selected here), and
**Fit Product** frames visible product bodies without including sketches or selection.

Preview drafts do not make the document dirty until saved. Unknown backend preferences
and unsupported modes remain saved intent: the strict resolver reports the unsupported
viewport request. **Use Integrated Preview** is an explicit draft change, never an
automatic fallback. There is no final-mode or second-engine selector in this slice.
Saved exposure/environment/output fields are not newly applied to shading or export.

## Host capabilities and resources

`render.backends` always retains the built-in `cpu-raster` preview/PNG image executor.
Only an installed UI GPU host adds `gpu-viewport`, advertising **Preview / Viewport**
with no image formats or HDRI/refraction/progressive features. The installed
[opaque PBR consumer](render-opaque-pbr.md) adds the `pbr` feature. The host reports
effective limits from its created device and depth attachment: at most 4096 pixels
per dimension and 4,194,304 pixels, within the 256 MiB pixel scratch admission policy.
The estimate includes the selected MSAA colour/depth cost; it excludes scene geometry,
driver overhead and cached source meshes. It is not a measurement of total GPU memory.

The preview never refuses a frame because of the screen size. When the viewport's physical
size is past those limits (a maximised 4K or 5K window is 8–15 million pixels), the preview
draws at the largest size of the same aspect the executor admits (`RenderLimits::fit`, both
axes scaled by one factor, scratch included) and filters it up into the viewport. A viewport
within the limits draws at full size.

PNG images keep the saved-setup limits above. A setup past them is refused with the limits and
the largest size at its aspect (for example `4096 × 4096` → `2048 × 2048`), and the image panel
offers to use that size.

Pure capability metadata lives in the session/host, outside persistence and history,
surviving document replacement, scratch sessions and document-tab switches. No wgpu
types enter the engine or document crates. A host without an installed GPU/depth buffer
reports that product viewport preview is unavailable, rather than silently using the
opaque CPU snapshot path to approximate transparent appearances.

GPU product batches are separate from Design batches while sharing the installed
device and pipelines. Model/visibility/appearance changes update the product scene;
camera movement updates uniforms without rebuilding vertices. Selection and section
state do not change the product buffers. Vertex uploads are bounded to 128 MiB and
checked before allocation; malformed/non-finite triangles and invalid opacity fail.
Reinstalling the host invalidates product uploads. This is not a general device-loss,
render-job or cancellation protocol; those ownership mechanisms belong to R4.

Transparency uses the existing raster far/near alpha-blending approximation, without
refraction or HDR illumination. Opaque PBR uses a separate surface batch in the
same installed renderer. R3 itself added no export job; R4a supplies CPU product
PNG jobs. There is no result gallery or Bevy dependency. Saved setups were added
in format 21, alongside opt-in optical parameters on existing looks.
`ui.render` remains the existing Design CPU snapshot API; `ui.screenshot` captures
the actual integrated Render workspace. Product-image export uses `render.start`
and `render.export`, rather than the Design snapshot API.

## Reproduce

Run the original procedural `examples/render-studio.json` through the engine/CLI and
open its saved design in the app. Select Render, adjust its background or camera and
save a setup. The fixture uses original boxes/cylinders and simple colour/opacity
assignments; no commercial CAD files, artwork or materials are used. Actual Windows
Vulkan/MSAA evidence is attached to [#673](https://github.com/bherbruck/solidcraft/issues/673).
