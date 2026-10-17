# Ribbon structure

The UI layout in `crates/ui-egui/src/workspace.rs` maps the original engine registry placements into ribbon sections. `docs/features.md` continues to document registry placements; command registrations and ids are unchanged.

Always shown, in order: MODEL, SURFACE, ASSEMBLE, INSERT, INSPECT, MANAGE, VIEW. SKETCH appears during sketch editing; SHEET METAL appears for the selected body (or the last body of the active component) when it has evaluated sheet-metal data. Entering a new context selects its tab; the other tabs remain available. RENDER belongs to the Render workspace.

**Bold command ids are default promoted buttons.** Every listed command remains in the section menu. Saved user pins can add buttons, as before.

| Tab | Section | Command ids, in menu order |
|---|---|---|
| MODEL | SKETCH | **`sketch.create`**, `sketch.edit`, `sketch.line3d`, `sketch.spline3d`, `sketch.point3d`, `sketch.move3d` |
| MODEL | CREATE | **`solid.extrude`**, **`solid.revolve`**, **`solid.hole`**, `solid.sweep`, `solid.loft`, `solid.box`, `solid.cylinder`, `solid.sphere`, `solid.torus`, `solid.pipe`, `solid.bounding_solid`, `solid.thread`, `solid.boundary_fill`, `solid.coil`, `solid.rib`, `solid.web`, `solid.emboss`, `plastic.boss`, `plastic.lip`, `plastic.snap_fit`, `plastic.rest` |
| MODEL | MODIFY | **`solid.fillet`**, **`solid.chamfer`**, **`solid.shell`**, **`solid.press_pull`**, `sheet.convert`, `solid.combine`, `solid.scale`, `solid.offset_face`, `solid.draft`, `solid.split_face`, `solid.split_body`, `solid.replace_face`, `solid.remove`, `timeline.delete`, `face.delete` |
| MODEL | ARRANGE | **`solid.move`**, **`solid.mirror`**, **`solid.pattern.rectangular`**, `solid.align`, `solid.pattern.circular`, `solid.pattern.path` |
| MODEL | CONSTRUCT | **`construct.plane.offset`**, `construct.plane.at_angle`, `construct.axis.through_cylinder`, `construct.axis.normal_to_face`, `construct.axis.two_planes`, `construct.axis.two_points`, `construct.axis.edge`, `construct.point.vertex`, `construct.point.two_edges`, `construct.point.three_planes`, `construct.point.center`, `construct.point.edge_plane`, `construct.point.along_path`, `construct.plane.midplane`, `construct.plane.two_edges`, `construct.plane.three_points`, `construct.plane.tangent`, `construct.plane.along_path`, `construct.plane.perpendicular` |
| SURFACE | CREATE | **`surface.extrude`**, **`surface.revolve`**, **`surface.sweep`**, **`surface.loft`**, `surface.boundary`, `surface.patch`, `surface.offset`, `surface.ruled`, `surface.blend` |
| SURFACE | EDIT | **`surface.trim`**, **`surface.untrim`**, **`surface.extend`**, **`surface.stitch`**, `surface.unstitch`, `surface.reverse_normal`, `surface.fair`, `surface.fillet`, `surface.match` |
| SURFACE | CONVERT | **`surface.thicken`**, **`mesh.convert`** |
| ASSEMBLE | COMPONENTS | **`component.create`**, **`component.from_bodies`**, **`component.insert_design`**, **`parts.library`** |
| ASSEMBLE | JOINTS | **`joint.create`**, **`joint.as_built`**, **`joint.origin`**, **`joint.rigid_group`**, `joint.limits`, `joint.gear`, `joint.rack_pinion`, `joint.screw`, `joint.cam`, `joint.reach` |
| ASSEMBLE | POSITION | **`occurrence.ground`**, **`component.capture_position`**, **`component.revert_position`** |
| ASSEMBLE | MOTION | **`joint.drive`**, **`joint.motion_link`**, **`motion.study`**, **`motion.trace`**, `explode.create` |
| ASSEMBLE | CONTACT | **`contact.enable_sets`**, **`contact.check`**, `contact.enable_all`, `contact.disable_all`, `contact.create`, `contact.edit`, `contact.list` |
| INSERT | FILES | **`file.insert_step`**, **`file.insert_mesh`**, **`component.insert_design`**, `file.save_mesh` |
| INSERT | IMAGES | **`canvas.insert`**, **`canvas.decal`**, **`canvas.edit`** |
| INSERT | 2D | **`sketch.insert_dxf`**, **`sketch.insert_svg`** |
| INSERT | LIBRARY | **`parts.fastener`**, **`parts.library`**, **`parts.resize`**, `parts.insert` |
| INSERT | LINKS | **`component.update_links`**, **`component.break_link`** |
| INSPECT | MEASURE | **`inspect.measure`**, **`inspect.center_of_mass`**, **`component.properties`** |
| INSPECT | SECTION | **`inspect.section`** |
| INSPECT | CHECK | **`inspect.interference`**, **`inspect.draft`**, **`inspect.accessibility`**, **`contact.check`**, `inspect.compare` |
| INSPECT | SURFACE QUALITY | **`inspect.zebra`**, **`inspect.curvature_map`**, **`inspect.curvature_comb`**, **`inspect.isocurve`**, `inspect.minimum_radius`, `inspect.environment_map`, `inspect.continuity` |
| INSPECT | HISTORY | **`timeline.dependents`**, **`document.inspect`** |
| MANAGE | PARAMETERS | **`parameters.change`**, **`parameters.graph`**, `parameters.move`, `parameters.import`, `parameters.export` |
| MANAGE | CONFIGURATIONS | **`config.table`**, **`config.configure`**, `config.rules`, `config.preview` |
| MANAGE | MATERIALS | **`material.assign`**, **`appearance.assign`**, `material.properties.validate`, `library.inspect`, `library.export`, `library.apply` |
| MANAGE | RULES | **`sheet.manage_rules`**, **`plastic.manage_rules`**, **`plastic.assign_rule`** |
| MANAGE | DESIGN | **`document.units`**, **`timeline.compute_all`**, `edit.checkpoint`, `edit.restore_checkpoint` |
| VIEW | VIEWS | **`view.apply`**, **`view.save`**, `view.rename`, `view.delete`, `view.list`, `view.navigate` |
| VIEW | SELECT | Selection-filter controls (existing UI, no command ids) |
| SKETCH | CREATE | **`sketch.line`**, **`sketch.rectangle.two_point`**, **`sketch.circle.center`**, **`sketch.arc.three_point`**, `sketch.infinite`, `sketch.rectangle.three_point`, `sketch.rectangle.rounded`, `sketch.rectangle.center`, `sketch.circle.two_point`, `sketch.circle.three_point`, `sketch.arc.center_point`, `sketch.polygon.inscribed`, `sketch.polygon.circumscribed`, `sketch.polygon.edge`, `sketch.slot.center_to_center`, `sketch.slot.overall`, `sketch.point`, `sketch.line.midpoint`, `sketch.arc.tangent`, `sketch.circle.two_tangent`, `sketch.circle.three_tangent`, `sketch.slot.center_point`, `sketch.slot.arc_three_point`, `sketch.slot.arc_center`, `sketch.ellipse`, `sketch.spline.fit_point`, `sketch.spline.control_point`, `sketch.spline.control_point_5`, `sketch.conic`, `sketch.centerline`, `sketch.text` |
| SKETCH | MODIFY | **`sketch.trim`**, **`sketch.offset`**, **`sketch.fillet`**, `sketch.copy`, `sketch.cut`, `sketch.paste`, `sketch.blend_curve`, `sketch.chamfer.equal_distance`, `sketch.chamfer.distance_angle`, `sketch.chamfer.two_distance`, `sketch.extend`, `sketch.break`, `sketch.scale`, `sketch.move` |
| SKETCH | PATTERN | **`sketch.mirror`**, **`sketch.pattern.circular`**, **`sketch.pattern.rectangular`** |
| SKETCH | CONSTRAIN | **`sketch.dimension`**, **`sketch.constraint.horizontal_vertical`**, **`sketch.constraint.coincident`**, **`sketch.constraint.tangent`**, **`sketch.constraint.equal`**, **`sketch.constraint.parallel`**, **`sketch.constraint.perpendicular`**, `sketch.construction`, `sketch.constrainer`, `sketch.constraint.fix`, `sketch.constraint.midpoint`, `sketch.constraint.concentric`, `sketch.constraint.collinear`, `sketch.constraint.symmetry`, `sketch.constraint.curvature`, `sketch.constraint.polygon`, `sketch.auto_constrain`, `sketch.auto_constrain_from_datum` |
| SKETCH | REFERENCE | **`sketch.project`**, `sketch.intersect`, `sketch.include_3d`, `sketch.project_to_surface`, `sketch.intersection_curve`, `sketch.spun_profile`, `sketch.isoparametric_curve`, `sketch.fit_curves_to_section`, `sketch.insert_dxf`, `sketch.insert_svg`, `canvas.insert` |
| SKETCH | FINISH SKETCH | **`sketch.finish`**, `sketch.finish_auto_constrain` |
| SHEET METAL | CREATE | **`sheet.flange`**, **`sheet.hem`**, **`sheet.fold`**, **`sheet.perforate`**, `sheet.convert` |
| SHEET METAL | FLAT | **`sheet.unfold`**, **`sheet.refold`**, **`sheet.flat_pattern`** |
| SHEET METAL | OUTPUT | **`sheet.export_dxf`**, **`sheet.export_bend_table`** |
| SHEET METAL | RULES | **`sheet.manage_rules`** |
| RENDER | SCENE | **`render.scene`** |
| RENDER | IMAGE | **`render.start`** |

