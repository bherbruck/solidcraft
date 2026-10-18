# Point-to-point move and paste

Point-to-point placement translates geometry so the **From** point lands on the **To**
point. Orientation stays the same. Pick an existing point, body vertex, edge midpoint or
face position; in a sketch, either anchor can also be an empty place on the sketch plane.
The preview changes while you revise the picks. **OK** applies one undo step; **Cancel**
leaves the design unchanged.

## Sketch geometry

- **Move/Copy**: select geometry, press **M**, choose **Point to Point** under **Move Type**,
  then pick **From** and **To**. **Create Copy** keeps the originals.
- **Point to Point Paste**: copy geometry with **Ctrl+C** (**⌘C** on macOS), then choose
  **Point to Point Paste** in the context menu or press **Ctrl+Shift+V** (**⌘⇧V**).
  Pick the source anchor and destination, then **OK**. Internal constraints and dimensions
  travel with the clipboard copy, with independent dimension parameters.
- Ordinary **Ctrl+V** / **⌘V** still pastes centered at the cursor.

Anchors use sketch coordinates. When pasting into a different sketch, a source point
picked on the original sketch is interpreted in that sketch's coordinates; the destination
uses the sketch being edited.

## Bodies and components

Select bodies or one component placement and choose **Point to Point Move/Copy** from
the context menu (or command palette). Pick **From** and **To**; **Create Copy** keeps the
originals. The existing body **Move/Copy** dialog also offers **Point to Point**.

**Copy** / **Ctrl+C** / **⌘C** copies selected bodies or a selected component placement.
**Paste** opens the two-anchor placement dialog for these objects. Component copies are
linked instances of the same component definition. Body operations require their component
to be active. To move an individual assembly instance, select its component placement.
Object clipboard references belong to the current document and must still exist on paste.
Timeline feature copies also support **Point to Point Paste**.

World-coordinate anchors account for rotated and nested component placements, including
pending occurrence moves. Moving a nested component definition has the same shared-parent
behavior as the existing occurrence move command.

## Command interface

- `sketch.move`: `entities`, `from: [x,y]`, `to: [x,y]`, optional `copy`.
- `sketch.paste`: paired `from` / `to`, or the existing `at` / `translate` options.
- `objects.copy`: optional `objects` array of body selections, or one occurrence selection
  with its exact `path`; defaults to the current selection.
- `objects.move.point_to_point`: `objects`, world `from: [x,y,z]`, `to: [x,y,z]`, optional `copy`.
- `edit.paste.point_to_point`: `from` / `to`; uses the sketch clipboard while editing a
  sketch, otherwise the object or timeline clipboard.

Anchor pairs cannot be mixed with `at` / `translate`; point-to-point sketch moves cannot
be mixed with rotation. Programmatic commands apply directly without opening a dialog.
