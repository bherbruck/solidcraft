# SolidCraft command reference for agents

These are the shapes of the commands a part build needs, so an agent rarely has to call `list_commands`. The worked example at the end runs in CI (`skill_reference_example_builds_as_documented` in `crates/mcp/src/tests.rs`).

- Lengths are in mm.
- Angles inside expressions take units: `"30 deg"`.
- Any numeric field can be an expression that names parameters: `"w/2 - wall - 6"`.

## Sketch planes: where sketch (x, y) lands in the world

| `plane` | sketch x | sketch y | normal (+distance) |
|---|---|---|---|
| `"XY"` | +X | +Y | +Z |
| `"XZ"` | +X | **-Z** | +Y |
| `"YZ"` | +Y | +Z | +X |
| `{origin, x_dir, y_dir}` | x_dir | y_dir | x_dir x y_dir |

- `offset: "expr"` on `sketch.create` moves the plane along its normal and follows parameters.
- **Front wall (-Y) of a part centred on the origin:**
  - Sketch on `{"plane":"XZ","offset":"-d/2"}`.
  - Draw a feature at height z at sketch y = **-z**.
  - A cut with a positive distance goes into the wall.
- **Top face:** sketch on `{"plane":"XY","offset":"h"}`.
- **Hole direction:** `solid.hole {sketch}` drills from the sketch plane along -normal. A hole sketch at z=0 under a part drills into empty space and still reports success, so put the sketch on the top face.

## Ids

- **Numbering:** every creator returns `created: {points, curves, constraints, dimensions}`. Ids are stable and numbered per sketch.
- **Curves:** `l1`, `c1`, `a1`.
- **Points:** `p1`, `l1.start`, `l1.end`, `c1.center`, `a1.center`, and the sketch origin `"origin"`.
- **Rectangles:** `sketch.rectangle.*` also returns `sides {bottom,right,top,left}` and `corners {bottom_left,...}`.
  - `rectangle.center` adds 2 construction diagonals, so each corner joins 3 lines. Its centre point is `p5`.
  - `sketch.fillet {point}` therefore fails on its corners. Use `{a:"l1", b:"l4", radius}` or the 3D `solid.fillet`.
- **Slots:** `sketch.slot.overall` and `sketch.slot.center_to_center` create curves `l1, a1, l2, a2` and points `p1`-`p4`, `a1.center`, `a2.center`.
  - The `length` role of `slot.overall` is the overall length. The arc centres are at ±(length/2 - width/2).
  - After `length` and `width`, 3 dof remain. To remove them, dimension `origin` to `a1.center` horizontally and vertically, and `origin` to `a2.center` vertically. For a slot along Y the solver refuses that last dimension as over-constraining; dimension `a2.center` horizontally instead.
  - `sketch.slot.center_point` creates no centre point. Dimension an arc centre instead.
- **Circles:** `sketch.circle.center` creates `c1` and `c1.center`, and no `p` point.
- **Dimensions** share the `k` counter with constraints.
- **Constraints can add curves:** a constraint about an axis, such as `sketch.constraint.symmetry` with `line:"y"`, adds a construction line and so takes the next `l` id. Read `created` before you count further.
- **Over-constraining** is refused with `that conflicts with the sketch: neither entity can move to satisfy it`. For example, symmetry already makes two circles equal.

## Dimensions

```json
{"command":"sketch.dimension","params":{"entities":["origin","c1.center"],"type":"horizontal","value":"w/2 - wall - 6"}}
{"command":"sketch.dimension","params":{"entities":["c1"],"type":"diameter","value":"6"}}
{"command":"sketch.dimension","params":{"entities":["l1"],"value":"w"}}
```

Distances are unsigned. Draw the geometry on the correct side first, and the dimension keeps it there.

`sketch.draw.dimensioned` creates a shape and its dimensions in one undo step. The dimension roles depend on the creator:

| Creator | Roles |
|---|---|
| `line` (exactly 2 points) | `length`, `angle` |
| `rectangle.*` | `width`, `height` |
| `circle.*` | `radius` or `diameter` |
| `arc.center_point` | `radius`, `sweep` |
| `slot.center_to_center`, `slot.overall` | `length`, `width` |
| `polygon.*` | `radius` |

```json
{"command":"sketch.draw.dimensioned","params":{"command":"sketch.rectangle.center","params":{"center":[0,0],"corner":[35,25]},"dimensions":{"width":"w","height":"d"}}}
```

