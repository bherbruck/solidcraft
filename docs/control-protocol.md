# Control protocol

`solidcraft --control PORT` (or `SOLIDCRAFT_CONTROL_PORT=PORT`) listens on `127.0.0.1:PORT` for
JSON lines. Each request is one line `{"id": …, "method": "…", "params": {…}}`; each reply is one
line `{"id": …, "ok": true, "result": …}` or `{"id": …, "ok": false, "error": "…"}`. Requests are
handled on the UI thread between frames; pointer and keyboard input is injected as real egui
events, one pointer event per frame.

### Token

Every connection must authenticate. At start the app makes a random 256-bit token for this
launch and writes it, as one line of hex, to a file only the user can read:

| System | Token file |
|---|---|
| Linux | `$XDG_DATA_HOME/solidcraft/control/PORT.token` (`~/.local/share/solidcraft/control/PORT.token`) |
| macOS | `~/Library/Application Support/SolidCraft/control/PORT.token` |
| Windows | `%LOCALAPPDATA%\SolidCraft\control\PORT.token` |

On Unix the folder is `0700` and the file `0600`; on Windows the file's inherited permissions are
replaced by a single entry for the user. `SOLIDCRAFT_CONTROL_DIR` moves the folder. The file is
removed when the app exits, and the next launch writes a new token. The token file's path is
printed to stderr at start-up.

The first request on a connection must carry the token as a top-level `"token"` member
(`{"id": 1, "method": "ui.inspect", "params": {}, "token": "…"}`); later requests on the same
connection may leave it out. A first line without the right token is answered with
`{"ok": false, "error": "unauthorized: …"}` and the connection is closed, without running
anything. Tokens are compared in constant time.

`solidcraft-cli mcp --connect 127.0.0.1:PORT` reads the token file for that port by itself
(`--token T` or `SOLIDCRAFT_CONTROL_TOKEN` give one explicitly). For a test harness, or a
sandboxed package whose data folder the client can't see, start the app with
`SOLIDCRAFT_CONTROL_TOKEN` set to at least 32 hex characters (for example
`openssl rand -hex 32`) and the app uses that token instead of making one; the file is still
written. A one-line client in Python:

```python
token = (Path.home() / ".local/share/solidcraft/control/7878.token").read_text().strip()
sock.sendall((json.dumps({"id": 1, "method": "ui.inspect", "params": {}, "token": token}) + "\n").encode())
```

### Limits

The channel is untrusted input. A request line may be at most 16 MiB; a longer one is answered
with `{"ok": false, "error": "request too large"}` and the connection is closed, as are
connections that send eight lines that are not JSON. At most 16 connections are served at once
(further ones get `{"ok": false, "error": "too many connections"}`), at most 64 queued requests
run per frame so a flood cannot starve drawing, and a request that the UI thread has not
answered within 60 seconds (3600 seconds for `engine.batch`) is replied to with
`{"ok": false, "running": true, "error": "timeout: … is still running …"}`. The same 16 MiB line
limit applies to the MCP server.

A timed-out request has not failed: it is still running in the app and will still apply (a long
fillet, say), so don't simply send it again. The connection keeps its reply (#1618). Ask for it
with `control.pending`: `{"pending": <id>, "method": …, "done": false}` while it runs, then
`{"pending": <id>, "method": …, "done": true, "reply": {…}}` (its own reply, once), and
`{"pending": null}` when nothing is outstanding. Until it lands, any other request on that
connection is refused at once with `{"ok": false, "busy": true, "error": "busy: …"}` and not
run, so nothing is queued to apply unseen behind it.

Any request's `params` may carry `allow_paths: ["/abs/folder", …]`: every file that request
reads, writes or deletes (command paths such as `file.export`'s `path` or `doc.recovery_*`'s
`dir`, `ui.render`, `ui.screenshot`) must then lie inside one of those folders after `..` and
symlinks are resolved, or the request fails with `… outside the folders this agent may use …`.
The app adds its own recovery folder and, when it lies safely inside the home folder and not in
a hidden folder, the open design's folder (whose hidden files stay out of reach). The bridged
MCP server sends its folders this way on every request ([mcp.md](mcp.md)). Without
`allow_paths` file paths are not limited: a client holding the token can do what the user can.