## Original placements to ribbon destinations

Explicit command-id lists override the default placement mapping. Shared commands appear in every section that lists them above. This table includes every command with an original toolbar placement.

| Original tab / panel | New tab / section | Command ids |
|---|---|---|
| MESH / MODIFY | INSERT / FILES | `file.save_mesh` |
| PLASTIC / CREATE | MODEL / CREATE | `plastic.boss`, `plastic.lip`, `plastic.snap_fit`, `plastic.rest` |
| PLASTIC / SETUP | MANAGE / RULES | `plastic.manage_rules`, `plastic.assign_rule` |
| RENDER / IMAGE | RENDER / IMAGE | `render.start` |
| RENDER / SCENE | RENDER / SCENE | `render.scene` |
| SHEET METAL / CREATE | MODEL / MODIFY | `sheet.convert` |
| SHEET METAL / CREATE | SHEET METAL / CREATE | `sheet.flange`, `sheet.perforate`, `sheet.hem`, `sheet.fold` |
| SHEET METAL / CREATE | SHEET METAL / FLAT | `sheet.flat_pattern` |
| SHEET METAL / MODIFY | MANAGE / RULES | `sheet.manage_rules` |
| SHEET METAL / MODIFY | SHEET METAL / FLAT | `sheet.unfold`, `sheet.refold` |
| SKETCH / CONSTRAINTS | SKETCH / CONSTRAIN | `sketch.constraint.horizontal_vertical`, `sketch.constraint.coincident`, `sketch.constraint.tangent`, `sketch.constraint.equal`, `sketch.constraint.parallel`, `sketch.constraint.perpendicular`, `sketch.constraint.fix`, `sketch.constraint.midpoint`, `sketch.constraint.concentric`, `sketch.constraint.collinear`, `sketch.constraint.symmetry`, `sketch.constraint.curvature`, `sketch.constraint.polygon`, `sketch.auto_constrain`, `sketch.auto_constrain_from_datum` |
| SKETCH / CREATE | SKETCH / CONSTRAIN | `sketch.dimension`, `sketch.construction`, `sketch.constrainer` |
| SKETCH / CREATE | SKETCH / CREATE | `sketch.line`, `sketch.rectangle.two_point`, `sketch.rectangle.three_point`, `sketch.rectangle.rounded`, `sketch.rectangle.center`, `sketch.circle.center`, `sketch.circle.two_point`, `sketch.circle.three_point`, `sketch.arc.three_point`, `sketch.arc.center_point`, `sketch.polygon.inscribed`, `sketch.polygon.circumscribed`, `sketch.polygon.edge`, `sketch.slot.center_to_center`, `sketch.slot.overall`, `sketch.point`, `sketch.line.midpoint`, `sketch.arc.tangent`, `sketch.circle.two_tangent`, `sketch.circle.three_tangent`, `sketch.slot.center_point`, `sketch.slot.arc_three_point`, `sketch.slot.arc_center`, `sketch.ellipse`, `sketch.spline.fit_point`, `sketch.spline.control_point`, `sketch.spline.control_point_5`, `sketch.conic`, `sketch.centerline`, `sketch.text` |
| SKETCH / CREATE | SKETCH / REFERENCE | `sketch.project`, `sketch.intersect`, `sketch.include_3d`, `sketch.project_to_surface`, `sketch.intersection_curve`, `sketch.spun_profile`, `sketch.isoparametric_curve`, `sketch.fit_curves_to_section` |
| SKETCH / FINISH SKETCH | SKETCH / FINISH SKETCH | `sketch.finish`, `sketch.finish_auto_constrain` |
| SKETCH / INSERT | INSERT / 2D | `sketch.insert_dxf`, `sketch.insert_svg` |
| SKETCH / INSERT | INSERT / IMAGES | `canvas.insert`, `canvas.decal`, `canvas.edit` |
| SKETCH / INSPECT | INSPECT / CHECK | `inspect.draft`, `inspect.accessibility` |
| SKETCH / INSPECT | INSPECT / MEASURE | `inspect.center_of_mass` |
| SKETCH / INSPECT | INSPECT / SURFACE QUALITY | `inspect.curvature_comb`, `inspect.minimum_radius`, `inspect.isocurve`, `inspect.zebra`, `inspect.environment_map`, `inspect.curvature_map` |
| SKETCH / MODIFY | SKETCH / MODIFY | `sketch.blend_curve`, `sketch.fillet`, `sketch.chamfer.equal_distance`, `sketch.chamfer.distance_angle`, `sketch.chamfer.two_distance`, `sketch.offset`, `sketch.trim`, `sketch.extend`, `sketch.break`, `sketch.scale`, `sketch.move` |
| SKETCH / PATTERN | SKETCH / PATTERN | `sketch.mirror`, `sketch.pattern.circular`, `sketch.pattern.rectangular` |
| SOLID / ASSEMBLE | ASSEMBLE / COMPONENTS | `component.create`, `component.insert_design` |
| SOLID / ASSEMBLE | ASSEMBLE / CONTACT | `contact.enable_sets`, `contact.enable_all`, `contact.disable_all`, `contact.create` |
| SOLID / ASSEMBLE | ASSEMBLE / JOINTS | `joint.create`, `joint.as_built`, `joint.rigid_group`, `joint.gear`, `joint.rack_pinion`, `joint.screw`, `joint.cam` |
| SOLID / ASSEMBLE | ASSEMBLE / MOTION | `joint.drive`, `joint.motion_link`, `explode.create`, `motion.study`, `motion.trace` |
| SOLID / CONFIGURE | MANAGE / CONFIGURATIONS | `config.configure`, `config.table` |
| SOLID / CONSTRUCT | MODEL / CONSTRUCT | `construct.plane.offset`, `construct.plane.at_angle`, `construct.axis.through_cylinder`, `construct.axis.normal_to_face`, `construct.axis.two_planes`, `construct.axis.two_points`, `construct.axis.edge`, `construct.point.vertex`, `construct.point.two_edges`, `construct.point.three_planes`, `construct.point.center`, `construct.point.edge_plane`, `construct.point.along_path`, `construct.plane.midplane`, `construct.plane.two_edges`, `construct.plane.three_points`, `construct.plane.tangent`, `construct.plane.along_path`, `construct.plane.perpendicular` |
| SOLID / CREATE | ASSEMBLE / JOINTS | `joint.origin` |
| SOLID / CREATE | MODEL / ARRANGE | `solid.pattern.rectangular`, `solid.pattern.circular`, `solid.pattern.path`, `solid.mirror` |
| SOLID / CREATE | MODEL / CREATE | `solid.extrude`, `solid.revolve`, `solid.sweep`, `solid.loft`, `solid.box`, `solid.cylinder`, `solid.sphere`, `solid.torus`, `solid.hole`, `solid.pipe`, `solid.bounding_solid`, `solid.thread`, `solid.boundary_fill`, `solid.coil`, `solid.rib`, `solid.web`, `solid.emboss` |
| SOLID / CREATE | MODEL / SKETCH | `sketch.create` |
| SOLID / INSERT | INSERT / FILES | `file.insert_step`, `file.insert_mesh` |
| SOLID / INSERT | INSERT / LIBRARY | `parts.fastener` |
| SOLID / INSPECT | INSPECT / CHECK | `inspect.interference` |
| SOLID / INSPECT | INSPECT / MEASURE | `inspect.measure` |
| SOLID / INSPECT | INSPECT / SECTION | `inspect.section` |
| SOLID / MODIFY | MANAGE / DESIGN | `timeline.compute_all` |
| SOLID / MODIFY | MANAGE / MATERIALS | `appearance.assign`, `material.assign` |
| SOLID / MODIFY | MANAGE / PARAMETERS | `parameters.change` |
| SOLID / MODIFY | MODEL / ARRANGE | `solid.move`, `solid.align` |
| SOLID / MODIFY | MODEL / MODIFY | `solid.fillet`, `solid.chamfer`, `solid.combine`, `solid.scale`, `solid.offset_face`, `solid.shell`, `solid.draft`, `solid.split_face`, `solid.split_body`, `solid.press_pull`, `solid.replace_face`, `solid.remove`, `timeline.delete`, `face.delete` |
| SOLID / POSITION | ASSEMBLE / POSITION | `component.capture_position`, `component.revert_position` |
| SURFACE / CREATE | SURFACE / CONVERT | `surface.thicken` |
| SURFACE / CREATE | SURFACE / CREATE | `surface.boundary`, `surface.patch`, `surface.extrude`, `surface.revolve`, `surface.sweep`, `surface.loft`, `surface.offset`, `surface.ruled`, `surface.blend` |
| SURFACE / INSPECT | INSPECT / SURFACE QUALITY | `inspect.continuity` |
| SURFACE / MODIFY | SURFACE / EDIT | `surface.stitch`, `surface.extend`, `surface.trim`, `surface.untrim`, `surface.unstitch`, `surface.reverse_normal`, `surface.fair`, `surface.fillet`, `surface.match` |

