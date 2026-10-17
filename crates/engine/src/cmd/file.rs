//! Documents and files: new, open (designs and STEP), insert STEP, save, export.

#[path = "file_thumbnail.rs"]
mod thumbnail;

#[path = "file_import.rs"]
pub(crate) mod import;

use serde_json::{Value, json};
use solidcraft_doc::Document;
use solidcraft_io::Format;

use super::CommandSpec;
use crate::params::{bad, bool_, num, str_, string_list};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("file.new", "New Design", new_doc).icon("new").key("Ctrl+N").noundo().params("name?"),
    CommandSpec::new("doc.open", "Open", open)
        .icon("open")
        .key("Ctrl+O")
        .noundo()
        .params("path: .solidcraft design, or a .step/.stp/.igs/.fcstd/.3dm/.3mf/.stl/.obj/.glb/.gltf file (STEP, IGES, FreeCAD, Rhino, 3MF, STL, OBJ or glTF; opens as a new design holding the file as a component named after it, as file.insert_step and file.insert_mesh make); up?: z|y for a STEP, IGES, FreeCAD, Rhino or glTF file (see file.insert_step and file.insert_mesh); names?: solid|product|instance for a STEP, IGES, FreeCAD or Rhino file (see file.insert_step)"),
    CommandSpec::new("file.insert_step", "Insert STEP", insert_step)
        .at("SOLID", "INSERT")
        .icon("import")
        .params("path: STEP, IGES, FreeCAD .FCStd or Rhino .3dm file (its bodies join the design as an Import base feature in a new component named after the file, placed in the active component; an assembly's products become nested components, a repeated part one component with an occurrence per placement, all held as one rigid group until component.unlock → component, component_name, occurrence, subcomponents?, rigid_joints?); name? (the feature's and the component's); up?: z|y (default z: the file's axes are kept; y: the file has Y up, so the import's component is turned a quarter turn about X, Y becoming Z → stand_up: its occurrence); names?: solid|product|instance (what the bodies are named after: the solid's own name, else the product's (default); the name of the product (part) that holds the body; the name of the assembly occurrence (instance), else the product's)"),
    CommandSpec::new("file.insert_mesh", "Insert Mesh", insert_mesh)
        .at("SOLID", "INSERT")
        .icon("import")
        .params("path: .3mf, .stl, .obj, .glb or .gltf file (its meshes join the design as mesh bodies in a new component named after the file → component, component_name, occurrence; a glTF file's node tree becomes nested components held as one rigid group until component.unlock, a mesh placed again another occurrence, its materials the bodies' appearances, and its metres millimetres); name?; up?: z|y for a glTF file (default y: glTF has Y up, so the import's component is turned a quarter turn about X, Y becoming Z → stand_up: its occurrence; z: the file's axes are kept)"),
    CommandSpec::new("file.save", "Save", save)
        .icon("save")
        .key("Ctrl+S")
        .noundo()
        .params("path? (default: current file); format?: container|json (default: json for .json, otherwise container); compression?: fast|compact (default: fast); cache?: bool (warm native geometry, default: false); preview?: bool (saved display meshes, default: false)"),
    CommandSpec::new("file.save_as", "Save As", save_as)
        .icon("save")
        .noundo()
        .params("path; format?: container|json (default: json for .json, otherwise container); compression?: fast|compact (default: fast); cache?: bool (warm native geometry, default: false); preview?: bool (saved display meshes, default: false)"),
    CommandSpec::new("doc.recovery_list", "Recoverable Designs", recovery_list).icon("restore")
        .noundo()
        .params("dir? (default: the recovery folder) → designs autosaved by apps that crashed or closed with unsaved changes"),
    CommandSpec::new("doc.recover", "Recover Design", recover).icon("restore")
        .noundo()
        .params("id (from doc.recovery_list); dir?; discard?: bool (delete the entry once open) — opens it with its file path, unsaved"),
    CommandSpec::new("doc.recovery_discard", "Discard Recovered Design", recovery_discard).icon("delete").noundo().params("id, or all: true; dir?"),
    CommandSpec::new("file.export", "Export", export)
        .icon("export")
        .noundo()
        .params("path; format?: stl|stla|obj|step|3mf|3mf-tagged|urdf|json (default from extension; 3mf-tagged: 3MF whose triangles carry their B-rep face, at full precision, for meshing and analysis tools; docs/formats/3mf-face-tags.md; urdf: the whole assembly as a robot, with STL meshes written next to it; docs/urdf.md); bodies?: [names] (JSON and URDF export the complete design); urdf only: mesh_prefix? (before each mesh file name, e.g. package://robot/meshes/), effort? (default 1000), velocity? (default 10)"),
    CommandSpec::new("file.save_mesh", "Save As Mesh", save_mesh)
        .at("MESH", "MODIFY")
        .icon("mesh_export")
        .noundo()
        .params("path; format?: stl|3mf|3mf-tagged (default from the extension, else stl; 3mf-tagged: docs/formats/3mf-face-tags.md); bodies?: [names] (default: all); ascii?: bool (STL); slicer?: id, name or program path from file.slicers (open the saved file there)"),
    CommandSpec::new("file.slicers", "Installed Slicers", slicers).icon("export")
        .noundo()
        .params("→ the slicers found on this machine: [{id, name, icon?}]; pass an id or name as file.save_mesh's `slicer`"),
];

fn new_doc(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_(p, "name").unwrap_or("Untitled");
    s.replace_design(Session::new(Document::new(name)));
    Ok(json!({"name": name}))
}

fn path_arg<'a>(p: &'a Value, cmd: &str) -> Result<&'a str> {
    str_(p, "path").filter(|x| !x.trim().is_empty() && x.len() < 4096).ok_or_else(|| bad(cmd, "`path` is required"))
}

/// How a STEP or IGES file comes in beyond its path: which way is up in the file, and what its
/// bodies are named after.
struct StepOptions {
    /// `up: "y"`: the file has Y up, so the bodies it brings are turned a quarter turn about X
    /// (Y becomes Z), as the app's "Stand it up" offer does. `z` (the default) keeps the file's axes.
    y_up: bool,
    names: solidcraft_io::StepNames,
}