## Engine

| Method | Params | Result |
|---|---|---|
| `engine.execute` | `command`: command id, `params`: JSON object | the command's result |
| `engine.script` | `commands`: `[{command, params}, …]` | results, stops at the first error |
| `engine.batch` | `commands`: `[{command, params}, …]` (at most 10 000), `rollback?`: bool (default true), `timings?`, `profile?`: bool (default false) | `{completed, results, undone}`, plus `failed: {index, command, error}` when a step fails, and with `timings` `timings: [{index, command, ms}]` (the failing step included) and `total_ms`; `profile` adds each step's `profile: {boolean_ms, tessellate_ms, other_ms}`. Runs the steps in one request, so within one frame, each as `engine.execute` runs it. The first failure stops the batch, and with `rollback` the design goes back to how it was before it (`undone`: the steps taken back). Refused before anything runs, with an error starting `engine.batch unavailable`, while the agent cursor is shown or when a step loads a file (`doc.open`, `file.insert_step`, `file.insert_mesh`). Send such steps one by one. Answered when done, for up to 3600 s |
| `engine.commands` | — | every command: id, label, tab, panel, icon, shortcut, params doc, enabled |
| `document.inspect` | `measure?`: bool | parameters, timeline (errors, timings), bodies, sketches, selection |

Commands never open dialogs when run this way. `solidcraft-cli commands` prints the same registry.

Product images use `render.preflight/start/status/cancel/export` through `engine.execute`.
Keep the returned session-scoped job ID for subsequent calls; status does not block or
render. See [render jobs](render-jobs.md) for capture, cancellation and export semantics.

While an interactive native file picker is open, `ui.inspect` reports `file_picker_busy: true`.
Engine commands, `ui.file` and `ui.home` requests wait for its answer; window resizing,
inspection and screenshots remain available.

Every `sketch.*` command that changes a sketch adds `created: {points, curves, constraints,
dimensions}` to its result (and `wires` for 3D curves): the ids of what it added, in creation
order, next to whatever else the command returns (`curves`, `sides`, `corners`…). Sketch ids are
stable and never reused, even after a delete (`l1`, `l2`, then `l3` once `l2` is gone; only undo
rolls the numbering back with the design), so they can go straight into later commands:

```text
{"id":7,"method":"engine.execute","params":{"command":"sketch.line","params":{"points":[[0,0],[40,0]]}}}
→ {"curves":["l1"],"created":{"points":["l1.start","l1.end"],"curves":["l1"],"constraints":["k1"],"dimensions":[]},…}
{"id":8,"method":"engine.execute","params":{"command":"sketch.dimension","params":{"entities":["l1"],"value":50}}}
```

A bare numeric `value` given to `sketch.dimension` (from the control channel, scripts or MCP) is
read in the design's units, exactly as if it were typed into the dimension box: in an inch design
`"value": 2` makes the line 2 in (50.8 mm) long. Point coordinates stay in millimetres. To pin a
unit regardless of the design, pass an expression with one: `"value": "50 mm"` or
`"value": "0.5 in"`. Angles are degrees unless the expression says otherwise.

The viewport also shows an X/Y/Z-aligned dimension chain between the endpoints of the
currently shown distance (centre, minimum or maximum), with each world-axis distance
labelled separately as an absolute value (including zero).
It uses the design's units and length precision, independently of the view or sketch plane.
Preferences → Graphics → **Show X/Y/Z distances** controls these extra lines and
labels (default on, saved between runs). Set `ui.prefs {"measure_axis_distances": false}`
to hide them while keeping the total distance and the dialog's ΔX/ΔY/ΔZ values. These
overlays remain visible until Measure closes.
Overlay handles `measure:label:x`, `measure:label:y` and `measure:label:z` identify the extra
labels; `prefs:measure_axis_distances` identifies the checkbox.

## UI

