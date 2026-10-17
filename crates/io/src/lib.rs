//! File formats. Designs use JSON or versioned containers (`.solidcraft`); the authoritative
//! document rebuilds on load and optional display caches remain provisional. Bodies export
//! as STL (binary or ASCII), OBJ, STEP and 3MF, and assemblies as URDF ([`urdf()`]); STEP, IGES,
//! FreeCAD and Rhino files import as a base feature (see [`step_import_feature`]), 3MF and
//! STL files as mesh bodies (see [`mesh_import_feature`]).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod catalog;
mod design;
mod design_metadata;
mod dxf;
mod dxf_out;
pub use design::{
    DesignCompression, DesignContainerReader, DesignGeometry, DesignPreview, THUMBNAIL_SIZE, is_design_container, read_design_file,
    read_design_geometry, read_design_preview, read_design_thumbnail, read_design_thumbnail_file, write_design_container,
    write_design_container_with_cache, write_design_container_with_compression, write_design_container_with_preview,
    write_design_container_with_thumbnail,
};
mod legacy_dir;
#[cfg(test)]
mod mutate;
mod native_cad;
pub use native_cad::{NATIVE_CAD_EXTS, is_native_cad_path, native_cad_to_step};
pub mod safe_write;
mod sketch2d;
mod svg;
mod threemf;
pub mod urdf;
pub mod vfs;

pub use dxf::{read_dxf, write_dxf};
pub use dxf_out::{DxfEntity, DxfLayer, DxfLinetype, DxfShape, loop_shapes, seg_shape, write_dxf_drawing};
pub use legacy_dir::{APP_ID, LEGACY_APP_ID, adopt_legacy_dir, legacy_flatpak_path};
pub use safe_write::{backup_path, write_atomic};
pub use sketch2d::{Geom2, MAX_DRAWING_BYTES};
pub use svg::read_svg;
mod gltf_in;
pub use gltf_in::{GltfImport, Loader, MAX_GLTF_BYTES, MeshPlan, PlanPlace, glb_container, is_gltf_path, read_gltf};
mod obj;
pub use obj::read_obj;
pub use threemf::{MAX_3MF_BYTES, MeshObject, model_xml, model_xml_with, read_3mf, weld, write_3mf, write_3mf_with};
pub use urdf::{UrdfOptions, UrdfPackage, urdf};

use solidcraft_doc::{Document, Metadata, ModelState};
use solidcraft_geom::{Mesh, Vec3};

/// Design file extension.
pub const DESIGN_EXT: &str = "solidcraft";
/// The design extension of earlier releases. Compatibility shim (docs/compatibility.md): such
/// files still open everywhere a design opens; new designs are saved as [`DESIGN_EXT`].
pub const LEGACY_DESIGN_EXT: &str = "solvecraft";
/// Every extension a design file may have, current first.
pub const DESIGN_EXTS: [&str; 2] = [DESIGN_EXT, LEGACY_DESIGN_EXT];

/// Is this a design file name (`.solidcraft`, or the legacy extension), any case?
pub fn is_design_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| DESIGN_EXTS.iter().chain(["json"].iter()).any(|x| e.eq_ignore_ascii_case(x)))
}
/// Largest design file we read.
pub const MAX_DESIGN_BYTES: usize = 256 << 20;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("unsupported format `{0}` (use stl, obj, step, 3mf, urdf or solidcraft)")]
    Format(String),
    #[error("nothing to export: the design has no bodies")]
    Empty,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Kernel(#[from] solidcraft_kernel::KernelError),
    #[error(transparent)]
    Doc(#[from] solidcraft_doc::DocError),
}

pub type Result<T> = std::result::Result<T, IoError>;

/// Largest STEP file we read.
pub const MAX_STEP_BYTES: usize = 512 << 20;

/// Is this a CAD file read as an Import base feature: STEP (`.step` / `.stp`), IGES
/// (`.igs` / `.iges`) or a format read through cadmpeg ([`is_native_cad_path`]), any case?
pub fn is_step_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["step", "stp", "igs", "iges"].iter().any(|x| e.eq_ignore_ascii_case(x)))
        || is_native_cad_path(path)
}

/// Is this an IGES file name (`.igs` / `.iges`, any case)?
pub fn is_iges_path(path: &str) -> bool {
    std::path::Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("igs") || e.eq_ignore_ascii_case("iges"))
}

/// What the bodies of an imported STEP file are named after. Files from different CAD systems
/// fill the solid's own name with different things (the part's name, the last feature's name,
/// nothing), so the caller chooses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StepNames {
    /// The solid's own name in the file, else the name of the product that holds it.
    #[default]
    Solid,
    /// The name of the product (part) that holds the body, else the solid's name.
    Product,
    /// The name of the assembly occurrence (instance) that holds the body, else the product's
    /// name (a body of a root part has no occurrence), else the solid's.
    Instance,
}

impl StepNames {
    /// `solid`, `product` or `instance` (any case); `None` for anything else.
    pub fn parse(s: &str) -> Option<StepNames> {
        match s.trim().to_ascii_lowercase().as_str() {
            "solid" => Some(StepNames::Solid),
            "product" => Some(StepNames::Product),
            "instance" => Some(StepNames::Instance),
            _ => None,
        }
    }
}

/// A name that says nothing about its body: empty, or the placeholder a file or the importer
/// gives to something it has no name for.
fn generic_name(n: &str) -> bool {
    n.trim().is_empty() || n.starts_with('(') || n.eq_ignore_ascii_case("import") || n.eq_ignore_ascii_case("unnamed")
}

/// The name an imported body gets under the `names` choice; the solid's name when the chosen
/// one is missing or says nothing.
fn body_name(b: &solidcraft_kernel::ImportedBody, names: StepNames) -> String {
    let usable = |n: &&String| !generic_name(n);
    let chosen = match names {
        StepNames::Solid => None,
        StepNames::Product => b.path.last().filter(usable),
        StepNames::Instance => b.instance.as_ref().filter(usable).or(b.path.last().filter(usable)),
    };
    chosen.cloned().unwrap_or_else(|| b.name.clone())
}

