# Sheet metal

MODEL → MODIFY offers Convert to Sheet Metal. The contextual SHEET METAL tab appears
when the active body is a sheet, and builds parts from a sheet of constant thickness under a **sheet metal rule**
(thickness, bend radius, K-factor), set in **Sheet Metal Rules** (`sheet.manage_rules`).

- **Flange** (`sheet.flange`): a base flange from a sketch profile, a contour flange from an open
  sketch, or an edge flange from a part edge.
- **Hem** (`sheet.hem`) and **Fold** (`sheet.fold`) along a sketch line.
- **Unfold / Refold** (`sheet.unfold`, `sheet.refold`) for working on the flat part.
- **Convert** (`sheet.convert`) turns a plate of constant thickness into sheet metal, or, with
  bend edges, a solid or a shelled body.
- **Create Flat Pattern** (`sheet.flat_pattern`) shows the flat part with its bend lines and a
  bend table, and exports it as DXF or a bend-table CSV through the save picker.

Bends follow the rule's K-factor, so flat lengths match what a press brake will make. Tests
check flat lengths against hand-computed bend allowances.

An edge flange takes its setback off its own edge only. Where that edge ends at an inside
corner of the base (the base runs on past it, like the leg of an L), the rest of the base
stays, and the rule's **bend relief** (`relief_width` and `relief_depth`; Relief Width and
Relief Depth in Sheet Metal Rules) is cut into it beside the bend's end: a slot the relief
width wide, from the relief depth before the bend line to the relief width past the flange's
far edge in the flat pattern. The bend and the flange are then free of the base, folded and
flat. A relief that can't be cut (a width under 0.01 mm, one as wide as the base beside the
bend, or a slot that would cut the base apart) is left out, and the flange feature reports a
warning saying where.

Where a cut-out has taken part of an edge away (a corner cut off the plate, a notch in the
middle of an edge), an edge flange picked on that edge runs along the stretch the cut-outs
leave nearest the pick, and takes its setback off that stretch only. The base on the other
side of a notch keeps its edge, and a pick there, in the same feature or a later one, makes
a flange of its own. Where the base is still left beside the bend's end at such a cut-out, it
gets the bend relief as above. An edge that cut-outs have taken whole is refused.

The setback is cut out of the base's outline as it was drawn: where the bend line crosses an
arc of the outline (a plate with rounded corners, bent inside or in the middle), the arc stays
an arc and ends there, in the flat pattern and the DXF. A free-form curve (an ellipse, a
spline) in an outline that a setback cuts is followed by tangent arcs within 0.001 mm.

Where two bends of one edge flange feature meet at a corner of the base, the rule's **corner
relief shape** (`corner_relief_shape`; Corner Relief Shape in Sheet Metal Rules) says what is
cut there. `notch`, the default, cuts nothing more: the bends end square at the corner, which
leaves a step in the flat pattern between each bend's end and its flange's closed end. `tear`
cuts no relief either, and no step: both bends run to the corner where the two bend lines
meet, and each bend's end is cut straight, in the flat pattern, from that corner to where its
flange's closed end starts. Folded, the two bends touch at the corner and open to the rule's
gap where the flanges start; the bend table lists a torn bend between the middles of its two
ends. A tear whose cuts would cross in the flat pattern, or that would run one bend into the
other folded (flanges at very different angles), is left out with a warning and the corner
keeps its notch. `round`
and `rectangular` cut a relief the size of the rule's **corner relief** across, centred on the corner where
the two bend lines meet: out of the base a round with half that size as its radius, or a square
with half that size as its side, and both bends stop where it begins, cut straight across. The
flange panels keep their ends. The relief is in the folded part, the flat pattern, the bend
table (the bends are shorter) and the DXF (a round relief is an arc). At a corner that isn't
square the round follows the angle of the base and the square becomes a kite, its two cuts
square to the edges. Changing the shape in a rule changes the parts made with that rule. A
relief that doesn't fit (under 0.01 mm across, leaving nothing of a bend, or reaching as far
from its corner as another edge of the base is) is left out, and the flange feature reports a
warning saying where.

```json
{"command":"sheet.manage_rules","params":{"name":"Sheet 2 mm","thickness":"2 mm","k_factor":0.5,
 "bend_radius":"1 mm","corner_relief_shape":"round","corner_relief":"8 mm","active":true}}
```