## Features

| Command | Minimal params | Traps |
|---|---|---|
| `solid.extrude` | `distance` | `profiles` defaults to **every region**, so two concentric circles give a solid disc. Pick regions with `[{"point":[x,y]}]` or `[index]`. Other options: `operation` (new/join/cut/intersect/auto), `direction` (positive/negative/symmetric), `through_all: true`, `start_offset`. A cut that misses reports success. |
| `solid.box` | `length`, `width`, `height` | `center:[x,y,z]` or `corner`. The quickest way to block out a part. |
| `solid.cylinder` | `radius` or `diameter`, `height` | `base` is the centre of the bottom (default origin), `axis` defaults to `Z`. Expressions work in `base`. Always pass `operation`. |
| `sketch.slot.overall` / `sketch.slot.center_to_center` | `p0`, `p1` (overall ends / arc centres), `width` | Ids and dof are in the "Ids" section above. |
| `solid.fillet` / `solid.chamfer` | `edges:[[x,y,z]]`, `radius` / `distance` | The pick snaps to the **nearest** edge, with no distance limit and no error, so check that the face count changed. One pick takes the whole tangent chain: a rounded rim needs 1 point, not 8. |
| `solid.shell` | `faces:[[x,y,z]]`, `thickness` | A point on each face to remove, or its name from `body_topology`. Fillet first, except an edge of a removed face (see Order in `SKILL.md`). |
| `solid.hole` | `position:[x,y,z]` on a face (numbers or length expressions such as `["40", "width/2", "t"]`, which the hole then follows) + `diameter`, or `sketch` (+ `points`) | `clearance:"M3"`. `depth` defaults to through all. `type` is one of simple/drilled/counterbore/countersink. A `countersink` needs `cs_diameter` and `cs_angle`; a `counterbore` needs `cb_diameter` and `cb_depth`. `position` is a fixed point. |
| `solid.pattern.rectangular` | `features`, `dir1`, `count1`, `spacing1` (+ `dir2`, `count2`, `spacing2`) | Spacing takes expressions and follows parameters. |
| `solid.pattern.circular` | `features`, `axis` (`"Z"` or `{origin, dir}`), `count` | `angle` defaults to 360 deg. A full pattern keeps the COM on the axis, so check its radius with a top-view screenshot. |
| `solid.mirror` | `features:[names]`, `plane` (`XY`/`XZ`/`YZ`, a construction plane, or `{origin, normal}` whose origin may use length expressions) | Follows parameters. Mirror and pattern features cannot be mirrored or patterned again. |

- **Names:** name every feature (`"name":"Standoff"`) and keep names unique across sketches and features. `timeline.roll_to {"feature":"Plate"}` picks the first match, which can be the sketch.
- **Counts as parameters:** `parameters.add {"name":"n","expression":"6","unit":""}` creates a unitless parameter, and `"count":"n"` uses it in a pattern.
- **Holes on a parametric bolt circle:**
  1. Cut one hole with `solid.cylinder {"diameter":6.6,"height":"T + 2","base":["bcd/2",0,-1],"operation":"cut","name":"BoltHole"}`.
  2. Repeat it with `solid.pattern.circular {"features":["BoltHole"],"axis":"Z","count":"n"}`.

  Both the radius and the count follow their parameters.
- **Several parameter changes:** use one `batch` of `parameters.change` steps instead of one `set_parameter` call each.
- **Primitive booleans:** a `solid.box` or `solid.cylinder` cut that touches curved faces of the target can fail with `the exact fallback ... takes planar faces only`. Use a sketch profile and an extrude cut instead, for example a slot sketch rather than a box plus 2 cylinders.

## Worked example: parametric plate with 4 counterbored holes (1 batch)

