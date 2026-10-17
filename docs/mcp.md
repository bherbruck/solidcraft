# SolidCraft and AI agents (MCP)

SolidCraft speaks the [Model Context Protocol](https://modelcontextprotocol.io) over stdio
(JSON-RPC 2.0, one message per line; protocol revisions 2025-06-18, 2025-03-26 and 2024-11-05).
An agent models exactly like a person does: every tool runs engine commands, the same ones the
toolbar, palette, scripts and control channel use, and never opens a dialog.

Persistent headless and bridged sessions expose asynchronous product images through the
ordinary `execute` tool: `render.preflight`, `render.start`, `render.status`,
`render.cancel` and `render.export`. Keep the complete returned job ID; image bytes remain
in the session until explicit PNG export. See [render jobs](render-jobs.md).

For how the same command surface should work in the browser (workers, tiered storage, in-tab
agents), see [web-runtime.md](web-runtime.md). For sandboxed plugins and parametric timeline
features that also use `run(command, args)`, see [plugins.md](plugins.md). Desktop MCP in this
document remains the reference implementation today.

## Start it

```sh
# Headless: an in-process design session, no window (screenshots are drawn on the GPU).
solidcraft-cli mcp

# Headless, starting from a design file or a command script.
solidcraft-cli mcp --in part.solidcraft

# Bridged to the running app, so you can watch the agent model:
solidcraft --control 7878 &
solidcraft-cli mcp --connect 127.0.0.1:7878
```

Bridged, the server finds the app's control token in the token file the app wrote for that port
([control-protocol.md](control-protocol.md#token)); `--token T` or `SOLIDCRAFT_CONTROL_TOKEN` give
it explicitly when the app runs in a sandbox whose data folder the server can't see (prefer the
variable: command-line arguments are visible to other users in the process list).

A bridged server starts even when no app listens on that port yet, so an MCP client that
launches it before the app (or after the app was closed) keeps its tools. The server's
instructions then say the app was not running, and every tool call answers with an error that
starts `no SolidCraft app is listening at HOST:PORT` and says how to start it
(`solidcraft --control PORT`). The first call after the app starts connects to it, reading that
launch's token. When the app quits or crashes mid-session, the next call says the same; a call
that was running when it went away says the app did not answer and may have quit or crashed,
after running the call or not, so inspect the design before sending a change again.
A `batch` that could not be sent says the app is not running, so it is safe to send again.

stdout carries only protocol messages; errors go to stderr. Bridged screenshots are written to a
temporary file by the app and read back by the server, so both must run on the same machine.

### Which files the agent can reach

Tool calls read, write and delete files only inside the server's folders:

- the folder of the open design (the `--in` design, or whatever the agent last opened or saved),
  but only when it lies strictly inside your home folder (or the temporary folder) and not in a
  hidden folder. Home itself, a folder above it (`/`, `/home`, `/Users`, a drive's root),
  hidden folders such as `~/.config` and folders elsewhere don't count. Hidden files and folders
  inside the design's folder (`.env`, `.git`) are out of reach too;
- SolidCraft's own recovery folder, so `doc.recovery_list`, `doc.recover` and
  `doc.recovery_discard` work out of the box (discarding deletes only real recovery entries);
- the server's own scratch folder (a new private `solidcraft-mcp-*` folder in the temporary
  folder), where `screenshot` without a `path` goes;
- every `--allow-path DIR` (repeat it for more). A folder named here is reached in full,
  hidden files included.

The working folder the server was started in does not count by itself, so `~/.ssh`, `~/.aws`,
`~/.bashrc` and other dotfiles are never in reach by default. stderr lists the folders at
start-up and says when the `--in` design's folder was left out; `--allow-path` adds it back.

Paths are resolved before they are compared, so `..` and symlinks that lead out are refused
(saving through a link writes the file it points to, so the link's target is what counts), and
a new file must go into a folder that exists (a missing folder inside an allowed one is reported as missing; it is not created and does not call for a wider `--allow-path`). On Windows a path must be on a local drive
(`C:\…` or `\\?\C:\…`): UNC, device and NT paths in any spelling (`\\server\share`,
`//server/share`, `\\?\UNC\…`, `\??\…`, `\\.\…`), drive-relative paths (`C:file`), a `:` in a
file name (alternate data streams), reserved device names with any extension (`NUL`, `CON`,
`COM1.txt`, …) and short 8.3 names (`PROGRA~1`) are refused before any file system call, and the
resolved path is checked again, so a link or junction that leads to a share is refused too. This
includes a `sketch.text` font given as a file path and the font file a design's text names when
the design is opened; installed fonts named by family still work.
The server spells its own folders (scratch, recovery) with long names, so they work when the
temporary folder's path has a short name (`…\NAME~1.XYZ\AppData\Local\Temp`, as Windows often
gives when the user folder's name is long).
A refused path is a tool error saying why.

A hard link inside an allowed folder can't be told apart from an ordinary file, so a hard link
to a file elsewhere is reachable through it; don't keep such links in a folder you allow.

```sh
solidcraft-cli mcp --allow-path ~/designs --allow-path ~/exports
solidcraft-cli mcp --allow-any-path   # no limit (the behaviour before this option existed)
```

Bridged, the server sends its folders with every request (`allow_paths`, see
[control-protocol.md](control-protocol.md)) and the app adds its own recovery folder and the
folder of the design open in it, under the same rules, so give absolute paths: a relative one
resolves against the app's working folder, not the server's. The app's own
file dialogs are never limited.

### Tool annotations

Each tool says what it may do, for clients that ask before running tools (or auto-approve on
these hints). `list_commands`, `inspect_design`, `measure`, `body_topology` and `crash_reports`
are read-only. `set_parameter`, `undo` and `redo` change the design undoably. `new_design`
replaces it (destructive). `execute`, `batch`, `open`, `save`, `export` and `screenshot` (whose
`path` writes a file) can read, write, overwrite or delete files: they are not read-only,
`destructiveHint` and `openWorldHint` are true.

### Client configuration

Claude Code (project `.mcp.json`, or `claude mcp add solidcraft -- solidcraft-cli mcp`; a setup
added under the previous name keeps working, see the [compatibility notes](https://github.com/bherbruck/solidcraft/blob/main/docs/compatibility.md#mcp)):

```json
{
  "mcpServers": {
    "solidcraft": { "command": "solidcraft-cli", "args": ["mcp"] }
  }
}
```

Claude Desktop (`claude_desktop_config.json`), bridged to a running app:

```json
{
  "mcpServers": {
    "solidcraft": { "command": "/path/to/solidcraft-cli", "args": ["mcp", "--connect", "127.0.0.1:7878"] }
  }
}
```

### Agent skills

[`skills/`](https://github.com/bherbruck/solidcraft/tree/main/skills) holds four [Agent Skills](https://agentskills.io) for agents that model through this server:
- `solidcraft-modeling` covers building a new part.
- `solidcraft-editing` covers changing an existing design.
- `solidcraft-checking` adds checks for errors SolidCraft does not report, regression tests and porting, next to the other two.
- `solidcraft-mechanisms` covers assemblies: joints placed by world coordinates, motion links, closed loops such as actuators, contact sets and motion studies, checked against analytic values.

They cover the workflow, the command shapes, how to verify with `measure`, and the known traps. With them an agent builds a part in a few `batch` calls and reads far less output. To install them for Claude Code, copy the folders into `~/.claude/skills/`; [skills/README.md](https://github.com/bherbruck/solidcraft/blob/main/skills/README.md) has the details.

## Tools

| Tool | Arguments | What it does |
|---|---|---|
| `list_commands` | `filter?` | The command catalog: id, label, tab/panel, shortcut, parameter docs, enabled now. `filter` matches id, label, tab and panel. |
| `execute` | `command`, `params?`, `verbose?` (default true) | Run any command with JSON parameters (`list_commands` shows them). Failures leave the design unchanged. A feature command replies with the feature (`feature`, `name`), every body (`bodies`) and the bodies it created, changed and removed (`created_bodies`, `changed_bodies`, `removed_bodies`). `verbose: false` uses the same compact reply as `batch`. |
| `batch` | `commands: [{command, params}]`, `rollback?` (default true), `verbose?` (default true), `timings?`, `profile?` (default false) | Run steps in order, stop at the first error and undo the completed steps. The reply names the failing step (`failed.index`, `failed.command`, `failed.error`) and every result before it. `verbose: false` keeps the reply small in large designs: feature steps leave out the full body list (`created_bodies`, `changed_bodies`, `removed_bodies` stay) and sketch steps the ids they repeat outside `created`. Bridged to an app that has `engine.batch`, the whole batch is one control request (one frame of the app). Older apps, and apps showing the agent cursor, get the steps one by one. `timings: true` adds `timings: [{index, command, ms}]` for every step run, the failing one included, and `total_ms`. `profile: true` also adds each step's `profile: {boolean_ms, tessellate_ms, other_ms}`, its time in kernel booleans, in tessellation and the rest. Steps are timed where they run: in the app when bridged. An app without `engine.batch` is timed step by step, without `profile`. |
| `inspect_design` | `measure?` (default true), `sketches?` (default true) | Design properties (`metadata`: project, author… and other tools' keys; set them with `execute` `document.metadata.set`), parameters, timeline (type, suppressed, rolled back, error, warning, timing), bodies (volume, area, centre of mass, bbox, face/edge/vertex counts, visible; `removed_later: true` while a sketch is being edited on a rolled-back timeline and a later feature deletes the body, which is then not shown), sketches (curves, points, constraints, profiles, DOF, solve status, visible), selection, undo depth. Hide helper sketches, bodies or construction planes with `execute` `browser.visibility` (`{items, visible}` or `{folder, visible}`), and joint glyphs with `{joints: [id or name], visible}` or `{folder: "joints", visible}` (a hidden joint still constrains and drives; `joint.list` reports each joint's `visible`). |
| `measure` | `body?` or `items?` | Volume (mm³), area (mm²), centre of mass, bbox and topology counts per body, with totals. With `items` (one or two selections as `execute` takes them: `face`, `edge`, `vertex`, `sketch_curve`, `profile`), the picked geometry instead: see below. |
| `body_topology` | `body` | Edges (index, midpoint, length, ends) and faces (index, name, area, centroid, normal, `point`, and for a cylindrical face its `cylinder`: `origin`, `axis`, `radius`). An edge midpoint is the `[x,y,z]` that picks that edge for fillet and chamfer. A face's `point` lies on it and picks it (its centroid may not: that of a face with a hole is in the hole); a face's `name` picks it for the commands that take faces (`face.delete`, `solid.shell`, `solid.draft`, `solid.offset_face`, `solid.split_face`, `solid.replace_face`, and `face` of `sheet.convert`). |
| `set_parameter` | `name`, `value` (number or expression), `unit?`, `comment?` | Create or change a user parameter and recompute; reports `recomputed` (features rebuilt), `restored` (features whose earlier result for the same inputs was reused, e.g. a value set back) and features that now fail. |
| `screenshot` | `view?` (iso, top, front, back, bottom, left, right, home, fit, or a named view saved with `view.save`), `camera?` (`{target, yaw, pitch, distance, fov?}`, as `view.apply`), `path?`, `width?`, `height?`, `source?` (window, model), `grid?`, `axes?` | A PNG image content block. Headless: a render with the viewport's pipelines on a headless GPU device (see `solidcraft-cli gpu`; with no adapter, the CPU rasterizer, without analysis colours), fitted to the model, or from the named view or camera; the reply's `renderer` says which (`gpu`, `gpu-software`, `cpu`, with a `warning` for `cpu`). Bridged: the app window (default) or a model render with the app's camera; a named view or camera moves the app's viewport there first (`execute` `view.apply` does the same without a screenshot). `grid: false` and `axes: false` leave the ground grid (it lies on Z 0 and cuts through a part that straddles it) and the origin's axes out of a model render; axes hidden with `browser.visibility` stay out whatever `axes` says. Bridged, a screenshot that gives `grid` or `axes` is a model render, which follows the app's Grid and Origin switches for what it does not give; `source: "window"` with either is an error. A window that is minimised or on another macOS Space presents no frame: with no `source` the reply is a model render and says so (`fallback`); `source: "window"` is an error saying why, at once. Agents that don't need the app's chrome can ask for `source: "model"`. |
| `export` | `path`, `format?` (step, 3mf, 3mf-tagged, stl, stla, obj, urdf), `bodies?`, `mesh_prefix?`, `effort?`, `velocity?` | STEP, 3MF or mesh export; URDF writes the whole assembly with its meshes next to it (`docs/urdf.md`). |
| `undo`, `redo` | — | Undo / redo one change. |
| `crash_reports` | `id?`, `limit?` | The crash reports on this computer, newest first; with `id`, the whole report (backtrace, command, recent commands, `design_path`/`replay_path`). See [crash-reports.md](crash-reports.md). |
| `new_design` | `name?` | Start an empty design. |
| `open` | `path` | Open a `.solidcraft` design, a STEP file (`.step`/`.stp`) as a new design holding its bodies (an Import base feature), or a 3MF/STL file as mesh bodies. |
| `save` | `path?` | Save the design (to its current file without `path`). |

`measure` with `items` reports each item under `items` (`length_mm`, `area_mm2` or `position`).
A circle, arc, hole or other cylindrical face also gives `diameter`, `radius`, `center` and
`axis`, with `circumference` when it goes all the way round, or `arc_length` and `angle_deg`
for an arc; `round` says which (`circle`, `arc`, `cylinder`). A slot (a sketch profile of two
half circles and two lines, or one half-cylinder end of a slot in a solid) gives `slot`:
`width`, `center_distance` and `overall_length`. With one item these fields are repeated at
the top level. With two items:

- `min` (also `distance_mm`, with `from`, `to` and `delta`): the closest distance;
- `max` (with `max_from`, `max_to`): the farthest distance;
- `center_distance` (with `center_from`, `center_to`): when an item is round, from its centre
  or axis: axis to axis for parallel axes, the closest distance between the axes otherwise,
  and axis to plane, line or point;
- `measure`: the distance the Measure panel shows first (`center` where there is one, else `min`);
- `angle_deg` between two planar faces or straight edges.

Earlier fields are unchanged, so older scripts keep working.

To change a feature after it is made, read its definition with `execute` `timeline.inspect`
(`{feature}`: id or name) and change fields with `timeline.edit` (`{feature, set}`; e.g.
`{"extent": {"distance": "30"}}`). The fields per feature type are listed in the book's
*Solid features* chapter.

Resources: `solidcraft://design` (inspect JSON with measurements) and `solidcraft://commands`
(the catalog).

Errors: an unknown tool or arguments that don't match the tool's schema are JSON-RPC
`-32602` errors; an engine failure (bad command parameters, a feature that fails) is a tool
result with `isError: true` and the engine's message, which the model can read and correct.

`batch` rollback goes back to an engine checkpoint (`edit.checkpoint`) taken before the first
step: the design, the active sketch and the undo history are restored exactly as they were,
however long the history is. Batches that call `file.new` or `doc.open` get the design back,
not its file path. (An older app without checkpoints falls back to undoing as many steps as
the batch added to the undo history.)

## Camera and close-up recipe

The `camera` of `screenshot` (the object `view.apply`, `view.save` and `solidcraft-cli snapshot --camera` take too) is an orbit camera, `{target, yaw, pitch, distance, fov?}`, in millimetres and **radians**. Unlike the `angle` of `solid.move`, a bare number is not degrees here. The eye is at `target + distance * (-sin(yaw) * cos(pitch), -cos(yaw) * cos(pitch), sin(pitch))`, looking at `target` with Z up on the screen. So `yaw: 0, pitch: 0` is the front view: the eye at -Y, looking along +Y, with +X to the right. A yaw of π/2 looks from -X, π from +Y and -π/2 from +X. The pitch is the elevation angle up from the XY plane, and π/2 looks straight down with +X to the right and +Y up. `fov` is the vertical field of view in radians. `0`, or no `fov`, is an orthographic view whose visible half-height is 0.4 × `distance`; with a perspective `fov` the half-height at the target is `distance * tan(fov / 2)`. `inspect.section` removes what lies on the side its normal points to: it keeps the points `p` with `n · p < d`, where `d = n · origin` (`flip: true` keeps the other side). To look into the cut, put the eye on the removed side.

To look closely at one part of a large model, such as a STEP assembly, work in a headless session. These steps change only the open design in memory:

1. Open the model with `solidcraft-cli mcp --in assembly.step` (or the `open` tool, which also takes `up` and `names`, see step 4 and [STEP import](step-import.md)). `inspect_design` lists the bodies with their names and bounding boxes.
2. Remove what is in the way with `solid.remove`, e.g. `{"bodies": ["Cover"]}`.
3. Cut the region out of the rest with `solid.box`, e.g. `{"corner": [40, 0, -45], "length": 20, "width": 50, "height": 45, "operation": "intersect", "targets": ["Housing", "Plate"]}`. Each listed body is trimmed to the box, and one that lies wholly outside the box is removed. The box itself is not kept.
4. If the model lies on its back (many STEP files have +Y up), open it with `up: "y"` (the `open` tool, `doc.open`, `file.insert_step`), which turns the file's bodies a quarter turn about X (Y becomes Z). One already open is stood Z up with `solid.move`, listing every body you keep, e.g. `{"bodies": ["Housing", "Plate"], "axis": [1, 0, 0], "angle": 90}`. It turns the bodies about the world origin, and a bare angle is in degrees. A turn of +90° about X takes +Y to +Z.
5. Colour the parts that matter with `appearance.assign`, e.g. `{"bodies": ["Housing"], "color": "#3a7bd5"}`, and hide helper bodies or sketches with `browser.visibility`, e.g. `{"items": ["Plate"], "visible": false}`.
6. Cut a section with `inspect.section`, e.g. `{"plane": {"origin": [50, 30, 0], "normal": [0, 1, 0]}}` or `{"plane": "XY", "at": 5}`.
7. Take the picture with `screenshot`: a `camera` whose `target` is the centre of the region and whose eye is on the cut side, `source: "model"`, `grid: false` if the region straddles Z 0 (the ground grid would cut through it), and `path`, `width` and `height` as needed. An orthographic picture is 0.8 × `distance` high, so a `distance` of about 1.5 times the region's size frames it. Look at the image, adjust `yaw`, `pitch` and `distance`, and shoot again. Headless, the picture is always a render of the model; bridged to the app, `source: "model"` asks for that render instead of the app's window.

## Compact replies

`execute` and `batch` default to `verbose: true`, preserving the full command results.
For long modelling sessions, set `verbose: false` at the tool-argument level:

```json
{"name": "execute", "arguments": {"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}, "verbose": false}}
```

Feature replies omit the full `bodies` list while keeping `feature`, `name`,
`created_bodies`, `changed_bodies` and `removed_bodies`. A cut can therefore report a
changed body even when no body name was added or removed. Sketch replies keep new
IDs under `created` and omit only identical top-level `points`, `curves`,
`constraints` and `dimensions` lists. Other fields and errors are unchanged;
inspection commands still return their complete results. Compaction uses the
engine's reply directly, without an additional design-inspection request.

## Demo: an enclosure built over MCP

`examples/demo/enclosure_mcp.py` is a small MCP client. It drives a running app (`solidcraft
--control PORT`) through `solidcraft-cli mcp --connect` and builds an electronics enclosure one
`execute` call at a time (`enclosure_steps.json`): a rounded box, shelled open at the top, a
USB port cut through a side wall, four screw bosses, and then a change of the `width`
parameter that the timeline rebuilds. `examples/demo/record.sh` records the run with ffmpeg,
with each step's caption burnt in.

`examples/demo/enclosure_ui_mcp.py` (v2) builds the same part the way a person would: the
modelling goes through the app's UI over its control channel. The pointer glides to toolbar
buttons and faces and clicks them, the real dialogs open, the sketches go on the model's faces,
and the camera follows. The parameters, the sketch dimensions bound to them and two extrudes go
over MCP. With `--story v3` (the default) one standoff is patterned 2 × 2 in the Rectangular
Pattern dialog with spacings from the parameters, and width and wall are then changed in the
Parameters dialog. `record_ui.sh` records it and draws the pointer and the captions.

## A typical session

```text
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"me","version":"1"}}}
→ {"jsonrpc":"2.0","method":"notifications/initialized"}
→ {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"batch","arguments":{"commands":[
     {"command":"sketch.create","params":{"plane":"XY"}},
     {"command":"sketch.rectangle.two_point","params":{"p0":[0,0],"p1":[40,30]}},
     {"command":"sketch.finish"},
     {"command":"solid.extrude","params":{"distance":20}},
     {"command":"solid.fillet","params":{"edges":[[0,0,10]],"radius":3}},
     {"command":"sketch.create","params":{"plane":"XY"}},
     {"command":"sketch.circle.center","params":{"center":[20,15],"radius":5}},
     {"command":"solid.extrude","params":{"distance":20,"operation":"cut"}}]}}}
← {"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"ok\": true, \"completed\": 8, \"results\": [...]}"}]}}
→ {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"measure","arguments":{}}}
← … "total": {"volume_mm3": 22390.6, …}
→ {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"screenshot","arguments":{"view":"iso"}}}
← {"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"image","mimeType":"image/png","data":"iVBORw0…"},{"type":"text","text":"{\"source\":\"model\",…}"}]}}
→ {"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"export","arguments":{"path":"/tmp/part.step"}}}
```

The tests in `crates/mcp/src/tests.rs` run sessions like this (box → fillet → hole, parameters,
batch rollback, hostile arguments) and check the results only through MCP.