fn step_options(p: &Value, cmd: &str) -> Result<StepOptions> {
    let y_up = match p.get("up") {
        None | Some(Value::Null) => false,
        Some(v) => match v.as_str().map(|u| u.trim().to_ascii_lowercase()).as_deref() {
            Some("z") => false,
            Some("y") => true,
            _ => return Err(bad(cmd, "`up` must be \"z\" (the file's axes are kept) or \"y\" (the file has Y up: its Y becomes Z)")),
        },
    };
    let names = match p.get("names") {
        None | Some(Value::Null) => solidcraft_io::StepNames::default(),
        Some(v) => {
            v.as_str().and_then(solidcraft_io::StepNames::parse).ok_or_else(|| bad(cmd, "`names` must be \"solid\", \"product\" or \"instance\""))?
        }
    };
    Ok(StepOptions { y_up, names })
}

/// Read a STEP file as an Import feature.
fn read_step(path: &str, names: solidcraft_io::StepNames) -> Result<solidcraft_io::StepFeature> {
    let bytes = solidcraft_io::vfs::read(path, solidcraft_io::MAX_STEP_BYTES).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    solidcraft_io::step_import_feature_named(&bytes, path, names).map_err(|e| EngineError::Other(format!("{path}: {e}")))
}

/// A quarter turn about X through the origin: a file's Y up becomes Z up.
fn quarter_turn_x() -> solidcraft_doc::Mat {
    solidcraft_doc::rigid(solidcraft_geom::Vec3::ZERO, solidcraft_geom::Vec3::ZERO, solidcraft_geom::Vec3::X, std::f64::consts::FRAC_PI_2)
}

/// Add an Import feature for a STEP file to the session's design, in a new component named
/// after the file (#1386).
fn add_step(s: &mut Session, path: &str, name: Option<&str>, options: &StepOptions) -> Result<Value> {
    let sf = read_step(path, options.names)?;
    let fname = name.filter(|n| !n.trim().is_empty()).unwrap_or(&sf.name).to_string();
    let cname = import::component_name(path, name);
    let (id, placed) = import::add_in_component(s, sf.kind, &fname, sf.body_names, &cname, import::Tree::Step(&sf.import))?;
    // Y up: the import's component is turned a quarter turn about X (Y becomes Z), so an
    // assembly's parts and instances turn together.
    let turned = if options.y_up { turn_up(s, &placed)? } else { None };
    let created = created_bodies(s, id);
    let fname = s.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default();
    let mut out = json!({"path": path, "feature": id, "name": fname, "bodies": created, "created_bodies": created, "warnings": sf.warnings});
    placed.report(&mut out);
    if let Some(turned) = turned {
        out["stand_up"] = json!(turned);
    }
    Ok(out)
}

/// The bodies an import feature built, by name.
fn created_bodies(s: &Session, feature: u64) -> Vec<String> {
    s.model.state().bodies.iter().filter(|b| b.feature == feature).map(|b| b.name.clone()).collect()
}

/// Add a MeshImport feature for a 3MF, STL, OBJ or glTF file, in a new component named after the
/// file. A glTF file's nodes become components in it, and its materials the bodies' looks; it is
/// in metres with Y up, so unless `y_up` is false its component is turned to Z up (#1505).
fn add_mesh(s: &mut Session, path: &str, name: Option<&str>, y_up: Option<bool>) -> Result<Value> {
    let err = |e: solidcraft_io::IoError| EngineError::Other(format!("{path}: {e}"));
    let bytes = solidcraft_io::vfs::read(path, solidcraft_io::MAX_3MF_BYTES).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let gltf = solidcraft_io::is_gltf_path(path);
    let (mf, plan, looks) = if gltf {
        let dir = std::path::Path::new(path).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
        let mut load =
            |name: &str| solidcraft_io::vfs::read(&dir.join(name).to_string_lossy(), solidcraft_io::MAX_GLTF_BYTES).map_err(|e| e.to_string());
        let mut g = solidcraft_io::read_gltf(&bytes, path, &mut load).map_err(err)?;
        if !import::mesh_plan_fits(&g.plan) {
            g.flatten();
        }
        (g.feature, Some(g.plan), g.looks)
    } else {
        (solidcraft_io::mesh_import_feature(&bytes, path).map_err(err)?, None, Vec::new())
    };
    let fname = name.filter(|n| !n.trim().is_empty()).unwrap_or(&mf.name).to_string();
    let cname = import::component_name(path, name);
    let tree = match &plan {
        Some(p) if !p.places.is_empty() => import::Tree::Mesh(p),
        _ => import::Tree::Flat,
    };
    let (id, placed) = import::add_in_component(s, mf.kind, &fname, mf.body_names, &cname, tree)?;
    let bodies = created_bodies(s, id);
    // The materials' looks, body by body (the bodies are built in the file's order).
    if looks.iter().any(Option::is_some) && bodies.len() == looks.len() {
        for (body, look) in bodies.iter().zip(looks) {
            if let Some(look) = look {
                s.doc_mut().appearances.bodies.insert(body.clone(), look);
            }
        }
        s.refresh();
    }
    let turned = if gltf && y_up.unwrap_or(true) { turn_up(s, &placed)? } else { None };
    let fname = s.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default();
    let mut out = json!({"path": path, "feature": id, "name": fname, "bodies": bodies, "created_bodies": bodies, "warnings": mf.warnings});
    placed.report(&mut out);
    if let Some(turned) = turned {
        out["stand_up"] = json!(turned);
    }
    Ok(out)
}

/// Turn an import's component a quarter turn about X (the file's Y up becomes Z up), contents
/// and all. Returns its occurrence.
fn turn_up(s: &mut Session, placed: &import::Placed) -> Result<Option<u64>> {
    let occ = placed.occurrence.ok_or_else(|| EngineError::Other("the import has no placement to turn".into()))?;
    if let Some(o) = s.doc_mut().occurrences.iter_mut().find(|o| o.id == occ) {
        o.transform = solidcraft_doc::mat_mul(&quarter_turn_x(), &o.transform);
    }
    s.refresh();
    Ok(Some(occ))
}

/// `up` for a glTF file: `y` (the default: glTF is Y up) turns it to Z up, `z` keeps its axes.
/// Other mesh files take no `up`.
fn mesh_up(p: &Value, path: &str, cmd: &str) -> Result<Option<bool>> {
    if p.get("up").is_none_or(Value::is_null) {
        return Ok(None);
    }
    if !solidcraft_io::is_gltf_path(path) {
        return Err(bad(cmd, "`up` applies to STEP, IGES and glTF files only"));
    }
    Ok(Some(step_options(p, cmd)?.y_up))
}