The rule's shape and size are the default. An edge flange feature can choose its own for the
corners its flanges make: `corner_relief_shape` (`notch`, `round`, `rectangular` or `tear`) and
`corner_relief` (the size across, a length or an expression) in `sheet.flange`, or Corner Relief
Shape and Corner Relief in the Flange dialog. In the dialog, Rule as the shape and an empty size
take the rule's; either one left out follows the rule while the other is the feature's. Corners are
relieved only between the flanges of one feature, so a feature's choice never meets another's at a
corner, and two edge flange features of one part can have different reliefs under one rule. A
base or contour flange takes its corners from the rule only and refuses both parameters. The
flange gap and the rule's other values stay with the rule for every flange. The feature's choice is
saved with the design (file format 23).

```json
{"command":"sheet.flange","params":{"edges":[[60,0,2],[120,45,2],[60,90,2],[0,45,2]],
 "height":20,"corner_relief_shape":"round","corner_relief":"6 mm"}}
```

Where the edge flanges of one feature meet at a corner of the base, their panels are closed
toward each other at any bend angle: each end is cut square to the sheet, on a line in the flat
chosen so that the edges nearest each other (on the inner faces) run parallel, the rule's
**gap** apart, either side of the plane halfway between the flanges. Flanges at 72° get slanted
ends in the flat; corners that aren't square close the same way, and at 90° it is the corner
the rule always gave. A corner that can't be closed (a gap under 0.01 mm, flanges bending
opposite ways, ends that would overlap in the flat pattern) stays open, and the flange feature
reports a warning saying where. A flange added by a later feature leaves the corner as it was:
the earlier flange keeps its length and the new one ends at its bend.

The flanges of one edge flange feature can each have an angle and a height of their own. An
entry of `edges` is a point `[x, y, z]`, which takes the feature's `height` and `angle`, or an
object `{"point": [x, y, z], "angle": "105 deg", "height": "30 mm"}` whose `angle` and `height`
(either one, or both) replace them for that edge. They are expressions like the feature's own,
so parameters work, and each is a feature input of its own (`Edge angle`, `Edge height`) after
`Height` and `Angle`. The corners between such flanges close like those between equal ones:
the ends run toward each other the rule's gap apart, either side of the plane halfway between
the two flanges, and a taller flange's end keeps that line up to its own tip. This is how to
close a corner between flanges at different angles, because flanges of separate features
don't close toward each other. In the Flange dialog, picking more than one edge shows a row
per edge with an angle and a height box; an empty box takes the value above.

```json
{"command":"sheet.flange","params":{"edges":[[60,0,2],{"point":[120,45,2],"angle":"90 deg"},
 {"point":[60,90,2],"angle":"105 deg","height":"30 mm"},{"point":[0,45,2],"angle":"120 deg","height":"25 mm"}],
 "height":38.6,"angle":"75 deg"}}
```

A flange can also stand on the far edge of another flange: a return, such as a lip turned in
along the top of a tray's walls. It runs along that edge from end to end, so it follows the
closed end of the flange it stands on, slanted or not. Two returns of one feature whose flanges
meet at a corner of the base are closed toward each other as well, with the rule's gap. They
are mitred either side of a plane that runs along the mean of the two panels' normals, bisects
their bend lines and passes where the lines their panels start on cross. Returns that lie in
one plane (level returns on a tray's walls, whatever the walls' angle) then meet on the corner's
bisector, like the sides of a picture frame. Each panel ends on a slanted line in the flat.
Each bend stops square where all of it keeps half the gap from that plane, at the latest at
the end of the flange it stands on, which can leave a short stretch of that flange's edge bare.
Returns turned opposite ways pass each other. A return that meets no other at a corner ends
with the flange it stands on; where the flange beside that one leans in over it, the return is
cut back until it keeps as clear of that flange as the end of the flange it stands on does.
Returns that can't be closed stay open with a warning, as on the base. Returns of separate
features don't close toward each other either: make the returns round a corner in one feature.
A corner relief or a tear changes none of this: it leaves the flanges' closed ends as they
are, so the returns on them close over a relieved or torn corner as over a whole one.

