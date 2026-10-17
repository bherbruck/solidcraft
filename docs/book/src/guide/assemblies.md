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
volumes in red, lists the pairs closer than its **Clearance** and each body's nearest neighbour.

The check behind it, `inspect.clearance`, does all of that in one call for any number of bodies,
which suits scripts and agents checking a whole assembly:

```json
{"command": "inspect.clearance", "params": {"clearance": 1, "nearest": 2, "skip_within": "component"}}
```

It returns `overlaps` (pairs sharing at least `min_volume`, 0.01 mm³ by default, with the shared
volume, area, box and centre), `near` (pairs closer than `clearance`, with the distance and the
two nearest points; a contact sharing less than `min_volume` is listed here at 0 with its
`overlap_mm3`), `nearest` (each body's `nearest` closest bodies) and `unchecked` (pairs whose
overlap could not be worked out, with the reason). `against` limits the check to pairs with one
body in `bodies` and the other in `against`, and `skip_within` leaves out pairs inside one
occurrence (`"occurrence"`) or one placed component, its subcomponents included
(`"component"`): the parts of a bought component that touch by design. Distances are measured on
the exact faces and edges; the meshes only rule out pairs that are too far apart, and the pairs
left are checked in parallel. `inspect.interference` remains for the overlaps alone.

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

**Cam** (`joint.cam {a, b, profile}`, on the **ASSEMBLE** panel) makes a follower ride a cam. The
cam turns on a revolute joint (a); the follower is a slider (b, a translating follower such as a
valve tappet) or a revolute joint (b, an oscillating follower such as a rocker arm). The profile
is the cam's outline in a plane square to its axis: a closed profile of a sketch in the cam's
component, or the cam's edges picked on its body (one click on an edge takes the closed loop it is
on in that plane). The follower is:

- a **Knife edge**: a point on the cam;
- a **Roller** of a radius, typed or taken from the roller's round face (picking that face also
  says where the roller is);
- a **Flat face** square to a slider's line, such as a bucket tappet.

A translating follower's line runs along its slider's axis, through its **Tip** when one is picked
(its knife's point or its roller's centre), so a follower offset from the cam's axis rides the
right flank. An oscillating follower needs its tip, or its roller's face, to know its arm. The
follower rides the cam from the side it is on (**Other side** takes the other one). The follower's
travel over a turn of the cam is worked out from the profile and kept with the link, one value a
degree with a smooth curve between them that never overshoots its samples. It is worked out again
whenever the design changes: change the cam's sketch, a parameter or the roller, and the follower
follows. Like the gears, the new link leaves the follower where it is (model it touching the cam),
and the cam is a motion link: it drives in **Drive Joints**, plays in motion studies, works in
closed loops (a tappet can be the piston of a slider-crank) and stops at contact sets. The profile
may also be on the grounded part, with the follower's guide on a turret that the cam joint turns.
`joint.list` gives a cam's link `lift` (the follower's lowest and highest values and its rise)
instead of a ratio. A profile that is not closed, a line or an arm that misses the cam somewhere,
a follower that would jump, and axes that are not square or parallel are refused.

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

**Export Animation…** in the **Motion Study** dialog saves the study as an animated GIF, drawn
from the viewport's camera as the study plays (the keys in the dialog count, even before OK).
`motion.export_animation` does the same for scripts, the CLI and agents, without a window: give a
`study`, or a `joint` with `from` and `to` (and `index?`) to animate a drive, and a `path`. A path
ending in `.gif` gets an animated GIF that loops; any other path is a folder that gets the frames
as `frame-0001.png`, `frame-0002.png`, and so on (made if missing; frames of a longer earlier
export there are removed), for a video tool. The web app writes GIFs only. `samples` is
the number of frames (one per step for a study by default, 100 for a joint), `width` and `height`
their size in pixels (800 × 500 by default, at most 4096 a side), and `fps` the GIF's speed (20
by default); `grid: false` and `axes: false` leave out the ground grid and the origin's axes.
`camera` takes the reply of the control channel's `ui.camera` (or any of its fields;
the rest come from the view `view.apply` chose, or the home view framing the model). The frames
are drawn by the same renderer as `solidcraft-cli snapshot`, on a copy of the design, which is not
changed. A GIF keeps one palette for all its frames, so colours don't flicker, and stores only
what changed from one frame to the next. Frames are capped: at most 1000, and a GIF holds at most
2^26 pixels of frames (about 160 frames of 800 × 500).

**Motion analysis.** The **Motion Study** dialog's **Study** box opens a saved study to edit, and
by its timeline the dialog charts what the study does.
**Plot** picks a moving joint value (a joint picked in the Browser is picked here too), or
**Picked point** once a point is picked in the dialog's **Point** box, and **Quantity** whether to
show its value, velocity or acceleration; the chart runs over the
study's steps with a cursor at the step shown, and a click or a drag on it moves to that step.
Values are in degrees and millimetres; rates are per step, or per second once **Duration** says
how many seconds the whole study takes. A picked point charts its distance from where it
starts, its speed and the size of its acceleration. **Check collisions** plays the study and
lists the pairs of parts that pass through each other, with the steps where they do (a click on
one shows the first of them); those steps are shaded on the chart. It checks the parts picked in
**Components** (against the ones picked in **Against**, when there are any), else every
component, or the pairs of the design's contact sets (a choice beside the button once it has any).

`motion.plot {study, joints?, occurrence?, point?, samples?, duration?}` answers the same
numbers: for each joint value its `value`, `velocity` and `acceleration` per sample (with their
units), and for a point its `position`, `velocity` and `acceleration` vectors, `speed` and
`acceleration_norm`. The study plays as `motion.play` places it (closed loops follow, contact sets
stop it), each sample from the one before, on a copy: the design is not changed. Velocities and
accelerations are finite differences between samples, central inside and one-sided at the ends,
so they are as smooth as the samples are dense (up to 1000, one per step by default). Angles are
unwrapped, so a joint that turns on past half a turn keeps counting.
`motion.interference {study, occurrences?, against?, contact_sets?, samples?}` answers each
colliding pair with its `first_step`, `last_step`, the `steps` ranges where they overlap, how
far into each other (`depth_mm`, the most at `deepest_step`) and their shared `volume_mm3`
there (null when it is not cheap to work out). It plays the study as `motion.play` does, but
contact sets don't stop it, and parts are tested at the samples only: a part that passes right
through another between two samples is missed. Parts are told apart by occurrence, as contact
sets tell them apart: an occurrence inside a sub-assembly placed twice is one part, so its two
copies are not checked against each other. A check takes at most 256 bodies; on a large design,
list the `occurrences` to check (an arm, with `against` the parts it must miss). When the work
runs out it says `truncated` and the step it `checked_to`.

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