fn insert_mesh(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "file.insert_mesh";
    let path = path_arg(p, cmd)?;
    if !solidcraft_io::is_mesh_path(path) {
        return Err(bad(cmd, "only 3MF, STL, OBJ and glTF files (.3mf, .stl, .obj, .glb, .gltf) can be inserted as meshes"));
    }
    let up = mesh_up(p, path, cmd)?;
    add_mesh(s, path, str_(p, "name"), up)
}

fn insert_step(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "file.insert_step";
    let path = path_arg(p, cmd)?;
    if !solidcraft_io::is_step_path(path) {
        return Err(bad(cmd, "only STEP, IGES, FreeCAD and Rhino files (.step, .stp, .igs, .iges, .fcstd, .3dm) can be inserted"));
    }
    let options = step_options(p, cmd)?;
    add_step(s, path, str_(p, "name"), &options)
}

fn open(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_arg(p, "doc.open")?;
    if !solidcraft_io::is_step_path(path) && p.get("names").is_some_and(|v| !v.is_null()) {
        return Err(bad("doc.open", "`names` applies to STEP and IGES files only"));
    }
    if !solidcraft_io::is_step_path(path) && !solidcraft_io::is_gltf_path(path) && p.get("up").is_some_and(|v| !v.is_null()) {
        return Err(bad("doc.open", "`up` applies to STEP, IGES and glTF files only"));
    }
    if solidcraft_io::is_step_path(path) || solidcraft_io::is_mesh_path(path) {
        // A new, unsaved design holding the file's bodies.
        let stem = std::path::Path::new(path).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
        let mut fresh = Session::new(Document::new(&stem));
        let r = if solidcraft_io::is_step_path(path) {
            add_step(&mut fresh, path, None, &step_options(p, "doc.open")?)?
        } else {
            add_mesh(&mut fresh, path, None, mesh_up(p, path, "doc.open")?)?
        };
        // The design is the file: its component is the one being worked in.
        if let Some(c) = r["component"].as_u64() {
            fresh.active_component = c;
            fresh.active_occurrence_path = Some(fresh.doc.first_path(c));
        }
        fresh.undo.clear();
        s.replace_design(fresh);
        let errors = s.model.results.iter().filter(|r| r.error.is_some()).count();
        let mut out = json!({"path": path, "features": s.doc.features.len(), "errors": errors, "bodies": r["bodies"], "warnings": r["warnings"]});
        for k in ["component", "component_name", "occurrence", "subcomponents", "rigid_joints", "stand_up"] {
            if let Some(v) = r.get(k) {
                out[k] = v.clone();
            }
        }
        return Ok(out);
    }
    let mut doc = solidcraft_io::read_design_file(path, s.open_preview.take()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    if let Some(stem) = default_name_for(&doc.name, path) {
        doc.name = stem;
    }
    s.replace_design(Session::new(doc));
    s.path = Some(path.to_string());
    let errors = s.model.results.iter().filter(|r| r.error.is_some()).count();
    Ok(json!({"path": path, "features": s.doc.features.len(), "errors": errors, "name": s.doc.name}))
}

/// The name a design never given one by its user is shown under.
const DEFAULT_NAME: &str = "Untitled";

/// A design still under the default name takes its file's name (`bracket.solidcraft` →
/// "bracket"); a name the user chose is kept. `None` when nothing changes.
fn default_name_for(name: &str, path: &str) -> Option<String> {
    let name = name.trim();
    if !name.is_empty() && name != DEFAULT_NAME {
        return None;
    }
    let stem = std::path::Path::new(path).file_stem()?.to_string_lossy().trim().to_string();
    (!stem.is_empty()).then_some(stem)
}

/// The name a design takes when saved to `path`, if it changes: its file's name while it has
/// none of its own ([`default_name_for`]), or while it still has the name it took from the file
/// it was saved as before, so Save As under another name renames it too (#1710). A name the
/// user chose is kept.
fn saved_name(name: &str, old_path: Option<&str>, path: &str) -> Option<String> {
    if let Some(stem) = default_name_for(name, path) {
        return Some(stem);
    }
    let stem = |p: &str| std::path::Path::new(p).file_stem().map(|x| x.to_string_lossy().trim().to_string()).filter(|x| !x.is_empty());
    let (old, new) = (stem(old_path?)?, stem(path)?);
    (name.trim() == old && new != old).then_some(new)
}

/// Rename the design, and the undo and redo states that still carry the old name, so undo
/// does not bring the default name back and leave the design looking changed.
fn rename_design(s: &mut Session, from: &str, to: &str) {
    s.doc_mut().name = to.to_string();
    for snap in s.undo.iter_mut().chain(s.redo.iter_mut()) {
        if snap.doc.name == from {
            std::sync::Arc::make_mut(&mut snap.doc).name = to.to_string();
        }
    }
}

fn write(s: &mut Session, path: &str, format: &str, compression: &str, preview: bool, cache: bool) -> Result<Value> {
    solidcraft_io::vfs::check(path).map_err(|e| EngineError::Other(e.to_string()))?;
    // Saving an unnamed design, or one named after the file it was saved as, names it after its
    // file (not an undoable edit).
    let renamed = saved_name(&s.doc.name, s.path.as_deref(), path).map(|stem| (s.doc.name.clone(), stem));
    if let Some((from, to)) = &renamed {
        rename_design(s, from, to);
    }
    let r = write_file(s, path, format, compression, preview, cache);
    if r.is_err()
        && let Some((from, to)) = &renamed
    {
        rename_design(s, to, from);
    }
    r
}

fn write_file(s: &mut Session, path: &str, format: &str, compression: &str, preview: bool, cache: bool) -> Result<Value> {
    let compression = match compression {
        "fast" => solidcraft_io::DesignCompression::Fast,
        "compact" => solidcraft_io::DesignCompression::Compact,
        _ => return Err(bad("file.save", "compression must be fast or compact")),
    };
    // Linked designs are kept relative to the folder the design is saved in.
    let links = super::component_link::rebased(&s.doc, s.path.as_deref(), path);
    let old_links = (links != s.doc.links).then(|| std::mem::replace(&mut s.doc_mut().links, links));
    let r = write_bytes(s, path, format, compression, preview, cache);
    if r.is_err()
        && let Some(old) = old_links
    {
        s.doc_mut().links = old;
    }
    r
}

fn write_bytes(
    s: &mut Session,
    path: &str,
    format: &str,
    compression: solidcraft_io::DesignCompression,
    preview: bool,
    cache: bool,
) -> Result<Value> {
    let bytes = match format {
        "container" => s.design_container(compression, preview, cache)?,
        "json" => solidcraft_io::write_design(&s.design()),
        _ => return Err(bad("file.save", "format must be container or json")),
    };
    // Atomic, keeping the version being replaced as `<file>.bak`.
    solidcraft_io::write_atomic(std::path::Path::new(path), &bytes, true).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    s.path = Some(path.to_string());
    s.mark_saved();
    Ok(json!({"path": path, "bytes": bytes.len(), "name": s.doc.name}))
}

impl Session {
    /// Container bytes for desktop and browser saves. Only already evaluated bodies
    /// and warm meshes are eligible; saving never forces evaluation or tessellation.
    pub fn design_container(&self, compression: solidcraft_io::DesignCompression, preview: bool, cache: bool) -> solidcraft_io::Result<Vec<u8>> {
        let geometry = cache.then(|| saved_geometry(self)).flatten();
        let preview = preview.then(|| saved_preview(self)).flatten();
        let thumbnail = cache.then(|| thumbnail::saved(self)).flatten();
        solidcraft_io::write_design_container_with_thumbnail(&self.design(), compression, preview.as_ref(), geometry.as_deref(), thumbnail.as_deref())
    }
}

fn saved_geometry(s: &Session) -> Option<Vec<solidcraft_io::DesignGeometry>> {
    if s.active_sketch.is_some() || !s.pending_moves.is_empty() || s.model.results.iter().any(|r| r.error.is_some()) {
        return None;
    }
    let world = s.cached_world_state()?;
    if world.bodies.is_empty() || world.bodies.len() > 4096 {
        return None;
    }
    let painted = s.doc.painted(&world);
    Some(painted.bodies.iter().map(|b| solidcraft_io::DesignGeometry { name: b.name.clone(), body: b.body.clone() }).collect())
}

/// Cache only warm display geometry. A save never waits for an unevaluated mesh.
fn saved_preview(s: &Session) -> Option<solidcraft_io::DesignPreview> {
    if s.active_sketch.is_some() || !s.pending_moves.is_empty() || s.model.results.iter().any(|r| r.error.is_some()) {
        return None;
    }
    let world = s.cached_world_state()?;
    if world.bodies.is_empty() || world.bodies.len() > 1024 {
        return None;
    }
    let mut preview = solidcraft_io::DesignPreview::default();
    let mut bytes = 0usize;
    let mut triangles = 0usize;
    for body in &world.bodies {
        let mesh = body.cached_mesh()?;
        bytes =
            bytes.saturating_add(mesh.positions.len().saturating_mul(24)).saturating_add(mesh.triangles.len().saturating_mul(12)).saturating_add(24);
        triangles = triangles.saturating_add(mesh.triangles.len());
        if bytes > 8 << 20 || triangles > 100_000 {
            return None;
        }
        preview.meshes.push(solidcraft_doc::MeshData {
            name: body.name.clone(),
            positions: mesh.positions.iter().flat_map(|p| [p.x, p.y, p.z]).collect(),
            triangles: mesh.triangles.iter().flatten().copied().collect(),
            color: s.doc.body_color(body),
        });
    }
    Some(preview)
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match str_(p, "path") {
        Some(x) if !x.trim().is_empty() => x.to_string(),
        _ => s.path.clone().ok_or_else(|| bad("file.save", "the design has no file yet: give a `path`"))?,
    };
    write(
        s,
        &path,
        str_(p, "format").unwrap_or(default_design_format(&path)),
        str_(p, "compression").unwrap_or("fast"),
        bool_(p, "preview").unwrap_or(false),
        bool_(p, "cache").unwrap_or(false),
    )
}

fn default_design_format(path: &str) -> &'static str {
    if std::path::Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("json")) { "json" } else { "container" }
}

