# Closer, lower coffee framing variant

This is a separate original composition for review. The baseline scene and render
request remain unchanged. Use `coffee-framing-sweep.json` with
`coffee-framing-render.json`; `coffee-framing.json` is the flat-floor counterpart.
Run `python examples/studio/author_coffee_framing.py` to regenerate all three.

The camera moves from 1200 mm / 30 degrees elevation to 950 mm / 18 degrees,
with 30 degrees front-left yaw, a 34-degree vertical field of view and target
[-25, -10, 170] mm. The recipe appends 27 named rigid translations with `copy:false`.
It preserves the original 32 bodies, their names and their order: there are no new
parts or replacement geometry. Replay and `render.scene` must verify this before
using the existing named body-index overrides.

The tank moves 100 mm farther right; its lid is also lifted 22 mm. The bin moves
70 mm left and the service panel 110 mm left. The head/collar/screen rise 28 mm,
while the roof and lever rise 40 mm. The tray moves 34 mm forward; the detached
cap, screws, cable and plug spread modestly with it. Existing U lever and real
perforated grate remain intact. All physical glass, coat, sample and corrected
light settings are copied from the baseline request without adjustment.

Projecting all corners of the original uploaded product-body bounds through the
new camera gives conservative normalized bounds X [-0.596, 0.568], Y [-0.849,
0.916], excluding the cyclorama. This occupies approximately 58% of frame width
and 88% of height, versus 36% and 63% for the baseline. The nearest product margin
is about 4% of image height. These are source-level framing checks, not a rendered
quality claim: actual mesh replay, body-order verification, physical-glass render
and visual review remain required. The deliberately broad cyclorama continues
outside the frame.
