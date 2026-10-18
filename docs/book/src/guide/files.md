# Import and export

| Format | Read | Write |
|---|---|---|
| `.solidcraft` (versioned container or JSON design) | yes | yes |
| `.json` (self-contained plain JSON design) | yes | yes |
| STEP (AP203, AP214, AP242) | solids, assemblies, units, names, colours | AP242 with names, colours and the component tree |
| IGES 5.3 | manifold solids, or trimmed surfaces sewn into solids | manifold solids with names and colours |
| FreeCAD `.FCStd`, Rhino `.3dm` | solids, through cadmpeg (desktop app only) | |
| 3MF, STL, OBJ | mesh bodies | yes |
| glTF 2.0 (`.glb`, `.gltf` with its `.bin` files) | mesh bodies in the node tree's components, materials as appearances | |
| 3MF with face tags | as 3MF | each triangle tagged with its B-rep face, exact coordinates |
| URDF | | the assembly as a robot: links with meshes, mass and inertia, joints from the assembly's joints |
| DXF, SVG | into a sketch | sketches and flat patterns as DXF |
| PNG, JPEG | canvases and decals | |

Opening a STEP, IGES, FreeCAD or Rhino file creates a design with the file's bodies as a base feature, and
inserting one adds them to the current design. Later features build on them. Mesh bodies render,
measure, move and export, but solid features need B-rep bodies.

Every imported file (STEP, IGES, 3MF, STL, OBJ or glTF) comes in as a new component named after the
file, so its bodies never mix with yours. A STEP assembly keeps its structure: its parts and
sub-assemblies become nested components, and a part used several times is one component with an
occurrence for each place. The imported assembly is rigid: moving its component moves everything,
and its parts don't move on their own. Right-click the component and choose **Unlock Contents** to
joint the parts one by one. Undo removes a whole import in one step.