fn save_as(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_arg(p, "file.save_as")?.to_string();
    write(
        s,
        &path,
        str_(p, "format").unwrap_or(default_design_format(&path)),
        str_(p, "compression").unwrap_or("fast"),
        bool_(p, "preview").unwrap_or(false),
        bool_(p, "cache").unwrap_or(false),
    )
}

fn export_to(s: &Session, path: &str, format: Format, bodies: &[String]) -> Result<Value> {
    let name = std::path::Path::new(path).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| s.doc.name.clone());
    // A design with components exports as a STEP assembly; otherwise bodies as placed.
    let bytes = if format == Format::Design {
        if !bodies.is_empty() {
            return Err(bad("file.export", "design JSON exports the complete document; omit bodies"));
        }
        solidcraft_io::write_design(&s.design())
    } else if format == Format::Step && bodies.is_empty() && !s.doc.occurrences.is_empty() {
        solidcraft_io::step_assembly(&s.doc, &s.doc.painted(&s.model.state()), &name)?
    } else {
        // Bodies carry the colour they are shown in (appearance, material or imported), and the
        // file the design's properties where the format has a place for them.
        solidcraft_io::export_with_metadata(&s.doc.painted(&s.world_state()), bodies, format, &name, &s.doc.metadata)?
    };
    solidcraft_io::vfs::write(path, &bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    Ok(json!({"path": path, "bytes": bytes.len(), "format": format!("{format:?}")}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_arg(p, "file.export")?;
    let format = Format::from_name(str_(p, "format").unwrap_or(path))?;
    if format == Format::Urdf {
        return export_urdf(s, path, p);
    }
    export_to(s, path, format, &string_list(p, "bodies"))
}

/// The assembly as a URDF and its meshes next to it (docs/urdf.md). Every path is checked before
/// anything is written; the meshes go first and the `.urdf` last.
fn export_urdf(s: &Session, path: &str, p: &Value) -> Result<Value> {
    let cmd = "file.export";
    if !string_list(p, "bodies").is_empty() {
        return Err(bad(cmd, "urdf exports the whole assembly; omit bodies"));
    }
    let positive = |k: &str| match p.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => num(p, k).filter(|x| *x > 0.0).map(Some).ok_or_else(|| bad(cmd, format!("`{k}` must be a positive number"))),
    };
    // `io::urdf` refuses a prefix XML cannot hold.
    let mesh_prefix = str_(p, "mesh_prefix").unwrap_or("").to_string();
    let options = solidcraft_io::UrdfOptions { mesh_prefix, effort: positive("effort")?, velocity: positive("velocity")? };
    let file = std::path::Path::new(path);
    let stem = file.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| s.doc.name.clone());
    // Occurrences where they are shown, uncaptured moves included, except those the joint solver
    // places: it puts them back as soon as a joint moves, so the move must not become part of
    // their links.
    let st = s.model.state();
    let mut held = Vec::new();
    let doc = if s.pending_moves.is_empty() {
        std::borrow::Cow::Borrowed(&*s.doc)
    } else {
        let placed = solidcraft_doc::joints::solve(&s.doc, &st).world;
        let mut doc = (*s.doc).clone();
        for o in &mut doc.occurrences {
            match s.pending_moves.get(&o.id) {
                Some(_) if placed.contains_key(&o.id) => held.push(o.name.clone()),
                Some(t) => o.transform = *t,
                None => {}
            }
        }
        std::borrow::Cow::Owned(doc)
    };
    let mut pkg = solidcraft_io::urdf(&doc, &doc.painted(&st), &stem, &options)?;
    for name in held {
        pkg.warnings.push(format!("`{name}` was moved but the move is not captured: exported where its joints place it"));
    }
    let dir = file.parent().unwrap_or(std::path::Path::new(""));
    let mut targets: Vec<(String, &[u8])> = pkg.meshes.iter().map(|(n, b)| (dir.join(n).to_string_lossy().to_string(), b.as_slice())).collect();
    targets.push((path.to_string(), pkg.xml.as_bytes()));
    for (t, _) in &targets {
        solidcraft_io::vfs::check(t).map_err(|e| EngineError::Other(format!("{t}: {e}")))?;
    }
    let mut written: Vec<String> = Vec::new();
    for (t, bytes) in &targets {
        if let Err(e) = solidcraft_io::vfs::write(t, bytes) {
            let before = if written.is_empty() { "none".to_string() } else { written.join(", ") };
            return Err(EngineError::Other(format!("{t}: {e} (this file may be incomplete; written before it: {before})")));
        }
        written.push(t.clone());
    }
    Ok(
        json!({"path": path, "format": "Urdf", "files": written, "links": pkg.model.links.len(), "joints": pkg.model.joints.len(), "warnings": pkg.warnings}),
    )
}