## Placement decisions and missing commands

- Save As Mesh (`file.save_mesh`), previously MESH / MODIFY, is in INSERT / FILES. The existing File menu continues to offer it.
- The selection filter, previously in SELECT on multiple tabs including UTILITIES, is in VIEW / SELECT. This is a populated control group, although it has no command ids.
- MODEL’s Pattern button uses the existing `solid.pattern.rectangular` command; Circular and Path stay in its menu. No generic pattern command is registered.
- There is no separate New 3D Sketch command. MODEL / SKETCH lists the existing 3D line, spline, point and move-point commands. Infinite Line (`sketch.infinite`) is in SKETCH / CREATE.
- Physical Properties maps to the existing `component.properties` command. Material data checks map to `material.properties.validate`. Calibrate maps to the existing canvas editor (`canvas.edit`).
- VIEW / VIEWS lists the existing named-view and navigation commands. Display, Show, Agent and Window are omitted: their requested settings have no engine commands. No Fit or Orientation commands were invented.
- RENDER / IMAGE exposes the already registered `render.start`, which previously had no visible panel. The Render workspace and its existing SCENE command are preserved.
- Constrain is the gallery exception to the four-promoted-button cap: the large Dimension button is followed by a 3 × 2 icon-only constraint gallery (Horizontal/Vertical, Coincident, Tangent; Equal, Parallel, Perpendicular). Every command remains in the section list and saved pins still apply.