/// A STEP file read as an import feature.
pub struct StepFeature {
    /// Timeline name: the root product's name, else the file name.
    pub name: String,
    pub kind: solidcraft_doc::FeatureKind,
    /// Body names from the file, as [`StepNames`] chose them (by default the solid names, else
    /// the product names).
    pub body_names: Vec<String>,
    pub warnings: Vec<String>,
    /// The file as read. Hold it until the feature has been evaluated: the timeline then reuses
    /// it instead of reading the file again (it is freed with the last holder).
    pub import: std::sync::Arc<solidcraft_kernel::StepImport>,
}

/// Read STEP bytes (`file` is the source file name) into an Import base feature. The file is
/// read once here to fail early and collect names; the feature holds the STEP text.
pub fn step_import_feature(bytes: &[u8], file: &str) -> Result<StepFeature> {
    step_import_feature_named(bytes, file, StepNames::default())
}

/// [`step_import_feature`], with the bodies named after `names`.
pub fn step_import_feature_named(bytes: &[u8], file: &str, names: StepNames) -> Result<StepFeature> {
    if bytes.len() > MAX_STEP_BYTES {
        return Err(IoError::Invalid(format!("STEP file too large ({} MB, limit {} MB)", bytes.len() >> 20, MAX_STEP_BYTES >> 20)));
    }
    // STEP is 7-bit text with escapes; tolerate stray 8-bit bytes. IGES and the cadmpeg formats
    // are restated as STEP (the feature keeps that STEP text).
    let (text, iges_warnings) = if is_native_cad_path(file) {
        native_cad_to_step(bytes, file).map_err(IoError::Invalid)?
    } else {
        let text = String::from_utf8_lossy(bytes).into_owned();
        if is_iges_path(file) { solidcraft_kernel::iges_to_step(&text).map_err(IoError::Invalid)? } else { (text, Vec::new()) }
    };
    let imp = solidcraft_kernel::step_import_shared(&text)?;
    let stem = std::path::Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Import".into());
    let body_names: Vec<String> = imp.bodies.iter().map(|b| body_name(b, names)).collect();
    // The root product's name; else the only body's name; else the file name.
    let name = match (imp.tree.as_slice(), body_names.as_slice()) {
        ([root], _) if !generic_name(&root.name) => root.name.clone(),
        (_, [only]) if !generic_name(only) => only.clone(),
        _ => stem,
    };
    let file_name = std::path::Path::new(file).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| file.to_string());
    Ok(StepFeature {
        name,
        body_names,
        kind: solidcraft_doc::FeatureKind::Import { file: file_name, step: text.into(), components: imp.tree.clone(), skip: Vec::new() },
        warnings: iges_warnings.into_iter().chain(imp.warnings.iter().cloned()).collect(),
        import: imp,
    })
}

/// Is this a mesh file we import (`.3mf`, `.stl`, `.obj`, `.glb`, `.gltf`, any case)?
pub fn is_mesh_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["3mf", "stl", "obj", "glb", "gltf"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// A 3MF, STL, OBJ or glTF file read as a mesh import feature.
pub struct MeshFeature {
    /// Timeline name: the only mesh's name, else the file name.
    pub name: String,
    pub kind: solidcraft_doc::FeatureKind,
    pub body_names: Vec<String>,
    pub warnings: Vec<String>,
}

/// Read 3MF, STL, OBJ or glTF bytes (the format from `file`'s extension) into a MeshImport
/// feature. A glTF file's bodies are all built where they are (see [`read_gltf`] for its node
/// tree), and it can't read buffers in other files.
pub fn mesh_import_feature(bytes: &[u8], file: &str) -> Result<MeshFeature> {
    if is_gltf_path(file) {
        let mut no_files = |name: &str| Err(format!("`{name}` can't be read here"));
        let mut g = read_gltf(bytes, file, &mut no_files)?;
        g.flatten();
        return Ok(g.feature);
    }
    if bytes.len() > MAX_3MF_BYTES {
        return Err(IoError::Invalid(format!("mesh file too large ({} MB)", bytes.len() >> 20)));
    }
    let path = std::path::Path::new(file);
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Mesh".into());
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let (objects, warnings) = if ext == "obj" {
        obj::read_obj(bytes, &stem)?
    } else if ext == "stl" {
        let tris = read_stl(bytes)?;
        if tris.is_empty() {
            return Err(IoError::Invalid("STL has no triangles".into()));
        }
        let positions: Vec<Vec3> = tris.iter().flatten().map(|v| Vec3::new(v[0] as f64, v[1] as f64, v[2] as f64)).collect();
        let n = u32::try_from(tris.len()).map_err(|_| IoError::Invalid("STL too large".into()))?;
        let triangles = (0..n).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect();
        (vec![MeshObject { name: stem.clone(), positions, triangles, ..Default::default() }], Vec::new())
    } else {
        read_3mf(bytes)?
    };
    // Check every mesh now, so the command fails rather than the timeline.
    let mut meshes = Vec::with_capacity(objects.len());
    for o in &objects {
        solidcraft_kernel::mesh_body(&o.positions, &o.triangles).map_err(|e| IoError::Invalid(format!("mesh `{}`: {e}", o.name)))?;
        meshes.push(solidcraft_doc::MeshData {
            name: o.name.clone(),
            positions: o.positions.iter().flat_map(|p| [p.x, p.y, p.z]).collect(),
            triangles: o.triangles.iter().flatten().copied().collect(),
            color: o.color,
        });
    }
    let name = match objects.as_slice() {
        [only] if !only.name.trim().is_empty() => only.name.clone(),
        _ => stem,
    };
    let file_name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| file.to_string());
    Ok(MeshFeature {
        name,
        body_names: objects.iter().map(|o| o.name.clone()).collect(),
        kind: solidcraft_doc::FeatureKind::MeshImport { file: file_name, meshes },
        warnings,
    })
}

/// Export formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    StlBinary,
    StlAscii,
    Obj,
    Step,
    Iges,
    ThreeMf,
    /// 3MF whose triangles carry their B-rep face, at full precision, for meshing and analysis
    /// tools (docs/formats/3mf-face-tags.md). Chosen by name only; a
    /// `.3mf` path alone means [`Format::ThreeMf`].
    ThreeMfTagged,
    Design,
    /// A robot description of the assembly with its mesh files ([`urdf()`]); written by the
    /// engine, since it is several files.
    Urdf,
}