fn save_mesh(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "file.save_mesh";
    let path = path_arg(p, cmd)?;
    let format = match str_(p, "format").map(str::to_ascii_lowercase).as_deref() {
        None => match std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref() {
            Some("3mf") => Format::ThreeMf,
            _ => Format::StlBinary,
        },
        Some("stl") if bool_(p, "ascii").unwrap_or(false) => Format::StlAscii,
        Some("stl") => Format::StlBinary,
        Some("3mf") => Format::ThreeMf,
        Some("3mf-tagged") => Format::ThreeMfTagged,
        Some(other) => return Err(bad(cmd, format!("unknown mesh format `{other}` (stl, 3mf, 3mf-tagged)"))),
    };
    let slicer = match str_(p, "slicer") {
        None => None,
        Some(spec) => Some(
            crate::slicers::resolve(spec).ok_or_else(|| bad(cmd, format!("no slicer `{spec}` (see file.slicers, or give the path of a program)")))?,
        ),
    };
    let mut out = export_to(s, path, format, &string_list(p, "bodies"))?;
    if let (Some(slicer), Some(obj)) = (slicer, out.as_object_mut()) {
        // The file is saved either way; a slicer that won't start is reported, not fatal.
        match slicer.open(path) {
            Ok(()) => obj.insert("slicer".into(), json!(slicer.name)),
            Err(e) => obj.insert("slicer_error".into(), json!(e)),
        };
    }
    Ok(out)
}

fn slicers(_: &mut Session, _: &Value) -> Result<Value> {
    let list: Vec<Value> = crate::slicers::detect()
        .iter()
        .map(|d| json!({"id": d.id, "name": d.name, "icon": d.icon.as_ref().map(|i| i.path().to_string_lossy().into_owned())}))
        .collect();
    Ok(json!({ "slicers": list }))
}

fn recovery_dir(p: &Value, cmd: &str) -> Result<std::path::PathBuf> {
    let dir = match str_(p, "dir").filter(|d| !d.trim().is_empty() && d.len() < 4096) {
        Some(d) => d.into(),
        None => crate::recovery::default_dir().ok_or_else(|| bad(cmd, "no recovery folder on this system (give `dir`)"))?,
    };
    // An agent's commands reach the recovery folder only when it is one of their folders.
    solidcraft_io::vfs::check(&dir.to_string_lossy()).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(dir)
}

fn recovery_list(_s: &mut Session, p: &Value) -> Result<Value> {
    let dir = recovery_dir(p, "doc.recovery_list")?;
    Ok(json!({"dir": dir, "designs": crate::recovery::orphans(&dir)}))
}

fn recover(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "doc.recover";
    let dir = recovery_dir(p, cmd)?;
    let id = str_(p, "id").ok_or_else(|| bad(cmd, "`id` is required (see doc.recovery_list)"))?;
    let (mut doc, entry) = crate::recovery::load(&dir, id)?;
    if let Some(stem) = entry.path.as_deref().and_then(|path| default_name_for(&doc.name, path)) {
        doc.name = stem;
    }
    s.replace_design(Session::new(doc));
    s.path = entry.path.clone();
    s.mark_unsaved();
    if bool_(p, "discard").unwrap_or(false) {
        crate::recovery::remove(&dir, id)?;
    }
    let errors = s.model.results.iter().filter(|r| r.error.is_some()).count();
    Ok(json!({"id": id, "path": entry.path, "name": entry.name, "features": s.doc.features.len(), "errors": errors}))
}

fn recovery_discard(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "doc.recovery_discard";
    let dir = recovery_dir(p, cmd)?;
    let ids: Vec<String> = if bool_(p, "all").unwrap_or(false) {
        crate::recovery::orphans(&dir).into_iter().map(|e| e.id).collect()
    } else {
        vec![str_(p, "id").ok_or_else(|| bad(cmd, "`id` or `all` is required"))?.to_string()]
    };
    for id in &ids {
        crate::recovery::remove(&dir, id)?;
    }
    Ok(json!({"discarded": ids}))
}