## Saved customizations

Legacy toolbar preferences are plain maps keyed by `DESIGN/<tab>/<panel>`. On load, known keys migrate using the same command destinations. Previously promoted buttons absent from a saved list remain removed. Merging panels retains saved order within each source; sources merge in sorted-key order, and duplicates are removed. New promoted defaults are retained unless an old saved removal applies. Unknown keys and unknown command ids are preserved. An existing new destination takes precedence over a legacy destination.

Preferences and shortcut exports now wrap the map as `{"version":2,"panels":{...}}`. The version prevents repeated migration of unchanged SKETCH, SURFACE and SHEET METAL keys. Legacy imports still migrate. Reset restores the new defaults.

Additional old panels used for shared layouts migrate as follows: SELECT on any old tab → VIEW / SELECT; ASSEMBLE → ASSEMBLE / JOINTS; CONSTRUCT → MODEL / CONSTRUCT; INSPECT → INSPECT / CHECK; INSERT → INSERT / FILES; PLASTIC / MODIFY → MODEL / MODIFY; MESH / CREATE or PREPARE → SURFACE / CONVERT; UTILITIES / MAKE, ADD-INS or UTILITY → MANAGE / DESIGN. Command-id destinations override these panel defaults.

## Automation handles

The tab handle form stays `tab:<UPPERCASE NAME>`. Added: `tab:MODEL`, `tab:ASSEMBLE`, `tab:INSERT`, `tab:INSPECT`, `tab:MANAGE`, `tab:VIEW`. SURFACE and RENDER retain their names; SKETCH and SHEET METAL are available only in context. Legacy aliases remain: `tab:SOLID` and `tab:PLASTIC` → MODEL; `tab:MESH` → SURFACE; `tab:UTILITIES` → MANAGE. The Home and document-tab handles are unchanged.

