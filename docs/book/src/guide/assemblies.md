# Components and joints

A design is a tree of **components**. **New Component** (`component.create`) adds one, and
**Create Components from Bodies** (`component.from_bodies`) turns bodies into components. Each
component has its own origin, bodies, sketches and timeline entries. The **active component**
(the radio button in the browser) receives new sketches and features; the others are drawn
faded.

A component is placed by its **occurrences**. Paste creates another instance of the same
component, and **Paste New** creates an independent copy. Move an occurrence with the move
triad, then **Capture Position** to keep it or **Revert** to put it back. SolidCraft asks before
the next command if a move wasn't captured. **Ground** fixes an occurrence in place.

Paste New copies nested component definitions, sketches, feature histories and generated
dimension parameters. Repeated child instances remain shared within the new copy. Materials,
appearances and wholly internal joints/motion records receive copied references. Design-wide
user parameters remain shared; retained external links and omitted cross-boundary motion
records are reported. Unsupported destructive links refuse the entire copy. See
[individual-instance and copy details](https://github.com/bherbruck/solidcraft/blob/main/docs/occurrence-workflows.md).

**Joints** (`joint.create`, J) connect two components by joint origins that snap to faces, edges,
circles and vertices: rigid, revolute, slider, cylindrical, pin-slot, planar and ball, with
offsets, flip and limits. **As-built Joint** keeps the current position, **Rigid Group** locks
components together, **Drive Joints** moves a joint through its values, and **Motion Link**
couples two joints. Links can be chained (one joint drives a second, which drives a
third), and a value is carried through a chain of up to 64 links in whatever order they were
made. Contact sets stop motion where bodies would pass through each other,
**Motion Study** keys joint values along a timeline, and **Interference** shows overlapping
volumes in red.

**Gear**, **Rack and Pinion** and **Screw** (on the **ASSEMBLE** panel) are motion links whose
ratio comes from the parts rather than a typed number:

- **Gear** (`joint.gear {a, b, ra, rb}`) couples two joints that turn. Joint 2 turns ra/rb as far
  as joint 1, the other way round; **Internal gear** (one of them is a ring gear) turns both the
  same way. Pick the round face of each gear, such as its pitch cylinder, or type the radii. Tooth
  counts work as well as radii, since only their proportion counts. Picking a gear's face also
  chooses the joint that gear turns on.
- **Rack and Pinion** (`joint.rack_pinion {a, b, radius}`) couples a joint that turns with one
  that slides: the rack travels the pinion's pitch radius times its turn in radians (r·θ). Either
  joint may drive the other. In `joint.list` the ratio is given per degree, as for any link between
  an angle and a distance.
- **Screw** (`joint.screw {joint, pitch}`, or `{a, b, pitch}` for a revolute joint and a slider)
  advances `pitch` per turn. A cylindrical joint is a screw on its own: its turn drives its travel.
  The thread is right-handed; **Reverse** makes it left-handed.

A radius or pitch may be an expression of the design's parameters, and a picked face is found
again whenever the design changes. Change the parameter or the gear's diameter, and the ratio
follows. The new link leaves the driven joint where it is, so teeth modelled in mesh stay in mesh.
**Reverse** turns the driven joint the other way, for joints whose axes point opposite ways. These
couplings are motion links, so they work in closed loops, with contact sets and in motion studies:
a rack can push the piston of a slider-crank, or a pinion can steer through a rack and its tie
rods. Drive the joint that drives the coupling (joint 1, the pinion, or the screw's turn), not the
driven one. A joint follows one link at most, and a coupling that would drive its own driver is
refused. `joint.list` lists each coupling's definition under `links` with an `error` when it can't
be worked out (the picked face is gone); the link then keeps its last ratio.

A joint moves one component against another, so its two picks must be on different components.
A STEP file with several parts often comes in as bodies of one component, and picks on two of
those bodies make no joint. The Joint and As-built Joint dialogs say so under the picks and offer
**Create Components from Bodies**: each picked body gets a component of its own and stays where it
is, the picks are kept, and the joint previews. `joint.create` refuses such origins with the same
advice.

Joints may close a loop, as in a four-bar linkage or a slider-crank. When one joint of the loop
is driven, the other joints in the loop follow so that every joint still meets, and the
mechanism stays in the assembly mode it started in (an open four-bar does not flip to the
crossed one). If the motion would take a loop past the point where it can still close, for
example driving a rocker beyond its swing, the motion stops at the last position where the loop
closes, and the drive reports `stopped_by_loop`. A loop that cannot close at all is listed
under `conflicts` and nothing in it moves. `joint.list` reports how many degrees of freedom each
part still has with its loops closed: the moving parts of a four-bar have one between them, and a
loop locked by a rigid joint has none. The Drive Joints dialog says when a drive stops short and
why. While it previews, a row under the values gives the value the joint reached and the reason
(the loop that can't close further, or the two parts a contact set keeps apart), and after OK the
status line repeats it. The value box keeps what you typed. The dialog opens on the joint
picked in the browser, or on the first joint that moves when none is picked.

**Drag a part that joints place** and it moves as far as its joints allow: the point you
grabbed follows the pointer in the plane through it that faces the view, the joints on the
part's chain take the values that bring it nearest, within their limits, through motion links,
and with closed loops kept closed, and a four-bar's rocker stops at the end of its swing. The
whole drag is one undo step. A grounded part, or one no joint places, is not moved; a drag on
it draws a selection box as before. Each frame runs `joint.reach`, which scripts and agents use
directly to put a part's point somewhere: `joint.reach {occurrence, point}` moves the joints on
its chain from where they are to the nearest pose (the arm keeps its elbow up or down, and a
linkage stays on its branch). `at` picks the part's point (the centre of its bodies by
default), `z` with `axis` also turns one of its axes, and `joints` names the only joints to
move (the other joints of a closed loop they are in follow). The answer says whether it
`reached` the point and how close it got (`error_mm`, `error_deg`), and when it falls short,
the limits it rests on (`stopped_by_limit`) or the loop that stopped it (`stopped_by_loop`).
Contact sets are not checked by a reach; drive the joints for that.

The solver places what it reaches from the root and from grounded components. A chain of joints
that is joined to nothing grounded is never placed, so driving one of its joints moves nothing.
The drive says so: `joint.drive` answers `moved: false` with a `reason` and the `unconnected`
part ("Pedestal:1 is not connected to anything grounded"), the Drive Joints dialog shows the same
row, `motion.play` lists such joints under `unmoved`, and `joint.list` names every such chain under
`floating`. Ground a part of the chain, or join it to a grounded part (a rigid as-built joint keeps
it where it is), and it moves.

`joint.drive` answers the joint's values as `joint.list` gives them, in degrees and millimetres
with their units and limits, and `moved` says whether any part moved. A joint's origins are mated
by default: B's z axis is turned against A's, as two faces meet. `flip: true` lines them up
instead, so two origins given at one point with the same z stay as they are. For scripts that
drive many steps, `quiet: true` leaves the per-part degrees of freedom out of joint answers, and
`motion.play` with `quiet: true` gives the poses of only the parts that moved.

**Motion Trace** (`motion.trace`, on the **ASSEMBLE** panel) draws the path a point of a part
takes while a joint is driven or a motion study plays, such as the coupler curve of a four-bar.
Pick the point on the part (a face centre, a circle centre, an edge midpoint or a vertex), then
either a joint with the values to drive it from and to (a turn of the joint from where it is, at
first) or a motion study. The path previews in the viewport as the mechanism would move: closed
loops follow, and the path ends where a loop can no longer close or a contact set stops the
motion, which the dialog says under the values. OK keeps the path as a 3D sketch spline in a new
sketch of the root component (**Motion Trace1**, ...), where no joint moves it, and it can serve
as a sweep or pipe path. It is a fixed curve: it
does not follow later changes to the mechanism. A path that comes back to where it started is
kept as a closed, smooth spline.
`motion.trace {occurrence, point, joint, from, to, index?, samples?}` (or `{occurrence, point,
study, samples?}`) answers the world points without changing the design, and
`motion.trace.keep` with the same parameters (and `name?`) keeps them. A trace takes 2 to 500
samples: 100 by default for a joint, one per step for a study.

**Insert Design** (`component.insert_design {path, translate?}`, on the **ASSEMBLE** panel and
in the **File** menu) adds a component linked to another saved design, so each part can live in
its own document. A STEP or mesh file picked there is inserted as **Insert STEP** or **Insert
Mesh** would. The part's solid bodies come
in as they sit in that design, and they are kept in the assembly, so it opens even without the
part file. The link is stored relative to the assembly's folder once the assembly is saved.
After the part file changes, **Update Links** (`component.update_links`) reads it again.
**Break Link** (`component.break_link`) keeps the bodies and forgets the file, and
`component.links` lists the links and whether their files are found. A linked component holds
the part's bodies only: its timeline, parameters and sheet-metal data (flat pattern, bend
table) stay in the part's own design.

STEP export writes the component tree as an assembly.

Right-click a component in the Browser and choose **Properties** to inspect
material-aware mass, volume, surface area, centre of mass and world bounds. Descendants and
repeated instances contribute to assembly totals; pending moves are included. A component
uses its first placement; `occurrence.properties` selects a specific instance. Root Properties
measures the complete assembly. Volumes are summed, including overlapping bodies, rather than
measuring their union. The same workflow is available through `component.properties` and
`occurrence.properties`; see
[placement and measurement details](https://github.com/bherbruck/solidcraft/blob/main/docs/assembly-properties.md).

## Finding things in the browser

The field at the top of the browser finds components, bodies, sketches, construction planes and
joints by name (Ctrl+F with the pointer over the browser). It finds the text anywhere in a name,
ignoring case. With `*` (any text) or `?` (any one character) in it, the whole name must match:
`*:1` finds every first instance. Each placement of a component is found on its own, so a bolt
placed six times is listed six times, each under the components that hold it.

Only the matches and their parents stay in the tree, opened for the search; closing a row there
leaves the tree's own folds as they were. The chips narrow the search to one kind, or with
nothing typed list everything of that kind. **Select all** selects what was found, and **Hide
others** hides every body it didn't find. Esc or **×** ends the search. Scripts and agents search
the same way with `browser.find`.
