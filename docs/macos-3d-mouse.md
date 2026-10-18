# SpaceMouse on macOS

SolidCraft dynamically loads the **Navigation Library** from the user's installed 3DxWare:
`/Library/Frameworks/3DconnexionNavlib.framework/3DconnexionNavlib`. It does not ship a driver or
require an SDK to build. When Navlib is unavailable, the existing 3DconnexionClient/HID backend
handles the cap instead. Only one backend registers for input, avoiding duplicate motion.

## Setup

1. Install [3DxWare for macOS](https://3dconnexion.com/us/drivers/) and connect the SpaceMouse.
2. Launch SolidCraft and open a design. Leave **Preferences › Navigation › 3D mouse** enabled.
3. Select SolidCraft in **3Dconnexion Settings**. Choose **Object** for cap movements that move
   the model. Adjust sensitivity, axis directions, navigation mode,
   rotation center and horizon lock in that application profile.
4. Assign SolidCraft commands, including Fit, Home and standard views, to SpaceMouse Enterprise
   buttons or radial menus. Assignments use stable SolidCraft command identifiers. Driver-owned
   keyboard modifiers and macros continue to be handled by 3DxWare.

Design and Sketch have separate command banks. Entering or leaving a sketch automatically
activates the matching bank, so the same programmable button can start different tools.
When migrating an existing macOS driver profile, retain its native client identity (`SolidCraft`)
as well as the application bundle identity; a generic application profile can keep using its
default assignments even after the driver has selected the Sketch command bank. Back up driver
preferences before migrating assignments.

The driver calculates six-axis camera motion rather than emulating mouse drags or wheel steps.
It receives the camera, visible model and selection bounds, projection, up-axis and sketch lock.
The page showing the camera applies each completed navigation transaction through the shared
navigation handler. Render uses its cached product bounds; a sketch permits only pan and zoom.
Focus loss, the Home screen, open menus, file pickers and disabling 3D mouse navigation discard
queued motion and commands. Flat-pattern pages do not currently expose this 3D camera.

The integration exports command text and identifiers. It does not provide command artwork or
custom-view save/recall bindings for the Enterprise display. Availability of specific navigation
modes and device features depends on the installed driver.

SolidCraft checks for Navlib once at startup; installing or replacing the driver requires an app
restart. Without Navlib, Preferences keeps the existing speed and per-axis controls for the
fallback backend. With Navlib connected, Preferences points to 3Dconnexion Settings instead.

## Implementation and verification

- `apps/solidcraft/src/space_mouse_macos.rs`: safe snapshots, camera conversion and command dispatch.
- `apps/solidcraft/src/space_mouse_macos/ffi.rs`: the explicitly authorized native ABI exception.
  The dynamic library, property accessors and command tree stay owned until `NlClose` completes.
  Callbacks copy owned values, reject malformed numbers and contain Rust unwinds.
- `crates/ui-egui/src/space_mouse.rs` and `navigation.rs`: shared page routing and sketch restrictions.

Run `cargo test -p solidcraft --profile ci space_mouse_macos` and
`cargo test -p solidcraft-ui-egui --lib --profile ci space_mouse`. These cover native ABI sizes,
camera round trips (roll, Y/Z up, perspective/orthographic), pan plus zoom, invalid transforms,
atomic transactions, focus suspension and bounded command delivery. `cargo xtask ci` remains
the full repository gate.

For a physical smoke test, launch a sample design, confirm the log says it connected to the macOS
3DxWare Navigation Library, then test all six cap axes both separately and together. Verify
Object/Target modes, selected-item rotation center, horizon and rotation locks, Fit/standard view
buttons and an assigned modelling command. Enter a sketch and confirm rotation stays locked.
Switch to another app, disable navigation and unplug/reconnect the mouse; there should be no
stale movement or repeated button command on return.

### Local verification, 2026-10-17

Pan, zoom and rotation were confirmed on a physical SpaceMouse Enterprise in the macOS preview.
Assigned Extrude and Sketch Line commands were also confirmed, including the automatic Sketch
bank change. A migrated local driver profile initially lacked its `clientname` (`SolidCraft`);
restoring that identity enabled context-specific assignments without changing the app's bank API.
The initial zoom-only behavior came from selecting
`column_vectors` while supplying the default native row-vector layout. Keep `NlCreate` options
at the default matrix convention: right/up/back/eye rows, translation in the final row. The
incorrect combination generated projective transforms that the camera validator rejected.

The macOS build, nine Navlib integration unit tests and thirteen shared SpaceMouse tests pass.
The native tests include a regression check that an unavailable Navlib connection does not
schedule periodic UI wakeups. Ten existing driver-client/HID tests also pass.
An independent C ABI probe against the installed headers confirms the value/accessor/options/
command-node sizes. Formatting and asset/icon/list/layer checks pass. The full CI gate stops on
existing Rust 1.95 Clippy findings in `geom/fill_general.rs`, `ui-egui/canvas.rs`, `dialogs.rs`,
`issue_report.rs` and `sketch_tools.rs`. Scoped Clippy passes with only those two existing lint
classes (`nonminimal_bool`, `collapsible_match`) allowed; no new lint allowances are in source.
The review verification included upstream `487aa64e`, including its macOS driver-client/HID
backend. The Preferences note has English, German, Japanese, Korean and Chinese catalog entries.