Section labels retain `panel:<tab>:<section>` and collapsed sections retain `collapsed:<tab>:<section>`, with destinations from the tables above. In particular, `panel:SOLID:CREATE` becomes `panel:MODEL:CREATE`, selection-filter panels become `panel:VIEW:SELECT`, and `panel:SKETCH:CONSTRAINTS` becomes `panel:SKETCH:CONSTRAIN`. Command button and menu handles keep their command ids.

## Docked ribbon painting

The ribbon is a full-width surface immediately below the 34 px application bar, with no outer gap or rounding. `toolbar::{TAB_ROW_HEIGHT,GROUP_HEIGHT,RIBBON_HEIGHT}` reserve 30 + 86 + 1 px; browser and viewport placement follows the panel allocation. The workspace column is 88 px wide. Promoted buttons are 54 × 60 px with 3 px gaps (Sketch Create retains wider family selectors). Section labels occupy a 20 px row, inset 3 px, and retain the existing command lists, pin actions and drag targets.

Tab labels use normal word casing. Selected standard tabs use their soft accent; contextual tabs use their solid accent and add a row-wide line when selected. The existing `TabAccent` helper supplies all selection colours. Search is a 188 × 24 px launcher for the S box, using the existing `zoom` magnifier; it shrinks to 24 px before the tabs scroll horizontally. The selected tab scrolls into view after a context change or resize. Clicking Search or pressing S anchors the existing search popup below the field. The bottom command line remains available.

