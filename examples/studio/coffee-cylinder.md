# Cylindrical coffee housing variant

Use `coffee-cylinder-sweep.json` with `coffee-cylinder-render.json` for the
continuous CAD cyclorama. `coffee-cylinder.json` is the flat-floor counterpart.
These are separate original variants of the closer, lower coffee framing.
Regenerate with `python examples/studio/author_coffee_cylinder.py` after placing
the framing recipes beside the authoring tool.

Three existing bodies receive cylindrical CAD construction. The plinth is an
R76 mm cylinder at [0, 8, 30], 12 mm high. The tall housing has R70 mm outside
and R65 mm inside, from Z55 to Z265. Its through-cavity cutter extends 1 mm past
both ends to avoid coincident boolean caps. The round shoulder is R70 mm, from
Z265 to Z273. All three share the same XY axis at [0, 8]. The visible outer skin
is an analytic cylinder with a continuous broad curved silhouette.

The open brewing bay removes X[-47,47], Y[-80,0], Z[88,237] mm from the annular
shell. It opens the front wall while retaining side cheeks, a 33 mm lower band
and a 28 mm upper band. There are no new decorative parts. The separate head,
bent chrome lever and ball ends, tank, ribbed bin, perforated grate, service
panel, screws, cable and plug retain the framing recipe's geometry and moves.

The expected final body list remains the 32 names in the `coffee-exploded`
entry of `materials.json`, in exactly that order. Replacing the three original
sketch/extrude blocks creates no temporary additional bodies. The seven coating
overrides and two closed-mesh dielectric overrides retain their named ordinals.
The render request is copied byte-for-byte from `coffee-framing-render.json`,
including the corrected fill, 1920x1080 camera, 256 samples and depth 12.

This is independently authored SolidCraft geometry under MIT OR Apache-2.0.
No reference models, image assets, logos, product-specific detailing or triangle
soups are imported. Dimensions describe an original generic appliance study.

CAD replay, actual `render.scene` body-order checks, material validation and
rendered visual review are pending. Source command inspection alone does not
establish successful booleans or the quality of the glossy cylindrical result.
