# Original SolidCraft studio scenes

These are editable command recipes, not imported models or triangle meshes.
Every part is authored here from sketches, extrusions, primitive solids, boolean
cuts and swept pipes. There are no manufacturer logos, copied product assets or
external textures. `author_scene.py` is the optional local authoring tool; the
application reads the emitted JSON without Python.

## Lighting study

`stage1.json` is a five-body study of a hollow rounded black shell, an exploded
lid, a chrome sphere and puck over a real floor. `stage1-render.json` supplies
three rectangular softboxes and a neutral environment, with no clear-coat or
transmission overrides. This separates lighting review from material review.

## Exploded coffee machine

`coffee-exploded.json` contains 32 named B-rep bodies. The cylindrical brewing
head has an open horizontal bore, separate collar, recessed shower screen and
capsule slot in its floating roof. A round swept chrome lever has ball ends.
Other parts include a hollow D tank and detached matte lid, a hollow ribbed bin,
D drip tray, a 42-hole stainless grate, rounded cap, nine-slot/four-hole service
panel, eight slotted zinc screws and an S cable ending in a two-pin plug.

The tank is cut from one extruded solid, leaving 3 mm walls and a 4 mm base.
Its semicircle is a 64-chord editable sketch: maximum outer radial deviation
from the ideal 43 mm circle is `43*(1-cos(pi/128)) = 0.01295 mm`. This is a
deliberate geometry fixture approximation. The analytic-arc version produced
unpaired tessellation edges at the current 1e-6 mm dielectric topology check;
we did not relax that check or repair the renderer mesh silently. The emitted
tank has 532 triangles with paired opposite directed edges; the bin has 178.
Both pass an independent OBJ edge check at the same 1e-6 mm position precision.
These counts describe the baseline CLI and are to be rechecked after integration.

`coffee-exploded-render.json` adds seven coated-plastic bodies and two closed
dielectric bodies. Body indices and names are both explicit and must match
`render.scene`; editing the body list requires regenerating those indices.
Smoky brown tank absorption is [0.045, 0.075, 0.12] per mm at IOR 1.586; the bin
is nearly colourless at IOR 1.49. The black lid is opaque and matte. Metal is
isotropic GGX; the grate is satin, not a claim of anisotropic brushed shading.

Both render requests are 1920 by 1080, use an explicit front-left camera 30
degrees above horizontal, and have no lens override or depth of field. Three
large area lights and a neutral environment illuminate a real white-grey floor.
The floor extends beyond the camera's horizon intersection and fills the image;
it is a finite large studio floor, not an infinite-plane/shadow-catcher feature.

For a true cyclorama, `stage1-sweep.json` and `coffee-exploded-sweep.json`
replace only that floor with an extruded, closed YZ cross-section. Its exact
circular inner curve is tangent to the horizontal floor and vertical wall;
the concentric outer curve gives constant thickness. There is no geometric
crease or separate floor/wall overlap at either join. `author_sweep.py`
regenerates these variants from the baseline recipes. Use the corresponding
baseline render request; all object, light and camera coordinates, appearances,
body names and body ordinals are unchanged.

The stage1 sweep is 3000 mm wide, with a 4 mm thickness, a floor starting at
y=-1500, a 200 mm inner radius starting at y=250,z=0, and a rear wall at y=450
rising to z=1500. The coffee sweep scales these dimensions by 2.8: 8400 mm
width, 11.2 mm thickness, floor start y=-4200, a 560 mm curve starting at
y=700,z=0, and wall y=1260 rising to z=4200. Both are real shadow-receiving
opaque CAD solids, not image backplates. Integrated GPU comparison determines
the final lighting presentation; existing flat-floor evidence remains separate.

`materials.json` maps semantic names to intended material classes. Saved
appearances remain the opaque PBR preview. Runtime optical controls belong to
the render request, independently of engineering material properties.

## Geometry review

The recipes were initially replayed and saved with the compatible pre-existing
CLI during authoring. The integrating worker rebuilds the exact source and
repeats replay, geometry checks and GPU preflight before publishing render
evidence. Reference volumes below are calculated from the authored dimensions;
the measurement command integrates a tessellation, so a relative tolerance of
0.0003 is appropriate for these isolated checks.

| Body | Analytic volume, mm³ |
|---|---:|
| Coated curved shell | 65805.30723760049 |
| Floating coated lid | 25908.18530717959 |
| Chrome sphere | 102160.40430453529 |
| Chrome floating puck | 12666.901579274047 |
| Open brewing head | 265024.75625683495 |
| Perforated stainless grate | 15815.487854690262 |

For the tank use the polygon area, not the ideal circle: with
`D(r,b)=2*r*b+32*r*r*sin(pi/64)`, its volume is
`171*D(43,45)-167*D(40,42)`. Every body must have positive finite measured volume
and one kernel shell; transparent boundaries additionally require paired
opposite edges, one connected triangle shell and positive signed volume.

Example replay (outputs belong outside the repository):

```text
solidcraft-cli run examples/studio/coffee-exploded.json --save coffee.solidcraft
solidcraft-cli eval coffee.solidcraft --json-out coffee-measurements.json
```

Use the evidence runner with the paired render-request JSON to capture runtime
lighting/material controls. The plain `render` CLI does not load that JSON as
its request automatically.


## Thick tumblers in an original room