Sketch's Finish group reserves space at the far right even when other groups collapse. Its unfilled Finish Sketch button uses the existing `finish` icon. The chevron-free Exit row still opens the existing command list, preserving Finish with Auto-Constrain and saved pin customizations.

Additional handles: `ribbon:workspace`, `ribbon:search`, and `ribbon:section:<TAB>:<SECTION>`. Existing workspace, tab, panel, collapsed, family and command handles remain available. Layout evidence also publishes rectangles for these handles, `button:<command id>`, and the open `ribbon:search_popup`.

## Workspace and section lists

The workspace popup is 470 px wide, anchored below the workspace column. Design and Render keep their existing menu actions and enablement; planned workspaces cannot be selected. Each row also publishes `workspace:menu:<design|render|animation|simulation|manufacture|drawing>`. The constraint cells publish `ribbon:constraint:<command id>` alongside their existing `button:` and `toolbar:` handles. No icon assets or command ids were added.

Section lists use 30 px command rows, 20 px semantic icons, monospace shortcuts, hover pins and a pinning footer. `workspace::groups` optionally supplies headings and command order. MODEL / MODIFY uses EDGES, FACES, BODIES and SPECIALISE; unlisted commands follow the final heading in their original order. This ordering does not change promoted buttons or saved pins.

Ribbon tooltips show the full command name in bold and its registered shortcut in a key cap. Existing Render catalog descriptions are reused; commands without prose descriptions show the header alone (and their existing disabled reason when applicable). Registry parameter syntax is not presented as a description.