```json
{"commands":[
 {"command":"parameters.add","params":{"name":"L","expression":"80 mm"}},
 {"command":"parameters.add","params":{"name":"W","expression":"50 mm"}},
 {"command":"parameters.add","params":{"name":"T","expression":"6 mm"}},
 {"command":"parameters.add","params":{"name":"inset","expression":"8 mm"}},
 {"command":"sketch.create","params":{"plane":"XY","name":"PlateSketch"}},
 {"command":"sketch.draw.dimensioned","params":{"command":"sketch.rectangle.center","params":{"center":[0,0],"corner":[40,25]},"dimensions":{"width":"L","height":"W"}}},
 {"command":"sketch.finish","params":{}},
 {"command":"solid.extrude","params":{"distance":"T","name":"Plate"}},
 {"command":"solid.fillet","params":{"edges":[[40,25,3],[-40,25,3],[40,-25,3],[-40,-25,3]],"radius":5,"name":"Corners"}},
 {"command":"sketch.create","params":{"plane":"XY","name":"HolePts","offset":"T"}},
 {"command":"sketch.point","params":{"point":[32,17]}},
 {"command":"sketch.dimension","params":{"entities":["origin","p1"],"type":"horizontal","value":"L/2 - inset"}},
 {"command":"sketch.dimension","params":{"entities":["origin","p1"],"type":"vertical","value":"W/2 - inset"}},
 {"command":"sketch.finish","params":{}},
 {"command":"solid.hole","params":{"sketch":"HolePts","diameter":3.4,"type":"counterbore","cb_diameter":6.5,"cb_depth":3.3,"name":"Hole"}},
 {"command":"solid.pattern.rectangular","params":{"features":["Hole"],"dir1":[-1,0,0],"count1":2,"spacing1":"L - 2*inset","dir2":[0,-1,0],"count2":2,"spacing2":"W - 2*inset","name":"Holes"}}
]}
```

**Expected volume:**
- Plate: `L*W*T - 4*r²(1-π/4)*T`
- Each hole: `π*1.7²*(T-3.3) + π*3.25²*3.3`
- Total: 23335.2 mm³, with 22 faces.

After `set_parameter L=120` the volume is 35335.2 mm³, and the holes stay in the corners.

## Reading `measure`

- **Tolerance:** analytic and measured volumes agree to about 0.001-0.01%, so treat a gap that small as a match. Long shallow arcs measure worse, about 0.1% (R 1000 mm over 20 degrees).
- **Face counts:** compare the logical `faces`, not the larger `kernel.faces`.
- **Reply types:** `set_parameter` returns `recomputed` as a number. A batch reply echoes every step.
- **Boolean warning:** a feature `warning` such as `Boolean retry: temporary coincident-face separation up to 2.050000e0 mm per face` means the kernel nudged faces that coincide. A join flush on a face, such as a cylinder joined onto the floor of a shelled box, or a tapered cut that starts on a face, does not log it. When the volume still matches your number the warning is harmless. When it does not, the retry may have left the geometry wrong, with no error. A standoff built as a joined cylinder with a cut pilot hole, patterned together, reports `tool displacement` (moving a tool by about 2e-4 mm) but its volume agrees with the analytic growth; a boss with its hole as one ring profile (two concentric circles, extrude the ring) builds without a warning at every height tried (3 to 20 mm).

## Reply cost (0.8, plate example)

| Call | Size |
|---|---|
| `list_commands {}` | **Over 100 KB.** Never call it unfiltered; filter on a distinctive word such as `"hole"` or `"slot"`. |
| `body_topology` | 25-45 KB |
| `inspect_design {}` | 12 KB, and it grows with the sketches |
| `inspect_design {"measure":false,"sketches":false}` | 4 KB: parameters, timeline errors, bodies |
| `sketch.inspect` | 5 KB per small sketch |
| `parameters.list {"filter":"feature"}` | 3 KB |
| `measure {}` | 1 KB: volume, bbox, COM, face/edge/vertex counts |
| `sketch.solve` | under 100 B: dof, status, profile count |
| `screenshot {"source":"model","width":480,"height":320}` | one small image. Without `source`, a bridged server returns the whole app window and ignores the size. |

## Errors

| Error text | Fix |
|---|---|
| `... is not available right now: no sketch is being edited` | Run `sketch.create` or `sketch.edit {sketch}` first. |
| `unknown sketch entity 'p1'` | Use the ids from `created`. A circle's centre is `c1.center`. |
| `the corner must join exactly two lines` | It is a `rectangle.center` corner. Use the `{a, b}` form. |
| `` `cs_diameter` must be a number or an expression `` | Countersinks need `cs_diameter` and `cs_angle`, even with `clearance`. |
| `unsupported dimension role for this creator` | Check the roles table above. |
| `<Feature> cannot be patterned or mirrored yet` | Pattern the original feature, for example one 2x2 rectangular pattern. |
| `an edge is a point [x,y,z], {point} or {body, index}` | An edge pick contained an expression. Use plain numbers. |
| `boolean Cut: the operation failed ... planar faces only` | See "Primitive booleans" above. |