```json
{"command":"sheet.flange","params":{"edges":[[60,2,20],[118,45,20],[60,88,20],[2,45,20]],
 "height":12,"angle":"90 deg"}}
```

(The four returns of a 120 x 90 tray in 2 mm sheet with 90° walls 20 high: each point is the
middle of a wall's far edge on its inner face, so the returns turn in over the base.)

**Convert** also takes a solid or a shelled body: pick the face that stays fixed and the sharp
edges that become bends (`bends`; Bend Edges in the dialog). The flat faces reached from the
fixed face across those edges become the sheet. A shelled body keeps its walls and their
thickness; any other solid's faces become the outside of the sheet, with the rule's thickness
taken into the body. Each sharp edge stays where the faces of its bend would meet, so the part
keeps its envelope and the flat pattern is the outside lengths less a bend deduction per bend,
2·(R + T)·tan(θ/2) − θ·(R + K·T). A bend can be picked on either skin of a shell; bends turn
either way, follow one another and take any angle (the walls of a tapered tray at 72°). An
edge between two faces of the sheet that is no bend is cut open, each side half the rule's
**gap** from the plane halfway between the two, like the corners of a tray. A bend drawn as a
chamfer (a flat strip between two faces, the chord of the bend's arc, as models without bend
radii draw one) is picked in the middle of the strip: the two faces then meet at their virtual
sharp in one bend of the rule's radius, and the strip is no face of the sheet. A bend modelled
with its radius (a cylinder between two flat faces tangent to it, as sheet metal parts from
other systems come, or a filleted edge) is picked anywhere on its cylinder, on either skin, and
converts the same way: the sheet is bent there with the rule's radius and K-factor, not the
modelled radius, so the flat pattern is the one the rule makes. The result is an
ordinary sheet body: edge flanges, cuts, Unfold, the flat pattern and its exports work on it,
and it follows the solid when a parameter changes (the edges and the fixed face are found by
their names). Without bend edges Convert expects a plate, as before. Either way round walls
(holes, slot ends, rounded corners, notches) stay circles and arcs in the flat pattern and the
DXF, also where a STEP file writes one in pieces, and the holes and slots of every face of the
sheet are its cut-outs, where the bends put them. An engraving in a face of the sheet (a recess
with a flat floor, at most half the thickness deep) becomes a pocket of it, as a plate's does
(below), on either skin of a shell. Not converted yet: curved faces other than bends (rolled
sheet), with bend edges a hole or an engraving that reaches into a bend or a pocket deeper than
half the thickness (cut them after converting), and bends that close a ring.

A plate may carry recesses in one of its faces, such as an engraved label or a counterbore: a
recess with a flat floor and walls square to the plate, at most half the plate's thickness
deep. Each becomes a pocket of the sheet (see the pockets below), so the converted part keeps
it: the sheet is as thick as the plate, the flat pattern is the whole plate with the recess
listed under `pockets` (its outline, side and depth), and the DXF draws its outline on the
pocket layer of its depth, apart from the cut contours, for a laser to mark rather than cut.
The face the recesses are in can be the one picked or the other. A recess deeper than half the
thickness, recesses in both faces, and a plate whose recesses are larger than the face around
them (a thinner plate with raised parts) are refused with the reason. A recess lies in an
opening of the face it is cut into, so the faces of a bent part's legs are none: a bent part is
refused as no plate, and converts with its bend edges. A bend across a pocket is refused as for
any pocket, so fold or flange clear of an engraving.

```json
{"command":"solid.box","params":{"length":100,"width":60,"height":30,"body_name":"Tray"}}
{"command":"solid.shell","params":{"faces":[[50,30,30]],"thickness":2}}
{"command":"sheet.convert","params":{"body":"Tray","face":[50,30,0],
 "bends":[[50,0,0],[100,30,0],[50,60,0],[0,30,0]]}}
```

The `face` can also be the name `model.faces` lists for it (`"face":"F1:+z"`), which saves
computing a point on a face with holes or a curved outline: `model.faces` gives every face a
`point` that lies on it.

For a shop drawing or spreadsheet, `sheet.export_bend_table` returns CSV by default; choose
`format: "json"` for structured data. It includes the sheet rule, thickness, bend angles,
radii, allowances and flat centre-line coordinates. Lengths use millimetres and angles use
degrees. A bend is listed where its centre line has material: one that a cut-out has shortened
gives the length and ends it has left, and one that cut-outs split gives a row for every piece,
all with the bend's number (the flat pattern and the DXF draw the same pieces as bend lines).
Supply `path` to save it, or omit `path` to receive the content through the command
response. `body` defaults to the last sheet body. Exporting does not add a timeline entry or
consume an undo step.

```json
{"command":"sheet.export_bend_table","params":{"format":"csv","path":"bracket-bends.csv"}}
{"command":"sheet.export_dxf","params":{"path":"bracket-flat.dxf"}}
```

The DXF is an AutoCAD 2000 (`AC1015`) drawing in millimetres, made for a laser-cutting shop's
nesting or CAM software: each kind of line is on a layer of its own, with a colour (AutoCAD's
index) and, for the bends, a linetype that the file defines.

| Layer | Colour | Line | Holds |
|---|---|---|---|
| `OUTLINE` | 7 white or black | continuous | the outer contour of each piece of the sheet |
| `CUT` | 4 cyan | continuous | what is cut out inside it: holes, slots, and the loose pieces a cut leaves (the middle of a ring) |
| `TEXT` | 2 yellow | continuous | the letters that text cut through the sheet, with their counters; only with `"text": "cut"`, the default |
| `ETCH` | 6 magenta | continuous | letters as single contours, to be marked and not cut: `"text": "etch"`, and always text machined to a depth |
| `BEND_UP`, `BEND_DOWN` | 1 red, 5 blue | `DASHED` | the bend centre lines, by direction |
| `BEND_NOTES` | 8 grey | continuous | the note beside each bend line, only with `"bend_notes": true` |
| `POCKET_…` | 3 green | continuous | pockets, a layer for each depth (see below) |
| `PERFORATION` | 3 green | continuous | the holes of a perforation field |

`OUTLINE`, `CUT`, `BEND_UP` and `BEND_DOWN` are always in the layer table, the others when the
sheet has something for them. The inner contours (`CUT`, `TEXT`) are written before the outline,
so a program that follows the file's order cuts inside-out.

A round hole is a `CIRCLE`, a circular arc an `ARC`, a whole ellipse one `ELLIPSE` and an arc of
an ellipse an elliptical `ELLIPSE`; the pieces of a spline, the outlines of text and the other
conics are `SPLINE`s of their own degree, so no curve is written as chords. For a machine that
reads only lines and arcs, pass `"curves": "arcs"` to `sheet.export_dxf`: the free-form curves
are then tangent arcs and lines within 0.001 mm.

Letters are told from other cut-outs by where they came from: a cut whose sketch holds text
(Sketch Text) marks the cut-outs it makes as letters, counters included, while the design is
built; nothing more is saved in the design file. A letter that another cut-out overlaps, or
that the sheet's edge clips, has become one shape with it and stays on `CUT`.

```json
{"command":"sheet.export_dxf","params":{"path":"tray-flat.dxf","text":"etch","bend_notes":true}}
```

With `"bend_notes": true` each bend line gets a note on `BEND_NOTES`, `B1 UP 90° R1` (the bend's
number, its direction, its angle and its inner radius; `%%d` is AutoCAD's degree sign). It is 3 mm
high (`"note_height"` changes that), centred half a height beside the middle of the line,
parallel to it and readable from below or from the right. A bend that cut-outs split has a note
for each piece. Notes are off by default, because a program that cuts or marks every layer would
mark them.

A design with several sheet bodies exports one file for each with `"per_body": true`: the files
are `<path>-<body>.dxf` (characters a file name cannot hold become `_`; two bodies that would
share a name get `_2`, `_3`…), and the reply lists them in `files`. Without a `path` it lists
their DXF texts instead.

Base-flange profiles keep their interior holes. Through cuts that cross a sheet edge become
notches in the flat outline; overlapping and repeated cuts remove their combined area once.
The flat preview, DXF and unfolded solid use the same material boundaries. Lines and circular
arcs remain exact. An ellipse, a spline or text stays the curve it was drawn as in the flat
pattern and the unfolded solid; the folded solid follows it with tangent arcs within 0.001 mm,
and the point lists write it as chords within 0.001 mm (the DXF keeps the curve, see above).
Flat-pattern JSON lists each cut-out twice, in the same order: as points (`cutouts`), and
exactly (`cutout_shapes`): a round hole as `{"circle": {"center", "radius"}}`, any other as its
`line`, `arc` (centre, radius, start and sweep in degrees) and `polyline` segments (one
`polyline` for each ellipse quarter, spline or glyph curve).

Cuts can leave separate material pieces, including the centre of a ring cut. These pieces
remain one logical sheet group through Unfold / Refold and export as separate flat outlines.
A cut that parts a piece from the base (a flange cut off along its bend, the part of a ring's
inside on a flange's edge) keeps it too, folded or flat, and its warning says how many pieces
are no longer joined to the base, their area and where they lie in the flat pattern. Islands
inside other material (a ring's centre, a letter's counter) are not counted.
Large profiles that exceed the geometry processing budget report an error instead of
exporting a partial outline.

A Box cut, or a Cylinder cut whose axis is the sheet's normal, machines the sheet like the
extrude of its footprint: through the sheet it is a cut-out (a cylinder a round one), and
part way it is a pocket. A primitive cut at any other angle, or across a folded bend, is refused.

Hole machines the sheet the same way from a flat face of any panel, the base or a flange, on
either side, folded or unfolded: square to that panel, a through hole (plain or tapped) is a
round cut-out of the flat pattern and a `CIRCLE` in the DXF, and a counterbore or a blind
flat-bottomed hole is a round pocket. A hole slanted to its panel or drilled into the edge of
the sheet, a countersink and a drill point are refused.

Parallel, untapered extrude cuts also preserve shallow pockets through Unfold / Refold.
The pocket floor and side walls remain solid geometry; overlapping cuts from opposite sides
become a through hole wherever they remove the whole thickness. Flat-pattern JSON lists
`pockets` with their side, depth and bottom/top thickness coordinates in millimetres.
DXF pocket boundaries use `POCKET_BOTTOM_Z…_TO_…_MM`, `POCKET_TOP_Z…_TO_…_MM`, or
`POCKET_INTERNAL_Z…_TO_…_MM` layers. These are machining operations, so do not treat every
pocket layer as a through-cut contour when importing the drawing into a CAM tool. (Text
machined to a depth is a marking, on `ETCH`.)

Extrude Intersect on a straight base or an unfolded sheet retains the material inside both
the current tool footprint and its depth interval, including interior holes, detached islands
and earlier pockets. A shallow intersection records the removed layers above and below the
retained material as top and bottom pockets. The sheet rule keeps the original stock gauge;
the remaining solid can be thinner. A completely empty intersection removes the sheet and
its metadata together. DXF records pocket depth in its layer names; importing that DXF as
ordinary sketch geometry does not reconstruct the machining depths automatically. Each
positive retained or removed depth layer must meet the kernel's 0.000001 mm extrusion
resolution; smaller layers report an error rather than being rounded away. To Object can
retain a tool that trims to a straight prism, using its actual trimmed footprint. A curved
tool with varying depth is refused before changing sheet geometry. Pocket profiles can move with planar panels when forming,
but a pocket crossing a bend zone cannot yet be refolded. Machining folded bend material,
tapered sheet cuts and other unsupported 3D sheet Cut/Intersect operations report an error before changing
the design; they cannot silently restore removed material during a later sheet rebuild.

A sketch can be drawn on a flat face of the sheet: the base or a flange, on either side. Pick
the face (or give `sketch.create` the point `plane: {face: [x, y, z]}`) and the sketch attaches
to that panel instead of to a certified source face. It keeps the face's name,
`sheet:<body>:<panel>:<top|bottom>` (the panel is `base` or the flange's feature tag), and finds
the face again whenever the sheet is rebuilt: after a parameter change such as the thickness or
a flange angle, through Unfold / Refold, after the file is reopened and when the body is
renamed. Sketch coordinates are the flat pattern's, so a point drawn on a flange lies where the
flat pattern and its DXF have it, and on the bottom face y runs the other way, as seen from
below. When the face is gone (its flange is suppressed or deleted) the sketch reports a lost
attachment instead of moving to another face; **Redefine Sketch Plane** attaches it to another
face. `fixed: true` still freezes the picked plane in space.