A glTF file (`.glb`, or `.gltf` with the `.bin` files it names in the same folder) comes in the
same way: its nodes become nested components, a mesh placed several times is one component with
an occurrence for each place, and the whole import is rigid until you unlock it. glTF is in metres
with Y up, so the import is scaled to millimetres and turned to stand Z up (`up: "z"` keeps the
file's axes). Each material's base colour, metal and roughness become the body's appearance, so
the model looks right in Render; textures, skins and animations are not imported. Scaled or
mirrored nodes are baked into their meshes. Compressed meshes (Draco, meshopt) are refused with a
message: save the file uncompressed first.

A 3MF project saved by a slicer comes in too, also when it keeps its objects in model parts of
their own inside the file (the 3MF Production extension): each object on the build plate becomes
a mesh body, with its parts where the slicer placed them.

**File → Save as mesh…** (also on the Mesh tab under Modify, and in the right-click menu of a
body) saves the selected bodies as STL, 3MF or 3MF with face tags (below). With **Send to slicer** ticked, it writes the file to
a temporary folder and opens it in the chosen slicer instead of asking for a file name. The slicer
list shows the slicers found on the computer with their icons; **Custom…** adds any other program
to the top of the list (kept between runs; hover an entry you added and press Delete to remove it).

Closed mechanical meshes with a constant cross-section can be rebuilt as solids with the
`mesh.convert` command (command palette, scripts and the control channel).

**File → Save as mesh…** with the format **3MF with face tags** (`file.export` or `file.save_mesh` with `format: "3mf-tagged"`) writes an
ordinary 3MF in which every triangle also names the B-rep face it lies on and every coordinate
is the tessellation's own double-precision value. Meshing and analysis tools can use the tags to
keep a model's faces (for loads, boundary conditions or feature edges); other 3MF readers ignore
them. The format is specified in [`docs/formats/3mf-face-tags.md`](https://github.com/bherbruck/solidcraft/blob/main/docs/formats/3mf-face-tags.md).

**File → Export URDF…** (`file.export` with a `.urdf` path) writes the assembly for robot
simulators and ROS: a link per component placement with its mesh, mass and inertia, and the
assembly's joints, so the parts move in the simulator as they do in SolidCraft. The meshes are
written as STL files next to the `.urdf`. See [`docs/urdf.md`](https://github.com/bherbruck/solidcraft/blob/main/docs/urdf.md).

Saves are atomic: a crash at any moment leaves either the old file or the new one, and the
replaced version is kept as `<file>.bak`. A save keeps the file's permissions, and a design
opened through a symbolic link (a synced folder linked into a project) is saved into the file the
link points to, so the link stays. Unsaved changes are autosaved to a recovery folder,
and the next launch offers to recover them. Older design files are upgraded on open, and files
from a newer version are refused with a message.

**Save** writes a container by default; files named `.json` keep JSON on subsequent saves.
**Save As** offers a container or plain JSON on
desktop; **Save As JSON** is also available in the File menu on desktop and web. Choose
the `.json` extension for plain JSON. **Export Design as JSON** writes an independent,
self-contained copy without changing the current file. Scripts can explicitly pass
`"format": "json"` to `file.save` or `file.save_as` even with a `.solidcraft` name.
Opening detects the format from the contents, so existing JSON designs keep working.
Containers favor fast reading and saving by default. Scripts can request
`"compression": "compact"` when file size matters more than CPU time.

Containers deduplicate identical STEP, mesh and image payloads. Mesh coordinates retain
their original double precision; this is not a conversion to STL. Geometry still rebuilds
from the timeline when opening. When a saved display preview is available, desktop Open
shows it while checking geometry; editing starts after the complete rebuild. Cancelling
keeps the current design. Previews cannot be edited or measured. Scripts can opt in to a disposable geometry
cache with `cache: true` on warm saves; saving never rebuilds cold geometry just to
create one. Ordinary saves and browser downloads default to `cache: false`. An additional display-mesh cache remains opt-in
with `preview: true`. Unsupported or mismatched caches are ignored. Browser opening
still rebuilds synchronously.
Warm saves with explicit `cache: true` can also include a small PNG thumbnail. It travels with the file, so
Home can show the recent design without reconstructing it when no local picture
is available. Missing or incompatible pictures use the usual thumbnail generation.
The default `cache: false` omits the portable thumbnail too.
The repository's `docs/design-container.md` describes limits and encoding details.

Designs are saved in the newest format this version of SolidCraft knows (`"format": "solidcraft/N"`
in the file). It opens designs of every older format and upgrades them as they open; a design
from a newer version asks you to update instead of being guessed at. Development (nightly) builds
save a format number no release reads yet.

Sketch patterns retain their live direction-line references and placement settings when saved.
Older patterns keep their fixed vector directions and original placement defaults.

Upgrading keeps what a design meant. Designs keep recorded body transfers, explicit 3D curve coordinate
spaces and certified face bindings, alongside semantic configuration styles, associative
free-form offsets and cylindrical emboss selections. Older ownership maps, geometry and
expressions retain their saved meaning. Legacy 3D curves with retained associations receive
their producer's coordinate-space marker without changing any saved points; detached curves
retain world coordinates. A lost cross-component association refreshed by an older build may
need an explicit relink or refresh when its source returns. Loading never guesses a numerical
correction. Association records, coordinate arrays and placements are bounded and checked.

## Design properties

**File → Design Properties…** (also under **Document Settings** in the Browser, and on the
top component's right-click menu) records who made a design and what it is: project, part
number, revision, author, organization, description and licence. **OK** applies the changes as
one undo step. Scripts and agents use the same commands: `document.metadata` reads them,
`document.metadata.set` (`key` and `value`, or `values: {key: value, …}`) and
`document.metadata.remove` (`key` or `keys`) change them. `document.inspect` and the MCP
`inspect_design` tool include them.

Exports carry them where the format has a place for them:

| Format | Written |
|---|---|
| STEP | `author` and `organization` in `FILE_NAME`, `description` in `FILE_DESCRIPTION` (on one line) |
| IGES | `author` and `organization` in the global section |
| 3MF | `project` as `Title`, `author` as `Designer`, `description` as `Description`, `license` as `LicenseTerms` |

The part number and revision have no standard place in these formats and stay in the design.

In the file they are one optional top-level `metadata` object; a design without properties has
none, and is saved exactly as before:

```json
"metadata": {
  "project": "ACME enclosures",
  "part_number": "ACME-1042",
  "revision": "B",
  "author": "Jane Doe",
  "organization": "ACME",
  "description": "Vented case for Raspberry Pi 4",
  "license": "CC-BY-4.0",
  "com.acme.description": { "shop_id": 88123 },
  "org.printfarm": { "slicer_profile": "PETG-0.2", "internal_id": 4 }
}
```

- **SolidCraft's keys are unprefixed** and hold text. Every other unprefixed key is reserved for
  later SolidCraft versions.
- **Other tools keep their data under one reverse-DNS key** of their own (`com.example.tool`:
  two or more dot-separated labels of letters, digits, `-` and `_`), holding an object. A tool
  writes only under its own key and leaves the others alone.
- **SolidCraft keeps every key as it is** through open, edit and save, in both the container
  and plain JSON, and never checks what another tool wrote: a well-known key holding something
  other than text (`"author": 42`) is kept, reads as absent, and never stops the design from
  opening. Typing over it in Design Properties replaces it.
- **Other tools' keys are never exported**; Design Properties lists them read-only.
- **Keep values small**: `document.metadata.set` refuses text over 4 KiB, an object over 64 KiB
  or nested more than 32 levels deep, and more than 256 keys or 256 KiB in all. Binary data
  belongs in a file next to the design.

Design properties are first saved in format 21. Inserting or linking another design doesn't
copy its properties; Save As keeps them. A command script of the design
(`timeline.export_script`) sets them in one step.

## Saving in the web app

In the web app, **Save** and **Save As** keep designs in this browser's private storage;
they do not open a dialog for choosing a folder on your computer. Clearing the site's data
can remove those designs. Download a copy to keep it outside the browser or move it to
another device.

1. Open the File menu and choose **Open** to show **Files in this browser**.
2. Choose **Download open design** for the current design, including its unsaved changes,
   or **Download** beside a stored file for its last saved contents.
3. Find the `.solidcraft` file in your browser's downloads. Whether a folder chooser appears
   depends on your browser's download settings.

Use **Open from computer…** in the same panel to reopen a downloaded design. The web app
works on a browser-stored copy; later saves do not update the original downloaded file.
Use **Rename** beside a stored file to change its name. Exports such as STEP and STL are
also downloaded.
