# SolidCraft UI icon specification

This is the binding specification for new and revised icons. Use it with the
actual SVG references in `design/references.toml`; `http://127.0.0.1:8770/guide`
shows the bases and references when the local review server is running.
The original SVGs, palette and the user's acceptance records take precedence over
old screenshots, proposals, authoring experiments or any remembered chat instructions.

## First choose the category

| Category | Shape language | Finished references |
| --- | --- | --- |
| Solid / modelling feature | One substantial isometric volume; grey reference geometry, blue change/result | `model/box.svg`, `extrude.svg`, `hole.svg`, `rib.svg`, `contact_sets.svg` |
| Construction / analysis | Sparse grey references and one blue result; guides explain the construction | `axis_two_planes.svg`, `axis_normal_to_face.svg`, `point_three_planes.svg`, `plane_midplane.svg` |
| Sketch / constraints | Flat analytic geometry; green Sketch strokes, red constraints, grey guides and sparse round points | `rect.svg`, `circle.svg`, `polygon_circumscribed.svg`, `slot.svg`, `sketch_fillet.svg`, `c_parallel.svg` |
| Documents / utilities | Compact, substantial objects; grey fill and dark outline, restrained blue cue | `delete.svg`, `copy.svg`, `paste.svg`, `library.svg`, `new.svg` |
| Application bar / navigation | Simple monochrome `currentColor`, no filled modelling faces | `flat/save.svg`, `undo.svg`, `orbit.svg`, `fit.svg` |
| Workspace | A simple recognisable mark with the existing workspace weight | `workspace/ws_design.svg`, `ws_animate.svg` |

Unqualified references in the table are in `design/svg/model/`. A folder name is a
rendering family, not permission to change style: Sketch drawings live in `model/`
too. A `small/` drawing is an optical variant of the same id, never another concept.
Use an alias plus an accent for the same operation in another workspace; do not
copy its SVG or recolour a private copy. Do not add a filled cube to a flat control.

## Exact common measurements

- Coordinate system: `viewBox="0 0 64 64"`; no intrinsic SVG width/height.
- Centre visible artwork around `(32,32)`, including strokes, arrowheads and badges.
  Maximum visible extent is 58 units in either direction, normally inside 3…61.
  Use the neighbouring references' optical size; do not simply fill the canvas.
- Geometry outline: **2.55**; action / Sketch stroke: **4.25**; navigation: **5.6**.
  Parameters retain their existing **4.2** weight. These values are in `palette.toml`.
  Sparse guides may use 2.0–2.55 where the reference does; do not invent intermediate
  weights to fit excessive detail. Every stroke has round caps and round joins.
- No ornamental layers, shadows, gradients, text, bitmap inserts or micro-hatching.
  Aim for one base object plus one result/action cue. Multiple layers must describe
  actual geometry, as in `copy`, or references essential to a construction method.
- Curves must be smooth at joins. Use analytic ellipses/arcs or matched Bézier
  tangents. Do not approximate a round face with a visibly irregular polygon.

## Canonical grey cube — do not redraw it by eye

`design/tools/author/bases.py::grey_cube(icon)` is the exact common base.
`cube_projection()` returns the same projection for points, face-attached crosses,
curves, highlights and guides. `lib.py::Iso` and `Icon` are the low-level helpers.

The physical cube is x/y = −12…12, z = 0…24. Default scale is 1.125. Projection:

```text
screen_x = 32 + (x-y) * sqrt(3)/2 * 1.125
screen_y = 32 + (x+y)/2 * 1.125 - (z-12) * 1.125
```

| Vertex | Screen coordinates |
| --- | --- |
| back top | (32, 5) |
| left top | (8.62, 18.5) |
| front top | (32, 32) |
| right top | (55.38, 18.5) |
| left bottom | (8.62, 45.5) |
| front bottom | (32, 59) |
| right bottom | (55.38, 45.5) |

Axes are x right/down, y left/down, z straight up. Diagonals have **30°** slopes;
equal physical dimensions remain equal. Screen-left face is y=+12; screen-right
face is x=+12. Default passive fills are **top P1, left P2, right P3**. Never swap
them to make an individual drawing prettier. Do not use perspective convergence.
`grey_slab()` uses the same axes and shading with a four-unit thickness. Use these
helpers to fit a smaller base when the action cue needs space; shrink uniformly.