impl Format {
    /// From a file name or extension (`part.stl`, `step`, `stp`…).
    pub fn from_name(name: &str) -> Result<Format> {
        let ext = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
        Ok(match ext.as_str() {
            "stl" => Format::StlBinary,
            "stla" | "stl-ascii" => Format::StlAscii,
            "obj" => Format::Obj,
            "step" | "stp" => Format::Step,
            "igs" | "iges" => Format::Iges,
            "3mf" => Format::ThreeMf,
            "3mf-tagged" => Format::ThreeMfTagged,
            "urdf" => Format::Urdf,
            "solidcraft" | "solvecraft" | "json" => Format::Design,
            _ => return Err(IoError::Format(ext)),
        })
    }
}

/// Fine export mesh for a body (chord tolerance 2e-4 of its size).
pub fn export_mesh(b: &solidcraft_kernel::Body) -> Result<Mesh> {
    Ok(b.tessellate((b.size() * 2e-4).max(1e-3))?)
}

/// Distance within which an exported mesh's vertices are welded (STL and 3MF alike).
fn weld_eps(b: &solidcraft_kernel::Body) -> f64 {
    (b.size() * 1e-6).max(1e-7)
}

/// Binary STL of several meshes (one solid).
pub fn stl_binary(meshes: &[Mesh], name: &str) -> Vec<u8> {
    let n: usize = meshes.iter().map(|m| m.triangles.len()).sum();
    let mut out = Vec::with_capacity(84 + n * 50);
    let mut header = format!("SolidCraft STL {name}").into_bytes();
    header.resize(80, b' ');
    out.extend_from_slice(&header);
    out.extend_from_slice(&u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes());
    for m in meshes {
        for t in &m.triangles {
            let Some([a, b, c]) = m.tri(t) else { continue };
            let nrm = (b - a).cross(c - a).normalized().unwrap_or_default();
            for v in [nrm, a, b, c] {
                for x in v.to_f32() {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            out.extend_from_slice(&[0, 0]);
        }
    }
    out
}

/// ASCII STL.
pub fn stl_ascii(meshes: &[Mesh], name: &str) -> String {
    let name: String = name.chars().filter(|c| c.is_ascii_graphic()).collect();
    let mut s = format!("solid {name}\n");
    for m in meshes {
        for t in &m.triangles {
            let Some([a, b, c]) = m.tri(t) else { continue };
            let n = (b - a).cross(c - a).normalized().unwrap_or_default();
            s += &format!("  facet normal {:e} {:e} {:e}\n    outer loop\n", n.x, n.y, n.z);
            for v in [a, b, c] {
                s += &format!("      vertex {:e} {:e} {:e}\n", v.x, v.y, v.z);
            }
            s += "    endloop\n  endfacet\n";
        }
    }
    s += &format!("endsolid {name}\n");
    s
}

/// Wavefront OBJ, one object per body.
pub fn obj(meshes: &[(String, Mesh)]) -> String {
    let mut s = String::from("# SolidCraft OBJ export (mm)\n");
    let mut base = 1usize;
    for (name, m) in meshes {
        s += &format!("o {}\n", name.replace(char::is_whitespace, "_"));
        for p in &m.positions {
            s += &format!("v {} {} {}\n", p.x, p.y, p.z);
        }
        for n in &m.normals {
            s += &format!("vn {} {} {}\n", n.x, n.y, n.z);
        }
        for t in &m.triangles {
            let [a, b, c] = t.map(|i| i as usize + base);
            s += &format!("f {a}//{a} {b}//{b} {c}//{c}\n");
        }
        base += m.positions.len();
    }
    s
}

/// Export the given bodies of a model (all when `bodies` is empty) in a format.
pub fn export(state: &ModelState, bodies: &[String], format: Format, name: &str) -> Result<Vec<u8>> {
    export_with_metadata(state, bodies, format, name, &Metadata::default())
}

/// The STEP or IGES header of an export named `file_name`: the design's author, organization
/// and description (#1015).
fn step_header(file_name: String, meta: &Metadata) -> solidcraft_kernel::StepHeader {
    solidcraft_kernel::StepHeader {
        file_name,
        author: header_line(meta.author()),
        organization: header_line(meta.organization()),
        description: header_line(meta.description()),
        ..Default::default()
    }
}

/// A header field is one line: each run of white space (line breaks too) becomes one space.
fn header_line(text: Option<&str>) -> String {
    text.map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

/// 3MF model metadata from the design properties, by the names the 3MF core specification
/// gives them. Other tools' keys are never exported.
fn threemf_metadata(meta: &Metadata) -> Vec<(&'static str, &str)> {
    [("Title", meta.project()), ("Designer", meta.author()), ("Description", meta.description()), ("LicenseTerms", meta.license())]
        .into_iter()
        .filter_map(|(name, text)| Some((name, text?)))
        .collect()
}

/// [`export`], with the design properties `meta` in the formats that have a place for them:
/// STEP and IGES headers (author, organization; STEP also the description) and 3MF model
/// metadata (project as the title, author as the designer, description, licence).
pub fn export_with_metadata(state: &ModelState, bodies: &[String], format: Format, name: &str, meta: &Metadata) -> Result<Vec<u8>> {
    let sel: Vec<&solidcraft_doc::ModelBody> = state.bodies.iter().filter(|b| bodies.is_empty() || bodies.contains(&b.name)).collect();
    if sel.is_empty() {
        return Err(IoError::Empty);
    }
    Ok(match format {
        Format::StlBinary | Format::StlAscii => {
            // Each body welded on its own (as for 3MF), so shared vertices are written identically
            // and the file is closed where faces meet.
            let meshes: Vec<Mesh> = sel
                .iter()
                .map(|b| {
                    let m = export_mesh(&b.body)?;
                    let (positions, triangles) = weld(&m.positions, &m.triangles, weld_eps(&b.body));
                    Ok(Mesh { positions, triangles, ..Mesh::default() })
                })
                .collect::<Result<_>>()?;
            if format == Format::StlBinary { stl_binary(&meshes, name) } else { stl_ascii(&meshes, name).into_bytes() }
        }
        Format::Obj => {
            let meshes: Vec<(String, Mesh)> = sel.iter().map(|b| Ok((b.name.clone(), export_mesh(&b.body)?))).collect::<Result<_>>()?;
            obj(&meshes).into_bytes()
        }
        Format::Step => {
            let bs: Vec<solidcraft_kernel::ExportBody> =
                sel.iter().map(|b| solidcraft_kernel::ExportBody { name: b.name.clone(), body: &b.body, color: b.body.color() }).collect();
            solidcraft_kernel::step_export_bodies(&bs, &step_header(format!("{name}.step"), meta))?.into_bytes()
        }
        Format::Iges => {
            let bs: Vec<solidcraft_kernel::ExportBody> =
                sel.iter().map(|b| solidcraft_kernel::ExportBody { name: b.name.clone(), body: &b.body, color: b.body.color() }).collect();
            solidcraft_kernel::iges_export_bodies(&bs, &step_header(format!("{name}.igs"), meta))?.into_bytes()
        }
        Format::ThreeMf | Format::ThreeMfTagged => {
            let objs: Vec<MeshObject> = sel.iter().map(|b| mesh_object(b, format == Format::ThreeMfTagged)).collect::<Result<_>>()?;
            write_3mf_with(&objs, &threemf_metadata(meta))?
        }
        Format::Design => return Err(IoError::Format("solidcraft (save the document instead)".into())),
        Format::Urdf => return Err(IoError::Format("urdf (an assembly package; use file.export)".into())),
    })
}

/// A body as a 3MF mesh: welded watertight (T-junctions split), coloured, and with `tagged`
/// each triangle keeps the B-rep face it came from.
fn mesh_object(b: &solidcraft_doc::ModelBody, tagged: bool) -> Result<MeshObject> {
    let m = export_mesh(&b.body)?;
    let eps = weld_eps(&b.body);
    // Faces with colours of their own tag their triangles.
    let paint = b.body.paint();
    let mut looks = Vec::new();
    let mut by_face = std::collections::HashMap::new();
    for f in paint.map(|p| p.faces.as_slice()).unwrap_or_default() {
        looks.push((f.color, 1.0 - f.opacity));
        by_face.insert(f.face, looks.len() as u32);
    }
    // Weld with each triangle's own index as its tag: the pieces of a split triangle keep it,
    // and its colour and face follow from it.
    let n = u32::try_from(m.triangles.len()).map_err(|_| IoError::Invalid("mesh too large".into()))?;
    let (positions, triangles, origin) = threemf::weld_tagged(&m.positions, &m.triangles, &(0..n).collect::<Vec<_>>(), eps);
    let face_of = |t: &u32| m.tri_face.get(*t as usize).copied().unwrap_or(0);
    let tri_looks =
        if looks.is_empty() { Vec::new() } else { origin.iter().map(|t| by_face.get(&(face_of(t) as usize)).copied().unwrap_or(0)).collect() };
    let (tri_faces, faces) = if tagged {
        let tri_faces: Vec<u32> = origin.iter().map(face_of).collect();
        let most = tri_faces.iter().max().map_or(0, |f| f.saturating_add(1));
        let faces = u32::try_from(b.body.face_count()).unwrap_or(u32::MAX).max(most);
        (tri_faces, faces)
    } else {
        (Vec::new(), 0)
    };
    let transparency = 1.0 - paint.map(|p| p.opacity).unwrap_or(1.0);
    Ok(MeshObject { name: b.name.clone(), positions, triangles, color: b.body.color(), transparency, looks, tri_looks, tri_faces, faces })
}

/// STEP assembly of a design with components: a product per component holding its bodies in
/// the component's own frame (`local` is the model before placing occurrences), and an
/// assembly occurrence per component occurrence. A body with an offset (an imported assembly's
/// part, built where the file placed it) is taken into its component's frame by that offset,
/// as the world placement does, so the occurrences above it place it only once.
pub fn step_assembly(doc: &Document, local: &ModelState, name: &str) -> Result<Vec<u8>> {
    use solidcraft_kernel::{ExportBody, ExportProduct};
    let mut index: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    index.insert(0, 0);
    let mut products = vec![ExportProduct { name: doc.name.clone(), bodies: Vec::new(), children: Vec::new() }];
    for c in &doc.components {
        index.insert(c.id, products.len());
        products.push(ExportProduct { name: c.name.clone(), bodies: Vec::new(), children: Vec::new() });
    }
    let framed: Vec<Option<solidcraft_kernel::Body>> = local
        .bodies
        .iter()
        .map(|b| match doc.model_body_offset(b) {
            Some(offset) if !solidcraft_doc::is_identity(offset) => solidcraft_kernel::transform_matrix(&b.body, *offset).map(Some),
            _ => Ok(None),
        })
        .collect::<std::result::Result<_, _>>()?;
    for (b, framed) in local.bodies.iter().zip(&framed) {
        let comp = doc.model_body_component(b);
        let Some(p) = index.get(&comp).and_then(|i| products.get_mut(*i)) else { continue };
        p.bodies.push(ExportBody { name: b.name.clone(), body: framed.as_ref().unwrap_or(&b.body), color: b.body.color() });
    }
    for o in &doc.occurrences {
        let (Some(&parent), Some(&child)) = (index.get(&o.parent), index.get(&o.component)) else { continue };
        if let Some(p) = products.get_mut(parent) {
            p.children.push((child, o.transform, o.name.clone()));
        }
    }
    Ok(solidcraft_kernel::step_export_products(&products, 0, &step_header(format!("{name}.step"), &doc.metadata))?.into_bytes())
}

/// Read a design file's bytes.
pub fn read_design(bytes: &[u8]) -> Result<Document> {
    if bytes.len() > MAX_DESIGN_BYTES {
        return Err(IoError::Invalid("design file too large".into()));
    }
    if is_design_container(bytes) {
        return design::read(bytes);
    }
    let s = std::str::from_utf8(bytes).map_err(|_| IoError::Invalid("design file is not UTF-8 JSON".into()))?;
    Ok(Document::from_json(s)?)
}

/// Design file bytes.
pub fn write_design(doc: &Document) -> Vec<u8> {
    doc.to_json().into_bytes()
}

/// Parse a binary or ASCII STL (used by tests and round-trip checks): triangles.
pub fn read_stl(bytes: &[u8]) -> Result<Vec<[[f32; 3]; 3]>> {
    // Binary when the size matches the triangle count in the header (some binary files start
    // with "solid" too); ASCII when the text has facets.
    let count = bytes.get(80..84).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let binary_size = count.is_some_and(|n| bytes.len() == 84 + n.saturating_mul(50));
    let head = String::from_utf8_lossy(bytes.get(..bytes.len().min(512)).unwrap_or_default()).to_string();
    let ascii = !binary_size && head.trim_start().starts_with("solid") && String::from_utf8_lossy(bytes).contains("facet");
    let mut tris = Vec::new();
    let ok = |v: &[f32; 3]| v.iter().all(|x| x.is_finite() && x.abs() < 1e9);
    if ascii {
        let s = String::from_utf8_lossy(bytes);
        let mut cur: Vec<[f32; 3]> = Vec::new();
        for l in s.lines() {
            let mut it = l.split_whitespace();
            match it.next() {
                Some("vertex") => {
                    let v: Vec<f32> = it.filter_map(|x| x.parse().ok()).collect();
                    if let [x, y, z] = v[..] {
                        cur.push([x, y, z]);
                    }
                }
                // Each facet takes exactly its three vertices (a bad vertex drops the facet).
                Some("endfacet") => {
                    if let [a, b, c] = cur[..]
                        && [a, b, c].iter().all(ok)
                    {
                        tris.push([a, b, c]);
                    }
                    cur.clear();
                }
                _ => {}
            }
            if tris.len() > MAX_STL_TRIANGLES {
                return Err(IoError::Invalid(format!("STL with more than {MAX_STL_TRIANGLES} triangles")));
            }
        }
        return Ok(tris);
    }
    let n = count.ok_or_else(|| IoError::Invalid("short STL".into()))?;
    // A wrong count in the header (some writers leave 0): use the triangles that are there.
    let room = bytes.len().saturating_sub(84) / 50;
    let n = if n == 0 || n > room { room } else { n };
    if n == 0 {
        return Err(IoError::Invalid("STL has no triangles".into()));
    }
    if n > MAX_STL_TRIANGLES {
        return Err(IoError::Invalid(format!("STL with {n} triangles (limit {MAX_STL_TRIANGLES})")));
    }
    let f = |o: usize| bytes.get(o..o + 4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(f32::NAN);
    tris.reserve(n);
    for i in 0..n {
        // The facet normal (first 12 bytes) is ignored: winding gives the orientation.
        let o = 84 + i * 50 + 12;
        let v = |k: usize| [f(o + k * 12), f(o + k * 12 + 4), f(o + k * 12 + 8)];
        let t = [v(0), v(1), v(2)];
        if t.iter().all(ok) {
            tris.push(t);
        }
    }
    Ok(tris)
}

/// Most triangles we read from one STL file.
pub const MAX_STL_TRIANGLES: usize = 20_000_000;

#[cfg(test)]
mod tests {
    use super::*;
    use solidcraft_doc::{FeatureKind, Model, Operation, point_expr};
    use solidcraft_geom::Vec3;

    fn model() -> (Document, Model) {
        let mut doc = Document::new("t");
        doc.add_feature(
            FeatureKind::Box {
                corner: point_expr(Vec3::ZERO),
                length: "10".into(),
                width: "20".into(),
                height: "30".into(),
                operation: Operation::NewBody,
            },
            None,
        )
        .unwrap();
        let mut m = Model::new();
        m.evaluate(&doc);
        (doc, m)
    }

    /// A binary STL of a cube's triangles.
    fn cube_stl(flip: &[usize], header: &[u8], count: Option<u32>, normals: [f32; 3]) -> Vec<u8> {
        let c = |i: usize| [(i & 1) as f32 * 10.0, ((i >> 1) & 1) as f32 * 10.0, ((i >> 2) & 1) as f32 * 10.0];
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let mut tris = Vec::new();
        for q in quads {
            tris.push([c(q[0]), c(q[1]), c(q[2])]);
            tris.push([c(q[0]), c(q[2]), c(q[3])]);
        }
        for &k in flip {
            tris[k].swap(1, 2);
        }
        let mut b = header.to_vec();
        b.resize(80, b' ');
        b.extend(count.unwrap_or(tris.len() as u32).to_le_bytes());
        for t in &tris {
            for x in normals.iter().chain(t.iter().flatten()) {
                b.extend(x.to_le_bytes());
            }
            b.extend([0, 0]);
        }
        b
    }

    fn mesh_volume(bytes: &[u8]) -> f64 {
        let f = mesh_import_feature(bytes, "x.stl").unwrap();
        let solidcraft_doc::FeatureKind::MeshImport { meshes, .. } = f.kind else { panic!() };
        let m = &meshes[0];
        let pos: Vec<Vec3> = m.positions.chunks(3).map(|c| Vec3::new(c[0], c[1], c[2])).collect();
        let tris: Vec<[u32; 3]> = m.triangles.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
        solidcraft_kernel::measure(&solidcraft_kernel::mesh_body(&pos, &tris).unwrap()).unwrap().volume
    }

    /// STL quirks: a binary header starting with "solid", a zero triangle count, garbage
    /// normals, stray inverted facets and non-finite vertices.
    #[test]
    fn stl_quirks_read_cleanly() {
        let v = mesh_volume(&cube_stl(&[], b"solid exported by something", None, [0.0; 3]));
        assert!((v - 1000.0).abs() < 1e-6, "{v}");
        let v = mesh_volume(&cube_stl(&[], b"x", Some(0), [f32::NAN, 9.0, -7.0]));
        assert!((v - 1000.0).abs() < 1e-6, "{v}");
        let v = mesh_volume(&cube_stl(&[1, 4, 7], b"x", None, [1.0, 0.0, 0.0]));
        assert!((v - 1000.0).abs() < 1e-6, "flipped facets: {v}");
        let mut bad = cube_stl(&[], b"x", None, [0.0; 3]);
        bad[84 + 12..84 + 16].copy_from_slice(&f32::INFINITY.to_le_bytes());
        assert_eq!(read_stl(&bad).unwrap().len(), 11);
        // ASCII with a non-UTF-8 name and a facet with a bad vertex.
        let mut ascii = b"solid caf\xe9\n".to_vec();
        ascii.extend(b"facet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\n");
        ascii.extend(b"facet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex nan 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid\n");
        assert_eq!(read_stl(&ascii).unwrap().len(), 1);
        for b in [&b""[..], b"solid", &[0u8; 83], &[0xff; 200], &cube_stl(&[], b"x", Some(u32::MAX), [0.0; 3])[..]] {
            let r = std::panic::catch_unwind(|| read_stl(b).map(|t| t.len()));
            assert!(r.is_ok());
        }
    }

    #[test]
    fn stl_round_trip_volume() {
        let (_, m) = model();
        let bytes = export(&m.state(), &[], Format::StlBinary, "t").unwrap();
        let tris = read_stl(&bytes).unwrap();
        let v: f64 = tris
            .iter()
            .map(|t| {
                let p = |k: usize| Vec3::new(t[k][0] as f64, t[k][1] as f64, t[k][2] as f64);
                p(0).dot(p(1).cross(p(2))) / 6.0
            })
            .sum();
        assert!((v - 6000.0).abs() < 1e-2, "{v}");
        let a = export(&m.state(), &[], Format::StlAscii, "t").unwrap();
        assert_eq!(read_stl(&a).unwrap().len(), tris.len());
        let o = String::from_utf8(export(&m.state(), &[], Format::Obj, "t").unwrap()).unwrap();
        assert!(o.contains("o Body1") && o.contains("\nf "));
    }

    /// The design properties land in each format's own metadata (#1015): STEP and IGES headers,
    /// 3MF model metadata. Other tools' keys never leave the design file.
    #[test]
    fn exports_write_design_properties() {
        let (_, m) = model();
        let meta: Metadata = serde_json::from_value(serde_json::json!({
            "project": "ACME <enclosures>", "author": "Jane O'Brien", "organization": "ACME", "license": "CC-BY-4.0",
            "description": "Vented Gehäuse\nfor a Pi", "revision": 7, "com.acme.secret": {"token": "SECRET-VALUE"},
        }))
        .unwrap();
        let step = String::from_utf8(export_with_metadata(&m.state(), &[], Format::Step, "t", &meta).unwrap()).unwrap();
        assert!(step.contains("FILE_DESCRIPTION(('Vented Geh\\X2\\00E4\\X0\\use for a Pi'),'2;1');"), "{}", &step[..300]);
        assert!(step.contains("FILE_NAME('t.step','',('Jane O''Brien'),('ACME'),"), "{}", &step[..300]);
        assert_eq!(step_import_feature(step.as_bytes(), "t.step").unwrap().body_names, ["Body1"], "still reads back");
        let iges = String::from_utf8(export_with_metadata(&m.state(), &[], Format::Iges, "t", &meta).unwrap()).unwrap();
        assert!(iges.contains("12HJane O'Brien") && iges.contains("4HACME"));
        let model_xml = |bytes: Vec<u8>| {
            use std::io::Read as _;
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            let mut x = String::new();
            zip.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut x).unwrap();
            x
        };
        let x = model_xml(export_with_metadata(&m.state(), &[], Format::ThreeMf, "t", &meta).unwrap());
        for want in [
            "<metadata name=\"Application\">SolidCraft</metadata>\n <metadata name=\"Title\">ACME &lt;enclosures&gt;</metadata>",
            "<metadata name=\"Designer\">Jane O&apos;Brien</metadata>",
            "<metadata name=\"Description\">Vented Gehäuse\nfor a Pi</metadata>",
            "<metadata name=\"LicenseTerms\">CC-BY-4.0</metadata>",
        ] {
            assert!(x.contains(want), "{want}\n{}", &x[..400]);
        }
        assert!(write_3mf_with(&[], &[("Title", "x")]).is_err(), "no objects is still an error");
        for out in [step.as_str(), iges.as_str(), x.as_str()] {
            assert!(!out.contains("SECRET-VALUE") && !out.contains("com.acme"));
        }
        // Without properties the files are as they always were.
        let plain = String::from_utf8(export(&m.state(), &[], Format::Step, "t").unwrap()).unwrap();
        assert!(plain.contains("FILE_DESCRIPTION(('SolidCraft model'),'2;1');\nFILE_NAME('t.step','',(''),(''),"));
        assert!(!model_xml(export(&m.state(), &[], Format::ThreeMf, "t").unwrap()).contains("Title"));
    }

    #[test]
    fn step_and_design() {
        let (doc, m) = model();
        let s = String::from_utf8(export(&m.state(), &[], Format::Step, "t").unwrap()).unwrap();
        assert!(s.starts_with("ISO-10303-21;") && s.contains("CLOSED_SHELL"));
        let back = read_design(&write_design(&doc)).unwrap();
        assert_eq!(back, doc);
        assert!(read_design(b"\xff\xfe").is_err());
        assert!(matches!(export(&m.state(), &["nope".into()], Format::Step, "t"), Err(IoError::Empty)));
        assert_eq!(Format::from_name("a.STP").unwrap(), Format::Step);
        assert!(is_step_path("/x/Part.STEP") && is_step_path("a.stp") && !is_step_path("a.step.json") && !is_step_path("step"));
        let f = step_import_feature(s.as_bytes(), "/tmp/My Part.step").unwrap();
        // Named after the exported product (the export was called `t`).
        assert_eq!(f.name, "t");
        assert_eq!(f.body_names, ["Body1"]);
        assert_eq!(f.body_names.len(), 1);
        assert!(matches!(&f.kind, FeatureKind::Import { file, .. } if file == "My Part.step"));
        let mut imported = Document::new("Imported STEP round trip");
        imported.add_feature(f.kind, None).unwrap();
        let packaged = read_design(&write_design_container(&imported).unwrap()).unwrap();
        assert_eq!(packaged, imported);
        let mut rebuilt = Model::new();
        rebuilt.evaluate(&packaged);
        assert!(rebuilt.results.iter().all(|r| r.error.is_none()));
        let state = rebuilt.state();
        assert_eq!(state.bodies.len(), 1);
        assert!((solidcraft_kernel::measure(&state.bodies[0].body).unwrap().volume - 6000.0).abs() < 1e-6);
        assert!(step_import_feature(b"ISO-10303-21;", "x.step").is_err());
        assert!(step_import_feature(&[0xff, 0xfe, 0x00], "x.step").is_err());
        assert!(Format::from_name("a.dwg").is_err());
        // Compatibility shim: the legacy design extension is still a design.
        assert_eq!(Format::from_name("old.solvecraft").unwrap(), Format::Design);
        assert_eq!(Format::from_name("new.solidcraft").unwrap(), Format::Design);
        assert!(is_design_path("/d/old.SOLVECRAFT") && is_design_path("new.solidcraft") && !is_design_path("a.step"));
        assert!(read_stl(&[0u8; 10]).is_err());
        let mut fake = vec![0u8; 84];
        fake[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_stl(&fake).is_err());
    }

    /// A frame holding a plate and two posts, whose solids carry the names of the last feature
    /// that made them, as some CAD systems write them; as STEP text.
    fn frame_step() -> String {
        use solidcraft_kernel::{ExportBody, ExportProduct, StepHeader, box_solid, step_export_products};
        let plate = box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 2.0)).unwrap();
        let post = box_solid(Vec3::ZERO, Vec3::new(2.0, 2.0, 8.0)).unwrap();
        let at = |x: f64| [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, 0.0, 0.0, 1.0]];
        let part = |name: &str, label: &str, body| ExportProduct {
            name: name.into(),
            bodies: vec![ExportBody { name: label.into(), body, color: None }],
            children: Vec::new(),
        };
        let products = vec![
            ExportProduct {
                name: "Frame".into(),
                bodies: Vec::new(),
                children: vec![(1, at(0.0), "Plate-1".into()), (2, at(20.0), "Post-1".into()), (2, at(30.0), "Post-2".into())],
            },
            part("Plate", "Cut-Extrude3", &plate),
            part("Post", "Boss-Extrude1", &post),
        ];
        step_export_products(&products, 0, &StepHeader::default()).unwrap()
    }

    /// The bodies of a STEP file are named after the solid (the default), the product that holds
    /// them, or the assembly occurrence; a name the file leaves out or fills with a placeholder
    /// falls back to the next one.
    #[test]
    fn step_import_names_follow_the_choice() {
        let text = frame_step();
        let names = |text: &str, choice| step_import_feature_named(text.as_bytes(), "frame.step", choice).unwrap().body_names;
        assert_eq!(names(&text, StepNames::Solid), ["Cut-Extrude3", "Boss-Extrude1", "Boss-Extrude1"]);
        assert_eq!(names(&text, StepNames::Product), ["Plate", "Post", "Post"]);
        assert_eq!(names(&text, StepNames::Instance), ["Plate-1", "Post-1", "Post-2"]);
        // Without a choice it is the solid's name, as before.
        assert_eq!(step_import_feature(text.as_bytes(), "frame.step").unwrap().body_names, names(&text, StepNames::Solid));
        // The timeline entry is the root product's name whatever the bodies are called.
        for choice in [StepNames::Solid, StepNames::Product, StepNames::Instance] {
            assert_eq!(step_import_feature_named(text.as_bytes(), "frame.step", choice).unwrap().name, "Frame");
        }
        // A product the file leaves unnamed says nothing: its bodies keep the solid's name.
        let unnamed = text.replace("PRODUCT('Plate','Plate',", "PRODUCT('(unnamed)','(unnamed)',");
        assert_eq!(names(&unnamed, StepNames::Product), ["Cut-Extrude3", "Post", "Post"]);
        // A part file has no occurrence: the instance choice gives the product's name.
        let (_, m) = model();
        let part = String::from_utf8(export(&m.state(), &[], Format::Step, "Bracket").unwrap()).unwrap();
        assert_eq!(names(&part, StepNames::Solid), ["Body1"]);
        assert_eq!(names(&part, StepNames::Product), ["Bracket"]);
        assert_eq!(names(&part, StepNames::Instance), ["Bracket"]);
        assert_eq!(StepNames::parse(" Product "), Some(StepNames::Product));
        assert_eq!(StepNames::parse("INSTANCE"), Some(StepNames::Instance));
        assert_eq!(StepNames::parse("label"), None);
    }

    fn stl_volume(bytes: &[u8]) -> f64 {
        read_stl(bytes)
            .unwrap()
            .iter()
            .map(|t| {
                let p = |k: usize| Vec3::new(t[k][0] as f64, t[k][1] as f64, t[k][2] as f64);
                p(0).dot(p(1).cross(p(2))) / 6.0
            })
            .sum()
    }

    /// Build → export 3MF → import → the same volume as the STL export, closed meshes, names.
    #[test]
    fn threemf_round_trip() {
        let mut doc = Document::new("t");
        doc.add_feature(
            FeatureKind::Box {
                corner: point_expr(Vec3::ZERO),
                length: "40".into(),
                width: "30".into(),
                height: "10".into(),
                operation: Operation::NewBody,
            },
            None,
        )
        .unwrap();
        doc.add_feature(
            FeatureKind::Fillet {
                edges: vec![solidcraft_doc::Position(Vec3::new(0.0, 0.0, 5.0))],
                radius: "4".into(),
                body: None,
                style: Default::default(),
                fit: Default::default(),
                corner: Default::default(),
            },
            None,
        )
        .unwrap();
        doc.add_feature(
            FeatureKind::Cylinder {
                base: point_expr(Vec3::new(80.0, 0.0, 0.0)),
                axis: solidcraft_doc::Dir(Vec3::Z),
                radius: "6".into(),
                height: "15".into(),
                operation: Operation::NewBody,
            },
            None,
        )
        .unwrap();
        doc.add_feature(
            FeatureKind::Torus {
                center: point_expr(Vec3::new(0.0, 80.0, 0.0)),
                major: "12".into(),
                minor: "3".into(),
                operation: Operation::NewBody,
            },
            None,
        )
        .unwrap();
        let mut m = Model::new();
        m.evaluate(&doc);
        let st = m.state();
        assert_eq!(st.bodies.len(), 3);
        let bytes = export(&st, &[], Format::ThreeMf, "t").unwrap();
        let f = mesh_import_feature(&bytes, "/tmp/parts.3MF").unwrap();
        assert_eq!(f.body_names, st.bodies.iter().map(|b| b.name.clone()).collect::<Vec<_>>());
        let mut d2 = Document::new("m");
        let id = d2.add_feature(f.kind, Some(&f.name)).unwrap();
        let mut m2 = Model::new();
        m2.evaluate(&d2);
        assert!(m2.result(id).unwrap().error.is_none());
        let st2 = m2.state();
        assert_eq!(st2.bodies.len(), 3);
        for (a, b) in st.bodies.iter().zip(&st2.bodies) {
            assert_eq!(a.name, b.name);
            assert!(b.body.is_mesh() && b.body.is_closed_mesh(), "{} is not watertight", b.name);
            let v_stl = stl_volume(&export(&st, std::slice::from_ref(&a.name), Format::StlBinary, "t").unwrap());
            let v = solidcraft_kernel::measure(&b.body).unwrap().volume;
            assert!((v - v_stl).abs() <= 1e-6 * v_stl.abs(), "{}: {v} vs {v_stl}", a.name);
        }
        // Mesh bodies export again (3MF, STL, OBJ) but not as STEP.
        assert!(export(&st2, &[], Format::ThreeMf, "t").is_ok());
        assert!(export(&st2, &[], Format::StlAscii, "t").is_ok());
        assert!(export(&st2, &[], Format::Obj, "t").is_ok());
        assert!(export(&st2, &[], Format::Step, "t").is_err());
        // STL imports the same way.
        let stl = export(&st, &[], Format::StlBinary, "t").unwrap();
        let fs = mesh_import_feature(&stl, "all.stl").unwrap();
        assert_eq!(fs.name, "all");
        assert!(mesh_import_feature(b"PK", "x.3mf").is_err());
        assert!(mesh_import_feature(&[0u8; 84], "x.stl").is_err());
        assert!(is_mesh_path("a.3MF") && is_mesh_path("b.stl") && !is_mesh_path("c.step"));
    }

    /// Face-tagged 3MF: every triangle carries its B-rep face, every coordinate is the
    /// tessellation's own `f64`, the bytes are deterministic, and the package stays plain 3MF.
    #[test]
    fn tagged_threemf_carries_faces_exactly() {
        use std::io::Read;
        let mut doc = Document::new("t");
        doc.add_feature(
            FeatureKind::Box {
                corner: point_expr(Vec3::new(0.1, -0.3, 1.0 / 3.0)),
                length: "40".into(),
                width: "30".into(),
                height: "10".into(),
                operation: Operation::NewBody,
            },
            None,
        )
        .unwrap();
        doc.add_feature(
            FeatureKind::Fillet {
                edges: vec![solidcraft_doc::Position(Vec3::new(0.1, -0.3, 5.0 + 1.0 / 3.0))],
                radius: "4".into(),
                body: None,
                style: Default::default(),
                fit: Default::default(),
                corner: Default::default(),
            },
            None,
        )
        .unwrap();
        let mut m = Model::new();
        m.evaluate(&doc);
        let st = m.state();
        let body = &st.bodies[0].body;
        let faces = body.face_count();
        assert_eq!(faces, 7);
        let bytes = export(&st, &[], Format::ThreeMfTagged, "t").unwrap();
        assert_eq!(bytes, export(&st, &[], Format::ThreeMfTagged, "t").unwrap(), "deterministic bytes");
        assert_eq!(Format::from_name("3mf-tagged").unwrap(), Format::ThreeMfTagged);
        assert_eq!(Format::from_name("part.3mf").unwrap(), Format::ThreeMf);

        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())).unwrap();
        let mut xml = String::new();
        zip.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut xml).unwrap();
        assert!(xml.contains(&format!("xmlns:ft=\"{}\"", threemf::FACE_TAGS_NS)), "{}", &xml[..400]);
        assert!(xml.contains(&format!("ft:faces=\"{faces}\"")));
        let tris: Vec<&str> = xml.lines().filter(|l| l.trim_start().starts_with("<triangle ")).collect();
        assert!(!tris.is_empty() && tris.iter().all(|l| l.contains(" ft:face=\"")));
        let mut seen: Vec<usize> =
            tris.iter().map(|l| l.split("ft:face=\"").nth(1).and_then(|r| r.split('"').next()).unwrap().parse().unwrap()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, (0..faces).collect::<Vec<_>>(), "every face is tagged");
        // Coordinates are the tessellation's own values, bit for bit.
        let exact: std::collections::HashSet<u64> =
            export_mesh(body).unwrap().positions.iter().flat_map(|p| [p.x, p.y, p.z]).map(|v| (v + 0.0).to_bits()).collect();
        let attr = |l: &str, k: &str| l.split(&format!(" {k}=\"")).nth(1).and_then(|r| r.split('"').next()).unwrap().parse::<f64>().unwrap();
        for l in xml.lines().filter(|l| l.trim_start().starts_with("<vertex ")) {
            for k in ["x", "y", "z"] {
                assert!(exact.contains(&attr(l, k).to_bits()), "{l}");
            }
        }
        // Any 3MF reader opens it: SolidCraft's own ignores the tags and gets a closed mesh.
        let f = mesh_import_feature(&bytes, "tagged.3mf").unwrap();
        let mut d2 = Document::new("m");
        let id = d2.add_feature(f.kind, Some(&f.name)).unwrap();
        let mut m2 = Model::new();
        m2.evaluate(&d2);
        assert!(m2.result(id).unwrap().error.is_none());
        assert!(m2.state().bodies[0].body.is_closed_mesh());
        // The plain export is unchanged: no tags.
        let plain = export(&st, &[], Format::ThreeMf, "t").unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(plain.as_slice())).unwrap();
        let mut xml = String::new();
        zip.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut xml).unwrap();
        assert!(!xml.contains("ft:"));
    }
}