| Method | Params | Notes |
|---|---|---|
| `ui.inspect` | — | UI state, pending file picker (`file_picker_busy`; compatibility alias `file_dialog_open`, no path), what is hidden (`visibility`: bodies, origin and construction items, sketches, joints (`hidden_joints`, joint ids); set it with the `browser.visibility` command), viewport rect, camera, active tool/dialog, the status line (`status`: text and whether it is an error), hover, renderer, timings, live preview (`active`, `busy`, `error`, `ms`, bodies it stands in for) |
| `ui.set` | any `UiState` field (`tab`, `perspective`, `showGrid`, `dark`, …), and `hiddenBodies`, `hiddenOrigin`, `hiddenSketches`, `shownSketches`, `hiddenJoints` (whole lists; `browser.visibility` changes single items) | `dark: false` switches to the light theme |
| `ui.view` | `view`: `front`, `back`, `top`, `bottom`, `left`, `right`, `iso`, `home`, `fit`; `animate` (bool, default false: snap) | |
| `ui.camera` | `target?`: `[x, y, z]` (mm), `yaw?`, `pitch?`, `roll?` (radians), `distance?` (mm), `fov?`, `up?`; `fit?`: `true`, `false` or `[occurrence id or name…]` (`root`: the bodies outside every component) | the viewport's orbit camera: it looks at `target` from `distance` away; `yaw` 0 looks along +Y (from the front), `pitch` is the elevation (±π/2: straight down or up), `roll` turns the image. Fields given are set first (the others keep their values), then `fit` frames the listed occurrences (or everything), keeping the direction. The reply, passed back, sets the same view again, so a framing can be stored and replayed. `fov` and `up` follow the perspective setting (`ui.set`) and the up axis preference (`ui.prefs`) and can't change here; a refused field changes nothing |
| `ui.start` | `command`, `params?` | like a toolbar click: starts the sketch tool or dialog for the command. `params` sets what the dialog opens on, and nothing runs until OK: `joint.drive` takes `joint` (id or name; without it, the joint picked in the browser, else the first joint that moves) and `values` (numbers in degrees or millimetres, or expressions); `motion.study` takes `study` (id or name), which opens that study to edit and chart. A command that takes no params is refused before it starts; a param the dialog doesn't take is refused and the dialog closes |
| `ui.click` | `x`, `y` (screen points), `button?`: left/right/middle, `shift?`, `ctrl?`, `double?` | real pointer input |
| `ui.move` | `x`, `y`, `shift?`, `alt?`, `ctrl?`, `cmd?` | pointer movement without a button; modifiers apply until the next synthetic move/click; a move with omitted modifiers releases them. With `nav: "touchpad"`, Shift pans and Alt/Option orbits. |
| `ui.drag` | `x0`, `y0`, `x1`, `y1`, `button?`, `shift?`, `ctrl?`, `steps?`, `hold?`, `hold_end?` | press, move, release; a left drag on the model is a box selection (left to right: window, right to left: crossing); with OpenSCAD navigation, plain left-drag on empty space orbits, Shift+left-drag box-selects, and drags starting on sketch geometry edit it; `hold` keeps the button down that many frames first (a right press-and-hold opens the marking menu, releasing over a direction picks it), `hold_end` that many frames at the end (a sketch drag shows its snap) |
| `ui.menu` | `x?`, `y?`, `target?`, `close?` | with a point: open a context menu there as a right-click would (the viewport's marking menu, or a browser menu with `target: {"type": "body", "name"} \| {"type": "sketch", "id"} \| {"type": "component", "id"}`); without: the open menu (`radial` and `items`: id, label, shortcut, enabled) |
| `ui.menuPick` | `item`: id or label | runs an item of the open menu, as clicking it does |
| `ui.selectOther` | `x?`, `y?`, `pick?`, `add?`, `hover?`, `close?` | Select Other: with a point, list everything pickable there (as holding the left button does); `pick`: select row `pick` (`add`: as Shift-click; with a dialog open, into its active input); `hover`: light a row up; without: the open list (`open`, `at`, `total`: everything under the cursor, `rows`: label, icon, sel, at most 20 per kind, nearest first) |
| `ui.rename` | `text?`, `commit?` (default true) | finishes the rename box a Rename item opened |
| `ui.selection` | | the selection, the open dialog's inputs, the hovered item, and the active sketch's drawn dimensions (`dimensions`: id and text centre, `dimension_selected`, `dimension_editing`) |
| `ui.worldToScreen` | `points`: `[[x, y, z]…]` world points | their screen points |
| `ui.triad` | | screen points of the 3D sketch move triad's handles (`x`, `y`, `z`, `plane`) |
| `ui.sketchToScreen` | `points`: `[[x, y]…]` in active-sketch coordinates | their screen points (null when behind the camera) |
| `ui.dialogInput` | `index` | what clicking an input's Select chip does: picks go to that input |
| `ui.at` | `world: [x,y,z]` \| `sketch: [x,y]` \| `plane: "XY"` \| `axis: "Z"` \| `dimension: "d1"` (its value text) \| `handle: "arrow"` (a manipulator handle, see `scenario::publish_handle`) | the screen point (a scenario step's target; see `crates/ui-egui/src/scenario.rs`) |
| `ui.handles` | `prefix?` | the handles drawn last frame, `[{name, x, y}]`: toolbar buttons and drop-down items `toolbar:<command id>` (the button alone: `button:<command id>`; a drop-down row: `dropdown:<command id>`, highlighted `dropdown_hot:<command id>`, its pin `dropdown_pin:<command id>`; Customize Toolbar's check boxes `customize:<command id>`), panel labels `panel:<tab>:<panel>`, browser rows `row:<label>` and fold arrows `fold:<label>`, manipulator handles; signed-axis tips `axis_view:z`, `axis_view:-y`; optional cube facets `navigation_cube:top`, `navigation_cube:top_front_right`, controls `navigation_cube:control:home` (and other actions), plus signed-axis aliases for visible cube faces; Preferences navigator picker `prefs:navigator`, options `prefs:navigator:Signed axes` and `prefs:navigator:Cube`; in the Motion Study dialog, where the chart puts its first and last step `motion:chart_start` and `motion:chart_end`, the Check collisions button `motion:check` and each collision found `motion:collision:<n>` |
| `ui.editFeature` | `feature` (id or name) | what double-clicking a timeline item does: rolls back to it and opens its dialog filled in (sketches open in sketch mode) |
| `ui.scroll` | `x`, `y`, `delta?` | wheel zoom at the cursor |
| `ui.key` | `key` (egui key name, `Enter`, `Escape`…), `cmd?`, `shift?` | |
| `ui.text` | `text` | typed text |
| `ui.screenshot` | `path?` | PNG of the window (needs a presented frame: a minimised or fully covered window answers at once with `hidden: true` and an error; use `ui.render`) |
| `ui.render` | `path`, `width?`, `height?`, `grid?`, `axes?` | Render of the model with the current camera (no window needed), drawn on the app's GPU device with the viewport's pipelines (the reply's `renderer`: `gpu`, or `gpu-software` on a software adapter), in the window's theme (light or dark; `solidcraft-cli snapshot` keeps the light colours); cut by the section the viewport shows (`inspect.section`, an open Section Analysis dialog's preview, a sketch's Slice), with the cut face capped. The ground grid and the origin's axes are drawn as the window shows them (`ui.set` `showGrid` and `showOrigin`, `hiddenOrigin`); `grid` and `axes` (booleans) say otherwise for this render |
| `ui.confirm` | `accept?`: bool | answers the Delete confirmation (shown when a delete takes other features with it or makes some fail); without `accept`, what it lists |
| `ui.documents` | `action?`: `new`, `switch`, `close`, `quit`; `index?`; `force?` (close without asking); `answer?`: `save`, `dont_save`, `cancel` (answers "save changes?") | the open designs (tabs): names, unsaved flags, the active one, a pending "save changes?" prompt, whether a quit is asking |
| `ui.file` | `path`; `insert?`: bool | starts an interactive background Open or Insert; poll `ui.inspect.file_busy` for completion; the window stays responsive |
| `ui.home` | `open?`: bool; `sample?`: index | the start page: show or hide it, open a built-in sample (once built); returns `open` and the recent designs |
| `ui.shortcut` | `command`, `key?`, `replace?` | a command's key, or rebinds it (`""` clears; a key another command uses is refused unless `replace`) |
| `ui.help` | `item?`: `about`, `shortcuts` (read only), `report` (opens a report draft for review), `crashes` (the Crash Reports window) | the version, commit, build date and diagnostic text |
| `ui.prefs` | any of `default_units`, `nav` (`fusion`, `solidworks`, `inventor`, `alias`, `blender`, `tinkercad`, `openscad`, `touchpad`), `zoom_reverse`, `zoom_to_cursor`, `smooth_wheel_zoom` (default true), `trackpad_pan` (default true: two-finger scroll pans, except Touchpad always zooms; pinch zooms), `orbit_around` (`view_centre` (default: what Pan moves and Fit frames), `origin` (the world origin stays still on screen), `cursor` (the model point under the pointer when the drag began); the older `orbit_cursor` boolean still sets `cursor` when true), `look_at_anim`, `look_at_speed`, `middle_double_click` (`home` (default), `fit`, `off`), `msaa`, `show_frame_rate` (default false: frames per second in the view's bottom-right corner; the view redraws every frame while it is on), `length_decimals`, `angle_decimals`, `theme` (`dark`, `light`, `system`), `language` (`en`, `ja`, `de`, `fr`, `ko`, `ru`, `uk`, `zh`; see [localization](localization.md)), `ui_scale`, `ui_font` (an installed font family or a font file for the interface; empty for the built-in font), `up_axis` (`z`, `y`), `offer_stand_up`, `color_vision` (`off` (default), `protan`, `deutan`, `tritan`: colour vision modes, #778), `check_updates` (default true; [updates](book/src/guide/updates.md)); `open` (a page: General, Navigation, Keyboard Shortcuts, Design, Graphics, Sketch, Selection, AI) opens the Preferences dialog there; `button`: `ok`, `cancel`, `apply`, `defaults`; `stand_up: true` stands up a just-opened Y-up file, `stand_up: false` leaves it as it is ("Leave it": the offer closes and `offer_stand_up` stays on); the offer's buttons are the handles `stand_up:stand`, `stand_up:leave` and `stand_up:never` | sets preferences (kept between runs), returns them all plus `dialog` (the open page or null), `dark` and `stand_up_offer` |
| `ui.capture` | `action?`: `capture`, `revert`, `cancel` | answers the Capture Position question (asked when a command starts or the design is saved while components were moved without capturing); without `action`, what waits |
| `ui.window` | `action`: minimize, maximize, restore, toggle, close | what the title bar's caption buttons do (on Windows and Linux the application bar is the title bar); close asks about unsaved designs first |
| `ui.agent_cursor` | `show` (bool), `speed`: a number from 1 to 5 (times the normal speed) or `"instant"` (no animation: the command runs at once and the pointer jumps to where it acted), `follow_camera` (bool); all optional | the settings. While the cursor is shown, `engine.execute` first glides to the command's toolbar button and to each pick, then runs and replies; later requests wait their turn. `"animate": false` on a call skips it |
| `ui.agent_chat` | `features?`, `toolbar_button?`, `permissions?` (`ask`, `edits`, `all`), `add_agent?` ({`name`, `kind`: `acp` or `api`, `command` or `base_url` + `model`}; no keys), `default_agent?`, `agent?` (the conversation's agent, by id), `mode?` (the agent's own mode), `model?` (one of its models), `open?`, `side?` (`left`, `right`), `minimized?`, `width?`, `picker?`, `refresh?`, `connect?`, `stop?`, `send?`: text, `attach_view?`, `permission?`: an option id or `"cancel"` (the newest question of the agent's own), `approve?`: true, false or `"always"` (the oldest request held for Allow/Deny), `cancel?` (Stop: the queue stays), `send_now?` (stop the running step, then send the queued messages), `remove_pending?` (index), `new_chat?` (clear the chat, new session), `undo?` (the last message's changes) | the chat's state: `features`, `agents` (found and added), `currentAgent`, `state` (off, starting, ready, busy, auth, exited), `agent`, `modes`, `mode`, `models`, `model`, `authMethods`, `toolsPort`, `pid`, `waiting`, `turn`, `pending` (queued messages), `stopped`, `detection` and the conversation (`items`). With `features` off only the settings apply. Desktop only; see [Agent chat](agent-chat.md) |
| `ui.resize` | `width`, `height` | |
| `app.quit` | `force?` (quit without asking about unsaved designs) | `quitting` (the window closes), `asking` (a "save changes?" prompt is open) |

`ui.workspace {workspace?: "design" | "render"}` reads or changes the active workspace.
Entering Render requires finishing the current modelling operation; returning restores
the Design camera. See [the integrated Render workspace](render-workspace.md).

### Continuous camera motion

Continuous camera motion is also available through `engine.execute` with command
`view.navigate`: pass `camera` (from `ui.inspect`) and `motion: "pan" | "orbit" | "zoom"`.
Pan and orbit take `delta: [dx,dy]` in screen points (each within ±10000); pan also requires
`viewport: [width,height]` (each 1..1000000). Orbit accepts a world-space `pivot` with the
viewport size. Zoom takes `factor` (0.05..20, below 1 zooms in) and an optional world-space
`anchor`. The returned `camera` is applied immediately by the UI, without animation,
document edits, undo steps or a success-log entry per movement.

## MCP

`solidcraft-cli mcp --connect 127.0.0.1:PORT` bridges an MCP client (an AI agent) to this
channel; see [mcp.md](mcp.md).

## Example

```text
{"id":1,"method":"engine.execute","params":{"command":"sketch.create","params":{"plane":"XY"}}}
{"id":2,"method":"engine.execute","params":{"command":"sketch.rectangle.two_point","params":{"p0":[0,0],"p1":[40,30]}}}
{"id":3,"method":"engine.execute","params":{"command":"solid.extrude","params":{"distance":20}}}
{"id":4,"method":"engine.execute","params":{"command":"solid.fillet","params":{"edges":[[0,0,10]],"radius":3}}}
{"id":5,"method":"ui.screenshot","params":{"path":"/tmp/part.png"}}
```

Preferences → Navigation → View navigator offers **Signed axes** (the original default) and
**Cube**. This choice is saved independently of the mouse preset. The control channel selects it
with `ui.prefs {"navigator_style":"axes"}` or `ui.prefs {"navigator_style":"cube"}`.
Existing settings without this field keep signed axes. Signed-axis tips choose principal views;
clicking an aligned tip again reverses the view.

Hold Shift during an orbit gesture to snap to the nearest standard or diagonal view. This applies to
right-drag, the Orbit navigation tool, and dragging either view navigator. Under the default preset,
Shift+middle also orbits with snapping; middle alone pans. Cube faces choose principal views,
and corners choose isometric views. The middle of each cube edge belongs to its face. Face names
follow the up-axis preference; the small X/Y/Z triad shows world axes. Clicking the same face keeps that view.
The inward-pointing arrows orbit by 90 degrees; the curved arrows at the upper right roll the
view by 90 degrees. The Home button at the upper left returns to isometric and the lower-right circle
fits the model.

The cube is subdued while idle. Hovering its area brightens the cube and navigation bar and
reveals the surrounding controls; the picked face or corner gets a pale blue fill and
blue outline. Moving away restores the subdued state. Navigation-bar controls also brighten
when hovered or when a navigation tool is active.

The cube has six face targets and eight corner targets, with no edge targets. Corner picking
regions lie on the six flat faces, preserving sharp corners with no permanent subdivision
lines. Hovering a corner highlights its parts on the adjacent visible faces together.

The cube widget occupies a compact 112-point square in the viewport corner. A straight-on
cube face is approximately 55 points wide; painting and picking scale together. Face labels
are stretched vertically. View clicks ease through intermediate orientations when Navigation
animated view changes is enabled (the default), at the configured animation speed.

The cube runs `view.navigate` through the command engine. Programmatic callers pass `camera`
and either `direction: [x,y,z]` in the camera's up-axis frame or `action` (`left`, `right`, `up`,
`down`, `clockwise`, `counterclockwise`, `home`, `fit`). The result contains `camera` and `fit`;
UI callers apply the camera with their animation preference, without editing the design.

Shift-orbit snapping includes 45-degree edge views and isometric corner views, with a
lightly damped spring transition and a small boundary margin to prevent flicker.