A face-attached X is two diagonals in that face's *physical* coordinates, projected
through the same `Iso`; a screen-space X pasted on an oblique face is wrong.
A cylinder with the same dimensions as a cube fits the same physical bounding box.
Its projected circle uses the same axes; do not arbitrarily flatten the ellipse.

## Colour and semantic roles

Only use the stable `source` identifiers below in model SVGs. The renderer maps roles to
light/dark theme values. `currentColor` is the background line-art role, not a way
to add arbitrary colours. All values and accents come from `design/palette.toml`.

| Role | Author helper | Light value | Use |
| --- | --- | --- | --- |
| Ink | INK | #34465b | Object contours and details over fills |
| Passive 1 | P1 | #e2e8ef | Top / light grey reference face |
| Passive 2 | P2 | #b8c3cf | Screen-left grey face |
| Passive 3 | P3 | #8e9faf | Screen-right grey face |
| Active 1 | A1 | #b9ddf8 | Pale result face |
| Active 2 | A2 | #328bd3 | Medium result face |
| Active 3 | A3 | #236ead | Strong blue action or result stroke |
| Guide | GUIDE | #6f879c | Sparse construction references |
| Gap | GAP | #ffffff | Erase earlier artwork to real transparency |

The source hexes identify paint roles, not the displayed colour. **Never recolour an
SVG to apply a workspace accent.** Keep these identifiers and every finished shape
unchanged; the runtime and review page resolve them through `design/palette.toml`.

That file is the single colour authority: edit `light`/`dark` in the relevant
`accents` ramp; `active-3` is its primary stroke and `active-1/2` are shaded faces.
Do not edit a role's `source` identifier. `variants` holds colour-vision adjustments.
A role with `use = "active-3"` follows that stroke automatically.

| Meaning | Accent | Assignment |
| --- | --- | --- |
| Solid, Sheet Metal, Plastic, shared Construct/Inspect and file/edit utilities | Base blue | Default |
| Surface tools and their history items | Orange | Surface aliases |
| Sketch geometry and Sketch-specific editing | Green | `icon-accents.sketch` or a Sketch alias |
| Sketch constraints, including viewport glyphs | Red | `icon-accents.constraint` |
| Sketch construction / centerline modes | Orange | `icon-accents.construction` |
| Mesh-specific actions | Yellow | Mesh aliases |
| Future Form workspace | Purple | Form aliases |

Passive faces, outlines, reference guides, knockouts and monochrome navigation stay
in their original roles. Non-blue dotted or dashed reference lines use **Guides**.
Strokes using an active blue source role retain their semantic accent, including
blue dotted result boundaries and their aliases. A wider knockout beneath such a
line erases earlier artwork. Draw manual reference dashes and dots with GUIDE.
Construct planes/axes/points remain blue. The active workspace never
recolours shared tools. Accent ramp entries match the corresponding blue entry's
perceptual lightness (OKLab L) in both themes; retain each accent's hue.
Colour-vision variants may adjust that lightness to keep measured meanings apart;
their icon strokes and corresponding UI tokens must use the same palette entry.

Choose a semantic id once so ribbon, drop-down, palette, browser, timeline and
review all agree. An explicit alias accent wins over an id's default. Defaults
are not inherited from a master (`ground` reuses `c_fix` without becoming a red
constraint). Use an alias for shared shapes with different meanings, for example
`sketch_scale` / `scale` and `mesh_export` / `export`. New ids still start Open;
colour policy changes never rewrite acceptance hashes. Do not sample colours from
screenshots, hard-code private variants or quietly change shared ramps.

## Arrows, overlap and detail — the frequent failure points

- Arrows follow the geometry: rotation arcs follow the cylinder's roundness using
  its actual projected ellipse. Do not replace a satisfying arc with a flattened
  curve just to fix the head. Preserve the shaft and correct the tangent/head.
- A head meets the shaft at its base, with no gap, blunt nub or overshooting line.
  Its direction follows the curve's final tangent. `bases.arrow` supplies the
  straight version. Standard small heads are around 8–10 units long/wide.
