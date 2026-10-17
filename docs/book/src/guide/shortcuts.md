# Keyboard shortcuts

Command shortcuts can be changed in File › Keyboard Shortcuts; these are the defaults.

Click a binding to record a new shortcut. A shortcut can contain Shift, Ctrl/Cmd and
Alt/Option modifiers plus exactly one other key. Hold the modifiers, then press the final
key; you can take your time assembling the modifiers before that key. The recorder also
allows modifiers to arrive after the ordinary key: it previews the chord and waits one
second after the last added modifier, and until all keys are released, before confirming.
For example, record Shift+Cmd+M for Sketch Mirror on macOS. Escape cancels recording without
changing the old binding. Pressing two different ordinary keys cancels the attempt.

The binding button shows modifiers immediately as you press them, even before adding the
ordinary key. Releasing a modifier before that key removes it from the preview. A status
line tells you to add a key, release all keys, or wait for confirmation; the proposed chord
stays visible during confirmation. Nothing is saved while the preview is still pending.

On macOS, the editor, Help list, toolbar hints, context menus and S search display Cmd and
Option. Existing settings retain SolidCraft's Ctrl/Cmd alias for the primary modifier;
Control and Command pressed together are recorded explicitly as Ctrl+Cmd. Conflicting
bindings still require confirmation, and built-in reserved shortcuts remain reserved.
Recording changes app settings, not the design file.

This follows the usual [modifier-before-key convention documented by Apple](https://support.apple.com/en-ie/102650)
and the [key-event and modifier-state model documented by W3C](https://www.w3.org/TR/uievents/#keys-modifiers).
The one-second grace period is SolidCraft's recording aid, not a standard-mandated delay;
using an already assigned shortcut has no added delay. Operating systems and browsers can
intercept reserved combinations before SolidCraft receives them. Choose a different chord
if that happens; recording is cancelled if the app loses focus.

| Key | Command | Key | Command |
|---|---|---|---|
| L | Line | E | Extrude |
| R | Two-Point Rectangle | Q | Press Pull |
| C | Center Diameter Circle | F | Fillet |
| D | Sketch Dimension | H | Hole |
| X | Construction | M | Move/Copy |
| O | Offset | J | Joint |
| T | Trim | I | Measure |
| P | Project | A | Appearance |
| Ctrl+Z | Undo | Ctrl+Y | Redo |
| Ctrl+N | New Design | Ctrl+O | Open |
| Ctrl+S | Save | Delete | Delete the selection |

Keys handled by the window itself:

| Key | Action |
|---|---|
| S | the S box: search every command at the cursor |
| V | show or hide the selected bodies, sketches and planes |
| F2 | rename the selected browser or timeline item |
| F6 | fit the view to the model |
| ← → ↑ ↓ | turn the view 15° (with Shift, a quarter turn) |
| Ctrl+← → ↑ ↓ | pan the view |
| Ctrl+8 | look straight at the selected face, plane or sketch |
| Esc | cancel the running command, or clear the selection |

While a value box has the keyboard (a dimension being added, a dialog field), keys type into
it and single-letter shortcuts don't fire. With Sketch Dimension (D), the box opens as soon as
you click a line, circle or arc (or the second point or line), so D, click, type the value,
Enter.

Mouse and touch:

| Action | Result |
|---|---|
| Click | select (Shift or Ctrl adds) |
| Click again on the same spot | select the next thing under the cursor (click cycling) |
| Hold the left button still (long-press on touch), or right-click › Select Other… | list everything under the cursor to pick from |
| Double-click a sketch curve or edge | select its chain or tangent loop |
| Double-click the middle button | go Home: the home view, framing everything shown (Preferences › Navigation › Middle double-click: Home, Fit or Off) |

Help › Keyboard Shortcuts in the app lists the current bindings, including any you changed.

With the **Touchpad** navigation preset, hold **Shift** while moving the pointer to pan, or
**Alt/Option** to orbit, without pressing a mouse button. Vertical scrolling zooms. These
viewport gestures are separate from rebindable command shortcuts; mouse-button selection
and editing keep their existing modifiers.