#[cfg(test)]
#[path = "urdf_tests.rs"]
mod urdf_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual warmed engine save and provisional-open benchmark"]
    fn benchmark_warm_design_save_with_preview() {
        let target = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("target"));
        // The mesh storage fixture intentionally does not evaluate geometry. Use
        // original valid STEP boxes for the engine/display-cache measurement.
        let input = target.join("issue303-step.json");
        let doc = solidcraft_io::read_design(&std::fs::read(input).unwrap()).unwrap();
        let mut session = Session::new(doc);
        for body in &session.world_state().bodies {
            body.mesh();
        }
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..9 {
            for index in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let path = target.join(if index == 0 { "issue303-warm.json" } else { "issue303-warm.solidcraft" });
                let began = std::time::Instant::now();
                session.execute("file.save", &json!({"path":path,"preview":true,"cache":false})).unwrap();
                samples[index].push(began.elapsed().as_secs_f64() * 1000.0);
            }
        }
        for s in &mut samples {
            s.sort_by(f64::total_cmp);
        }
        let bytes = std::fs::read(target.join("issue303-warm.solidcraft")).unwrap();
        assert!(solidcraft_io::read_design_preview(&bytes).is_some());
        assert_eq!(solidcraft_io::read_design(&bytes).unwrap(), *session.doc);
        let mut reads = Vec::new();
        for _ in 0..9 {
            let began = std::time::Instant::now();
            assert!(solidcraft_io::read_design_preview(&bytes).is_some());
            reads.push(began.elapsed().as_secs_f64() * 1000.0);
        }
        reads.sort_by(f64::total_cmp);
        eprintln!(
            "warmed engine median atomic save ms json={:.3} container_with_preview={:.3}; preview_decode_ms={:.3}; container_bytes={}",
            samples[0][4],
            samples[1][4],
            reads[4],
            bytes.len()
        );
    }

    #[test]
    fn save_uses_only_warm_meshes_and_open_preview_does_not_replace_evaluation() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length":10,"width":20,"height":30})).unwrap();
        assert!(saved_preview(&s).is_none(), "save must not triangulate a cold body");
        let state = s.world_state();
        for body in &state.bodies {
            body.mesh();
        }
        let save_path = std::env::temp_dir().join(format!("solidcraft-preview-save-{}.solidcraft", std::process::id()));
        s.execute("file.save", &json!({"path":save_path})).unwrap();
        let saved = std::fs::read(&save_path).unwrap();
        assert!(solidcraft_io::read_design_geometry(&saved).is_none(), "ordinary saves omit native geometry caches");
        assert!(!saved.windows(b"cache/preview.bin".len()).any(|s| s == b"cache/preview.bin"), "ordinary saves omit redundant display meshes");
        s.execute("file.save", &json!({"path":save_path,"preview":true})).unwrap();
        assert!(solidcraft_io::read_design_preview(&std::fs::read(&save_path).unwrap()).is_some());
        let _ = std::fs::remove_file(&save_path);
        let _ = std::fs::remove_file(format!("{}.bak", save_path.display()));
        let mut preview = saved_preview(&s).unwrap();
        // A checksum-valid but forged cached shape must not become editable geometry.
        for mesh in &mut preview.meshes {
            for coordinate in &mut mesh.positions {
                *coordinate *= 10.0;
            }
        }
        let bytes = solidcraft_io::write_design_container_with_preview(&s.doc, solidcraft_io::DesignCompression::Fast, Some(&preview)).unwrap();
        let path = std::env::temp_dir().join(format!("solidcraft-preview-{}.solidcraft", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback_seen = seen.clone();
        let mut reopened = Session {
            open_preview: Some(Box::new(move |preview| {
                callback_seen.fetch_add(preview.meshes.len(), std::sync::atomic::Ordering::Relaxed);
            })),
            ..Session::default()
        };
        reopened.execute("doc.open", &json!({"path":path})).unwrap();
        assert_eq!(seen.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(reopened.open_preview.is_none());
        let measured = reopened.execute("inspect.measure", &json!({})).unwrap();
        assert!((measured["bodies"][0]["volume_mm3"].as_f64().unwrap() - 6000.0).abs() < 1e-6);
        assert_eq!(*reopened.doc, *s.doc);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn preview_saves_do_not_build_cold_placements_or_persist_transient_edits() {
        let mut s = Session::default();
        let created = s.execute("component.create", &json!({"name":"PreviewPart"})).unwrap();
        let occurrence = created["occurrence"].as_u64().unwrap();
        s.execute("solid.box", &json!({"length":2,"width":3,"height":4,"body_name":"Block","operation":"new"})).unwrap();
        for body in &s.model.state().bodies {
            body.mesh();
        }
        *s.world_cache.lock().unwrap() = None;
        assert!(saved_preview(&s).is_none());
        assert!(saved_geometry(&s).is_none());
        assert!(s.world_cache.lock().unwrap().is_none(), "saving must not prepare cold placements");
        for body in &s.world_state().bodies {
            body.mesh();
        }
        assert!(saved_preview(&s).is_some());
        s.execute("occurrence.move", &json!({"occurrence":occurrence,"translate":[10,0,0],"capture":false})).unwrap();
        for body in &s.world_state().bodies {
            body.mesh();
        }
        assert!(!s.pending_moves.is_empty());
        assert!(saved_preview(&s).is_none(), "uncaptured positions are not in the saved document");
        assert!(saved_geometry(&s).is_none());
        s.execute("component.capture_position", &json!({})).unwrap();
        for body in &s.world_state().bodies {
            body.mesh();
        }
        let preview = saved_preview(&s).unwrap();
        let min_x = preview.meshes.iter().flat_map(|mesh| mesh.positions.as_chunks::<3>().0.iter().map(|p| p[0])).fold(f64::INFINITY, f64::min);
        assert!((min_x - 10.0).abs() < 1e-9);
        let mut editing = s.scratch();
        editing.execute("sketch.create", &json!({"plane":"XY"})).unwrap();
        for body in &editing.world_state().bodies {
            body.mesh();
        }
        assert!(editing.active_sketch.is_some());
        assert!(saved_preview(&editing).is_none());
        assert!(saved_geometry(&editing).is_none());
    }

    #[test]
    fn save_and_save_as_require_explicit_native_cache_opt_in() {
        // Thumbnails are drawn by the renderer the host installs.
        crate::capture::install_test_renderer();
        let mut session = Session::new(Document::new("Cache opt-in"));
        session.execute("solid.box", &json!({"length":10,"width":20,"height":30})).unwrap();
        let world = session.world_state();
        for body in &world.bodies {
            body.mesh();
        }
        let path = std::env::temp_dir().join(format!("solidcraft-cache-opt-in-{}.solidcraft", std::process::id()));
        for command in ["file.save", "file.save_as"] {
            for cache in [None, Some(true), Some(false), None] {
                let mut params = json!({"path":path});
                if let Some(cache) = cache {
                    params["cache"] = json!(cache);
                }
                session.execute(command, &params).unwrap();
                let bytes = std::fs::read(&path).unwrap();
                assert_eq!(solidcraft_io::read_design_geometry(&bytes).is_some(), cache == Some(true), "{command}: {cache:?}");
                assert_eq!(solidcraft_io::read_design_thumbnail(&bytes).is_some(), cache == Some(true), "{command}: {cache:?}");
                assert!(
                    !bytes.windows(b"cache/preview.bin".len()).any(|entry| entry == b"cache/preview.bin"),
                    "display meshes require preview: true"
                );
                assert_eq!(solidcraft_io::read_design(&bytes).unwrap(), *session.doc);
            }
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(format!("{}.bak", path.display())).unwrap();
    }

    #[test]
    fn native_cache_does_not_force_meshes_or_supply_editing_state() {
        let mut session = Session::new(Document::new("Native cache authority"));
        session.execute("solid.box", &json!({"length":10,"width":20,"height":30})).unwrap();
        let world = session.world_state();
        assert!(world.bodies.iter().all(|b| b.cached_mesh().is_none()));
        let mut cached = saved_geometry(&session).unwrap();
        let bytes = session.design_container(solidcraft_io::DesignCompression::Fast, false, true).unwrap();
        assert!(world.bodies.iter().all(|b| b.cached_mesh().is_none()), "saving native geometry must not triangulate");
        assert!(solidcraft_io::read_design_geometry(&bytes).is_some());
        assert!(
            solidcraft_io::read_design_geometry(&session.design_container(solidcraft_io::DesignCompression::Fast, false, false).unwrap()).is_none()
        );
        cached[0].body = solidcraft_kernel::box_solid(solidcraft_geom::Vec3::ZERO, solidcraft_geom::Vec3::new(100., 100., 100.)).unwrap();
        let bytes =
            solidcraft_io::write_design_container_with_cache(&session.doc, solidcraft_io::DesignCompression::Fast, None, Some(&cached)).unwrap();
        let path = std::env::temp_dir().join(format!("solidcraft-native-forged-{}.solidcraft", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_seen = seen.clone();
        let mut reopened = Session {
            open_preview: Some(Box::new(move |p| {
                callback_seen.store(!p.meshes.is_empty(), std::sync::atomic::Ordering::Relaxed);
            })),
            ..Session::default()
        };
        reopened.execute("doc.open", &json!({"path":path})).unwrap();
        assert!(seen.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(reopened.doc, session.doc);
        let measured = reopened.execute("inspect.measure", &json!({})).unwrap();
        assert!((measured["bodies"][0]["volume_mm3"].as_f64().unwrap() - 6000.).abs() < 1e-6);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "manual native cache save benchmark; generate issue303-step.json first"]
    fn benchmark_native_cache_save() {
        let target = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("target"));
        let input = std::env::var_os("SOLIDCRAFT_BENCH_FILE").map(std::path::PathBuf::from).unwrap_or_else(|| target.join("issue303-step.json"));
        let mut session = Session::new(solidcraft_io::read_design(&std::fs::read(input).unwrap()).unwrap());
        for body in &session.world_state().bodies {
            body.mesh();
        }
        let modes = [("json", false, "fast"), ("container", false, "fast"), ("container", true, "fast"), ("container", true, "compact")];
        let mut samples = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for round in 0..9 {
            for i in if round % 2 == 0 { [0, 1, 2, 3] } else { [3, 2, 1, 0] } {
                let (format, cache, compression) = modes[i];
                let path = target.join(format!("issue303-native-{i}.solidcraft"));
                let start = std::time::Instant::now();
                session.execute("file.save", &json!({"path":path,"format":format,"cache":cache,"compression":compression})).unwrap();
                samples[i].push(start.elapsed().as_secs_f64() * 1000.);
            }
        }
        for (i, times) in samples.iter_mut().enumerate() {
            times.sort_by(f64::total_cmp);
            let bytes = std::fs::read(target.join(format!("issue303-native-{i}.solidcraft"))).unwrap();
            assert_eq!(solidcraft_io::read_design(&bytes).unwrap(), *session.doc);
            assert_eq!(solidcraft_io::read_design_geometry(&bytes).is_some(), modes[i].1);
            eprintln!("native_save mode={i} median_ms={:.3} bytes={}", times[4], bytes.len());
        }
    }

    #[test]
    #[ignore = "manual fresh-process opening benchmark; one encoding per process"]
    fn benchmark_native_cache_open() {
        let target = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("target"));
        let mode = std::env::var("SOLIDCRAFT_BENCH_ENCODING").unwrap_or_else(|_| "2".into());
        let path = target.join(format!("issue303-native-{mode}.solidcraft"));
        let display = std::sync::Arc::new(std::sync::Mutex::new(None));
        let notify = display.clone();
        let start = std::time::Instant::now();
        let mut session = Session {
            open_preview: Some(Box::new(move |_| {
                *notify.lock().unwrap() = Some(start.elapsed().as_secs_f64() * 1000.);
            })),
            ..Session::default()
        };
        let result = session.execute("doc.open", &json!({"path":path})).unwrap();
        assert_eq!(result["errors"], 0);
        let reconstructed = start.elapsed().as_secs_f64() * 1000.;
        let mut triangles = 0;
        let world = session.world_state();
        for body in &world.bodies {
            triangles += body.mesh().triangles.len();
        }
        let ready = start.elapsed().as_secs_f64() * 1000.;
        eprintln!(
            "native_open mode={mode} provisional_ms={:?} rebuilt_ms={reconstructed:.3} meshes_ready_ms={ready:.3} bodies={} triangles={triangles}",
            *display.lock().unwrap(),
            world.bodies.len()
        );
    }

    fn name_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("solidcraft-name-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_as_names_an_untitled_design_after_its_file() {
        let dir = name_dir("untitled");
        let path = dir.join("bracket.solidcraft");
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length":10,"width":20,"height":30})).unwrap();
        assert_eq!(s.doc.name, "Untitled");
        let r = s.execute("file.save_as", &json!({"path":path})).unwrap();
        assert_eq!(r["name"], "bracket");
        assert_eq!(s.doc.name, "bracket");
        assert!(!s.is_dirty(), "taking the file's name is part of the save");
        assert_eq!(solidcraft_io::read_design(&std::fs::read(&path).unwrap()).unwrap().name, "bracket");
        // Undoing the box does not bring the old name back.
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.doc.name, "bracket");
        // Saved under another file, a name taken from the old file follows it (#1710), in the
        // file too, so reopening shows it.
        let copy = dir.join("copy.solidcraft");
        s.execute("file.save_as", &json!({"path":copy})).unwrap();
        assert_eq!(s.doc.name, "copy");
        let mut reopened = Session::default();
        reopened.execute("doc.open", &json!({"path":copy})).unwrap();
        assert_eq!(reopened.doc.name, "copy");
        // Saving again to the same file changes nothing.
        s.execute("file.save", &json!({})).unwrap();
        assert_eq!(s.doc.name, "copy");
        // As in the issue: a design opened from its file, then saved as another.
        reopened.execute("file.save_as", &json!({"path":dir.join("third.solidcraft")})).unwrap();
        assert_eq!(reopened.doc.name, "third");
        // A failed save leaves the name alone.
        let mut failed = Session::default();
        assert!(failed.execute("file.save_as", &json!({"path":dir.join("missing-dir").join("plate.solidcraft")})).is_err());
        assert_eq!(failed.doc.name, "Untitled");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_as_keeps_a_name_the_user_chose() {
        let dir = name_dir("named");
        let mut s = Session::default();
        s.execute("file.new", &json!({"name":"Gearbox lid"})).unwrap();
        s.execute("file.save_as", &json!({"path":dir.join("lid-v2.solidcraft")})).unwrap();
        assert_eq!(s.doc.name, "Gearbox lid");
        let mut reopened = Session::default();
        reopened.execute("doc.open", &json!({"path":dir.join("lid-v2.solidcraft")})).unwrap();
        assert_eq!(reopened.doc.name, "Gearbox lid");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn opening_an_untitled_file_shows_its_file_name() {
        let dir = name_dir("open");
        for (stored, file) in [("Untitled", "hinge.solidcraft"), ("", "spacer.json")] {
            let path = dir.join(file);
            let bytes = solidcraft_io::write_design(&Document::new(stored));
            std::fs::write(&path, bytes).unwrap();
            let mut s = Session::default();
            let r = s.execute("doc.open", &json!({"path":path})).unwrap();
            let stem = file.split('.').next().unwrap();
            assert_eq!(s.doc.name, stem);
            assert_eq!(r["name"], stem);
            assert!(!s.is_dirty(), "showing the file's name is not a change to save");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn design_save_formats_reopen_and_preserve_atomic_backups() {
        let dir = std::env::temp_dir().join(format!("solidcraft-container-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let packaged = dir.join("design.solidcraft");
        let plain = dir.join("design.json");
        let export = dir.join("export.json");
        let mut session = Session::default();
        session.execute("solid.box", &json!({"length":10,"width":20,"height":30})).unwrap();
        session.execute("file.save_as", &json!({"path":packaged})).unwrap();
        let original = std::fs::read(&packaged).unwrap();
        assert!(solidcraft_io::is_design_container(&original));
        session.execute("file.save_as", &json!({"path":plain})).unwrap();
        assert!(std::fs::read_to_string(&plain).unwrap().contains("solidcraft/"));
        let path = session.path.clone();
        session.execute("file.export", &json!({"path":export,"format":"json"})).unwrap();
        assert_eq!(session.path, path);
        for file in [&packaged, &plain, &export] {
            let mut reopened = Session::default();
            reopened.execute("doc.open", &json!({"path":file})).unwrap();
            assert_eq!(reopened.doc, session.doc);
            let measure = reopened.execute("inspect.measure", &json!({})).unwrap();
            assert_eq!(measure["body_count"], 1);
            assert!((measure["bodies"][0]["volume_mm3"].as_f64().unwrap() - 6000.0).abs() < 1e-6);
        }
        session.execute("file.save", &json!({"path":packaged})).unwrap();
        assert_eq!(std::fs::read(solidcraft_io::backup_path(&packaged)).unwrap(), original);
        let before = std::fs::read(&packaged).unwrap();
        assert!(session.execute("file.save", &json!({"path":packaged,"format":"invalid"})).is_err());
        assert_eq!(std::fs::read(&packaged).unwrap(), before);
        session.execute("file.save", &json!({"path":packaged,"format":"json"})).unwrap();
        assert!(std::fs::read_to_string(&packaged).unwrap().contains("solidcraft/"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A watched open reports the timeline's progress; cancelled, it skips the rebuild.
    #[test]
    fn watched_open_reports_progress_and_stops_when_cancelled() {
        use solidcraft_doc::progress::{Progress, watch};
        use std::sync::Arc;
        let dir = std::env::temp_dir().join(format!("solidcraft-watched-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("three.solidcraft").to_string_lossy().to_string();
        let mut s = Session::default();
        for x in [0, 20, 40] {
            s.execute("solid.box", &json!({"length":10,"width":10,"height":10,"corner":[x,0,0]})).unwrap();
        }
        s.execute("file.save", &json!({"path":path})).unwrap();

        let progress = Arc::new(Progress::default());
        let mut opened = Session::default();
        watch(progress.clone(), || opened.execute("doc.open", &json!({"path":path}))).unwrap();
        assert_eq!(progress.features(), (3, 3));
        assert_eq!(opened.world_state().bodies.len(), 3);

        let progress = Arc::new(Progress::default());
        progress.cancel();
        let mut stopped = Session::default();
        watch(progress, || stopped.execute("doc.open", &json!({"path":path}))).unwrap();
        assert!(stopped.model.results.iter().all(|r| r.skipped), "no feature was rebuilt after Cancel");
        assert!(stopped.world_state().bodies.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