`tumblers.json` is a separate 69-body scene. Two thick, open, tapered glass
tumblers rest on a dark walnut table, with a filleted upholstered sofa, soft
rectangular cushions, a potted plant, and a real slatted window behind them.
`author_tumblers.py` defines every dimension. The glass is made by revolving
closed radial sections: tall outer radii 36/42 mm and height 112 mm, 4.5 mm
walls and a 9 mm bottom; short radii 37/43 mm and height 94 mm, 5 mm walls and a
10 mm bottom. These are analytic curved B-reps, not polygon replacements.

`tumblers-render.json` supplies a warm small area light behind the blind slats,
two broad fill panels, two closed dielectric overrides and a thin lens. It is
1920 by 1080, with a 5.5 mm aperture radius and 670 mm axial focus distance.
The blind shadows require real light transport; the recipe contains no painted
shadow texture. `tumblers-materials.json` lists semantic material classes and
suggested world-coordinate texture projections. The scene's saved walnut and
linen appearances are plain colours: grain, woven texture and normal-map
transport must be provided by the integrating renderer and are not implied by
these material names.

`tumblers-sun.json` and `tumblers-sun-render.json` are a separate daylight
variation; the original studio-lit recipe and request are unchanged. The only
geometry change is a real 1850 by 1080 mm opening through the rear wall at
x = -1450..400, z = 760..1840. Without this opening, the intact rear wall would
occlude the infinite sun even where rays passed between the modelled blinds.
The sun points toward [-0.4, 1, 0.4], has a finite 0.00465 rad angular radius,
and uses warm radiance [18000, 16500, 14000]. An original procedural sky
gradient supplies cooler fill; it is an artistic gradient, not an atmospheric
simulation or an imported environment photograph. No extra area lights are
used in this variation. The camera, lens, glass and sample settings are retained.

From the table point [0, 0, 480], the central sun ray reaches [-576, 1440, 1056]
at the blind front and [-800, 2000, 1280] at the rear-wall opening. The ray is
above the sofa; the blind slats, 12 mm thick and 45 mm apart, therefore produce
real directional shadow stripes on the table. Their 52 mm depth also affects
the occluded band width. Finite-disk softness and the final visible pattern must
be checked in a GPU render; these analytic coordinates do not substitute for it.

On the authoring baseline the two tumbler meshes have 4856 and 4160 triangles.
Independent topology checks using Euclidean 1e-6 mm seam matching find zero
unpaired directed edges. Rounded integer keys alone rejected a few points on
opposite sides of a quantization boundary despite their sub-tolerance distance.
The accompanying renderer fix searches adjacent cells at the same tolerance;
it preserves original vertices, winding and connected-shell checks. Integration
must rerun those tests and GPU admission before claiming this scene rendered.

All three scenes have positive finite measured volume and one kernel shell per
body on the authoring baseline. The evidence report must state the exact rebuilt
source and backend; the initial CPU snapshots are composition checks only.

## Runtime showcase requests (render validation pending)

First generate the original local resource files from the repository root:

```text
python examples/studio/author_showcase.py
```

The source tree contains generators, editable scenes and runtime requests only.
The four PNG maps and RGBE gradient are generated on demand into ignored paths in
`examples/studio/textures`; they are not committed source assets. A fresh checkout
must run this command before replaying a textured or HDR showcase request.
The script uses only Python's standard library and downloads nothing.

Run the production `studio_evidence` example from the repository root so its
scoped VFS resolves these generated relative resource paths. Rendered outputs
belong outside the source tree and may be published only as evidence on the
results branch. Use these pairs:

| Scene recipe | Runtime request | Lighting |
| --- | --- | --- |
| `tumblers.json` | `tumblers-textures-render.json` | Three finite studio panels and constant fill |
| `tumblers-sun.json` | `tumblers-sun-textures-render.json` | Finite sun through actual window/blinds and original procedural sky |
| `tumblers-sun.json` | `tumblers-hdr-textures-render.json` | Same finite sun and authored RGBE sky fixture |

These requested settings are not a completed visual milestone. The requests use
1920x1080, 2048 samples, depth16, seed1948, one final
`aces_fitted` output transform, 5.5mm aperture radius and 670mm axial focus.
The two closed glass tumblers explicitly enable `shadow_transmission`. Their
matching `*-blocked-render.json` controls differ only by setting those two flags
false. This isolates the optional straight shadow connection; it is not a
refracted-caustic solver. Physical camera/reflection/refraction paths remain active.

Thirteen opaque body bindings add the original walnut and woven-linen colour and
OpenGL normal maps. Their 26MiB normalized pixels are below the32MiB texture cap.
Mapping uses explicit planar world frames; the normal maps do not model yarn
geometry and the side faces are not triplanar. Glass stays smooth and unmapped.
See [texture provenance](textures/ATTRIBUTION.md) and `textures/manifest.json`.

The locally generated HDR file contains only an original analytic gradient, sampled at512x256 and
quantized to Radiance RGBE with declared linear-sRGB/D65 primaries. It contains no
sun disk or downloaded imagery. The separate finite sun remains identical between
the procedural-sky and HDR requests. RGBE quantization means these two inputs need
not produce byte-identical renders. `textures/environment-manifest.json` records
its hash, dimensions and authoring parameters.

Regenerate the original room with `python examples/studio/author_tumblers.py` and
the textures/HDR/portable requests with `python examples/studio/author_showcase.py`.
These scripts use Python's standard library. They do not modify the coffee or
stage1 recipes. Source validation and asset hashes do not constitute GPU evidence:
the final report must include the actual backend, rebuilt source, full-resolution
image, metadata and matched blocked-shadow control.