- A knockout around an overlaid arrow/X is normally about 1–1.2 units per side.
  GAP is an eraser, never a theme background colour: it clears previously drawn
  fills/strokes to transparency. Draw the coloured foreground after the knockout.
  Existing binary masks use literal `white`/`black` for coverage, not GAP.
  Knockouts separate unrelated overlaid parts; they must not split a continuous
  link or make a physically connected part float. Wider borders need a clear
  semantic reason, as in the infinity symbol.
- Compute visibility before outlining. Draw distant opaque faces first, then near
  faces. A hook must cover both the stem's hidden fill and its hidden edges. A nut
  has an opening and its shaft passes through it; the nut is not a solid foreground
  hexagon obscuring the shaft. Never fix an occlusion error with a stray patch.
- Face removal: the missing face is unfilled. Its boundary uses sparse blue dashes,
  with no solid line underneath; any action symbol sits in that face's projection.
- Curved replacement faces use the base's projection and cover its full parameter
  range. Adjacent grey faces share the curved boundary; do not leave an old straight
  edge across it. Extend a surface slightly and consistently beyond all intended edges.
- Use round Sketch points sparingly: regular points are about radius 3–3.4. Construction
  reference points should stay subordinate to the result. Do not mark every sample.
- Dashes and dots must have even spacing, including the closing seam. Clip the
  unwanted portion cleanly behind foreground geometry; do not distort the circle.
- Utility icons follow `delete`, `copy` and `paste` proportions and roundness. Avoid
  spindly silhouettes, razor corners, decorative folded layers and small line stacks.
- Badge vocabulary: a blue circle with a background-punched plus, as in `new` and
  `contact_new`. Maintain the main object's silhouette and a clear small-size gap.

## SVG subset and source ownership

The application parser supports `path`, `circle`, `g`, `defs`, `clipPath`, `mask`
and `title`. Use paths for ellipses (`lib.ell`); native `<ellipse>` is unsupported.
Paths use absolute M/L/C/Q/A/Z. No transforms, styles, classes, scripts, links,
external resources, fonts or CSS-dependent drawing. Use explicit palette paints.
Keep clip ids unique inside each SVG. One id has one primary master; `small/` may
supply an optical variant. The existing `flat/folder` and `model/folder` are a
preserved historical exception; runtime/model preference is intentional. Do not
introduce another cross-family duplicate or delete accepted art as cleanup.

SVGs are editable masters. Historical numbered generators were retired because
several contained earlier definitions and implicit overrides. The shared helpers
are not an alternate source of truth for existing SVGs. Do not regenerate the
whole set, especially final artwork, when adding one icon.

## First-pass workflow and acceptance rubric

1. Read the command/interaction and two closest finished references. Choose a
   distinct concept and the category above. Check final/master/alias protection.
2. Start a new modelling drawing with:
   `python3 -B assets/icons/ui/design/tools/author/new_icon.py my_icon --base cube`.
   Choose `--base slab` or `--base empty` when appropriate. The starter is not a
   completed icon. The tool refuses an existing master/alias and adds attribution.
3. Add the smallest clear operation cue. Wire `.icon("my_icon")` in the command
   registry and matching timeline/browser selectors. Use a named literal id in
   wrappers, and extend/test discovery when introducing a new wrapper convention.
4. Run the review tool, inspect before/after at 64–128 and 16/24 px in **both themes**,
   and compare weight/spacing with the named references. At 16 px the operation
   must remain distinct. Add a small variant only if simplification is necessary.
5. Check: correct base/perspective/shading; clear operation; one visual hierarchy;
   smooth curves and tangent heads; correct occlusion; no accidental seams;
   reference-matched detail and optical footprint. Fix these before owner review.
6. Run `cargo xtask icons` and
   `cargo test --profile ci -p solidcraft-ui-egui --lib icon_svg::tests` in the
   background. Run normal repository gates before committing. Never claim visual
   quality from lint alone. Keep review evidence under ignored target/.
7. Leave the icon Open. The owner uses the review page to finalise the exact
   artwork revision. Do not add acceptance hashes or copy another icon's hash.

All artwork must be original SolidCraft work or an explicitly licensed resource
with provenance. Commercial CAD artwork, screenshots and extracted assets are
forbidden even as hidden references. This guide and the local finished set provide
all required visual context for future agents.
