//! Versioned design packages. Geometry is rebuilt from authoritative inputs; no
//! evaluated cache is trusted here. Entries are never extracted to filesystem paths.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

mod native_cache;
mod thumbnail;
pub use native_cache::{DesignGeometry, read_design_geometry};
pub use thumbnail::{THUMBNAIL_SIZE, read_design_thumbnail, read_design_thumbnail_file};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solidcraft_doc::{Document, FeatureKind, MeshData};

use crate::{IoError, MAX_DESIGN_BYTES, Result, design_metadata::replace};

const VERSION: &str = "solidcraft-container/1";
const LEGACY_VERSION: &str = "solvecraft-container/1";
const PREVIEW_VERSION: &str = "solidcraft-preview/1";
const MAX_ENTRIES: usize = 10_000;
const MAX_EXPANDED: usize = 512 << 20;
const MESH_MAGIC: &[u8; 8] = b"SCMESH01";
const MAX_PREVIEW_BYTES: usize = 8 << 20;
const MAX_PREVIEW_TRIANGLES: usize = 100_000;

/// Untrusted, display-only evaluated meshes. Never used for editing, picking or measurement.
#[derive(Default)]
pub struct DesignPreview {
    pub meshes: Vec<MeshData>,
}

/// Fast favors interactive file operations; Compact trades CPU time for space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DesignCompression {
    #[default]
    Fast,
    Compact,
}

fn invalid(message: impl std::fmt::Display) -> IoError {
    IoError::Invalid(format!("design container: {message}"))
}

/// Detect the package signature independently of the file name.
pub fn is_design_container(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

fn fields(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct PackageWriter {
    zip: zip::ZipWriter<Cursor<Vec<u8>>>,
    names: BTreeSet<String>,
    expanded: usize,
    resolved: usize,
}

impl PackageWriter {
    fn part(&mut self, name: &str, bytes: &[u8], compressed: bool) -> Result<()> {
        if bytes.len() > MAX_DESIGN_BYTES {
            return Err(invalid("part too large"));
        }
        if !self.names.insert(name.to_owned()) {
            return Ok(());
        }
        self.expanded = self.expanded.checked_add(bytes.len()).ok_or_else(|| invalid("expanded size overflow"))?;
        if self.expanded > MAX_EXPANDED || self.names.len() > MAX_ENTRIES {
            return Err(invalid("package too large"));
        }
        let method = if compressed { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
        let options = zip::write::SimpleFileOptions::default().compression_method(method).compression_level(compressed.then_some(1));
        self.zip.start_file(name, options).map_err(invalid)?;
        self.zip.write_all(bytes).map_err(invalid)
    }
    fn blob(&mut self, bytes: &[u8], extension: &str, compressed: bool) -> Result<Value> {
        self.resolved =
            self.resolved.checked_add(bytes.len()).filter(|&n| n <= MAX_EXPANDED).ok_or_else(|| invalid("resolved payloads too large"))?;
        let name = format!("blobs/{}.{extension}", digest(bytes));
        self.part(&name, bytes, compressed)?;
        Ok(Value::String(name))
    }
}

/// Write a compact container, deduplicating payloads by SHA-256. Mesh arrays are
/// stored directly for low save/open latency; small metadata uses level-one
/// Deflate. Use the explicit Compact option to compress STEP and meshes too.
pub fn write_design_container(doc: &Document) -> Result<Vec<u8>> {
    write_design_container_with_compression(doc, DesignCompression::Fast)
}

/// Write a container with an explicit payload compression policy.
pub fn write_design_container_with_compression(doc: &Document, compression: DesignCompression) -> Result<Vec<u8>> {
    write_design_container_with_preview(doc, compression, None)
}

/// Optional display cache failure never prevents saving the authoritative design.
pub fn write_design_container_with_preview(doc: &Document, compression: DesignCompression, preview: Option<&DesignPreview>) -> Result<Vec<u8>> {
    write_design_container_with_cache(doc, compression, preview, None)
}

/// Save an optional warm native cache without making document saving depend on it.
/// Display meshes remain separately opt-in; no evaluation or meshing happens here.
pub fn write_design_container_with_cache(
    doc: &Document,
    compression: DesignCompression,
    preview: Option<&DesignPreview>,
    geometry: Option<&[DesignGeometry]>,
) -> Result<Vec<u8>> {
    write_design_container_with_thumbnail(doc, compression, preview, geometry, None)
}

/// Include a bounded portable PNG picture when one is already available.
/// Unsupported headers or oversized pictures are omitted without changing the design.
pub fn write_design_container_with_thumbnail(
    doc: &Document,
    compression: DesignCompression,
    preview: Option<&DesignPreview>,
    geometry: Option<&[DesignGeometry]>,
    thumbnail: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut writer = PackageWriter { zip: zip::ZipWriter::new(Cursor::new(Vec::new())), names: BTreeSet::new(), expanded: 0, resolved: 0 };
    let mut features = Vec::with_capacity(doc.features.len());
    for feature in &doc.features {
        let replacements = match &feature.kind {
            FeatureKind::Import { step, .. } => {
                let blob = writer.blob(step.as_bytes(), "step", compression == DesignCompression::Compact)?;
                fields([("step", Value::String(String::new())), ("step_blob", blob)])
            }
            FeatureKind::MeshImport { meshes, .. } => {
                let mut metadata = Vec::with_capacity(meshes.len());
                for mesh in meshes {
                    let bytes = encode_mesh(mesh)?;
                    let blob = writer.blob(&bytes, "mesh", compression == DesignCompression::Compact)?;
                    let mut value = replace(mesh, fields([("positions", json!([])), ("triangles", json!([]))])).map_err(invalid)?;
                    value.as_object_mut().ok_or_else(|| invalid("mesh metadata"))?.insert("mesh_blob".into(), blob);
                    metadata.push(value);
                }
                fields([("meshes", Value::Array(metadata))])
            }
            _ => Map::new(),
        };
        // Extra reference fields are added after serializing existing fields.
        let step_blob = replacements.get("step_blob").cloned();
        let mut value = replace(feature, replacements).map_err(invalid)?;
        if let Some(blob) = step_blob {
            value.as_object_mut().ok_or_else(|| invalid("feature metadata"))?.insert("step_blob".into(), blob);
        }
        features.push(value);
    }
    let mut canvases = Vec::with_capacity(doc.canvases.len());
    for canvas in &doc.canvases {
        if canvas.data.len() > solidcraft_doc::canvas::MAX_CANVAS_BYTES.div_ceil(3) * 4 {
            return Err(invalid("canvas too large"));
        }
        let bytes = canvas.bytes().ok_or_else(|| invalid("invalid canvas base64"))?;
        if !matches!(canvas.format.as_str(), "png" | "jpeg") || bytes.len() > solidcraft_doc::canvas::MAX_CANVAS_BYTES {
            return Err(invalid("invalid canvas format or size"));
        }
        let blob = writer.blob(&bytes, &canvas.format, false)?;
        let mut value = replace(canvas, fields([("data", json!(""))])).map_err(invalid)?;
        value.as_object_mut().ok_or_else(|| invalid("canvas metadata"))?.insert("image_blob".into(), blob);
        canvases.push(value);
    }
    let metadata = replace(doc, fields([("features", Value::Array(features)), ("canvases", Value::Array(canvases))])).map_err(invalid)?;
    let metadata = serde_json::to_vec(&metadata).map_err(invalid)?;
    writer.part("document.json", &metadata, true)?;
    writer.part("container.json", &serde_json::to_vec(&json!({"format": VERSION})).map_err(invalid)?, false)?;
    if let Some((manifest, data)) = geometry.and_then(|g| native_cache::encode_geometry(g, &metadata).ok())
        && writer.names.len().saturating_add(2) <= MAX_ENTRIES
        && writer
            .expanded
            .saturating_add(manifest.len())
            .saturating_add(data.len())
            .saturating_add(writer.names.len().saturating_mul(256))
            .saturating_add(65_536)
            <= MAX_DESIGN_BYTES
    {
        writer.part("cache/key.json", &manifest, false)?;
        writer.part("cache/brep.bin", &data, compression == DesignCompression::Compact)?;
    }
    if let Some((manifest, data)) = preview.and_then(|p| encode_preview(p, &metadata).ok())
        && writer.names.len().saturating_add(2) <= MAX_ENTRIES
        && writer
            .expanded
            .saturating_add(manifest.len())
            .saturating_add(data.len())
            .saturating_add(writer.names.len().saturating_mul(256))
            .saturating_add(65_536)
            <= MAX_DESIGN_BYTES
    {
        writer.part("cache/preview.json", &manifest, false)?;
        writer.part("cache/preview.bin", &data, false)?;
    }
    if let Some(png) = thumbnail {
        thumbnail::write(&mut writer, png, &metadata)?;
    }
    let bytes = writer.zip.finish().map_err(invalid)?.into_inner();
    if bytes.len() > MAX_DESIGN_BYTES {
        return Err(invalid("compressed file too large"));
    }
    Ok(bytes)
}

fn validate_mesh(mesh: &MeshData) -> Result<()> {
    if !mesh.positions.len().is_multiple_of(3)
        || !mesh.triangles.len().is_multiple_of(3)
        || mesh.positions.iter().any(|v| !v.is_finite() || v.abs() > 1e9)
        || mesh.triangles.iter().any(|&v| v as usize >= mesh.positions.len() / 3)
    {
        return Err(invalid("invalid mesh coordinates or indices"));
    }
    Ok(())
}

fn encode_mesh(mesh: &MeshData) -> Result<Vec<u8>> {
    validate_mesh(mesh)?;
    let size = mesh
        .positions
        .len()
        .checked_mul(8)
        .and_then(|v| mesh.triangles.len().checked_mul(4).and_then(|t| v.checked_add(t)))
        .and_then(|v| v.checked_add(24))
        .filter(|&n| n <= MAX_DESIGN_BYTES)
        .ok_or_else(|| invalid("mesh too large"))?;
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(MESH_MAGIC);
    bytes.extend_from_slice(&(mesh.positions.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(mesh.triangles.len() as u64).to_le_bytes());
    for value in &mesh.positions {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in &mesh.triangles {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Ok(bytes)
}

fn decode_mesh(bytes: &[u8], mesh: &mut MeshData) -> Result<()> {
    if bytes.get(..8) != Some(MESH_MAGIC.as_slice()) {
        return Err(invalid("unsupported mesh blob version"));
    }
    let count = |range: std::ops::Range<usize>| -> Result<usize> {
        let array: [u8; 8] = bytes.get(range).ok_or_else(|| invalid("truncated mesh blob"))?.try_into().map_err(invalid)?;
        usize::try_from(u64::from_le_bytes(array)).map_err(invalid)
    };
    let positions = count(8..16)?;
    let triangles = count(16..24)?;
    let middle = positions.checked_mul(8).and_then(|n| n.checked_add(24)).ok_or_else(|| invalid("mesh size overflow"))?;
    let total = triangles.checked_mul(4).and_then(|n| n.checked_add(middle)).ok_or_else(|| invalid("mesh size overflow"))?;
    if total != bytes.len() || !positions.is_multiple_of(3) || !triangles.is_multiple_of(3) {
        return Err(invalid("invalid mesh blob lengths"));
    }
    mesh.positions =
        bytes.get(24..middle).ok_or_else(|| invalid("truncated mesh"))?.as_chunks::<8>().0.iter().map(|&v| f64::from_le_bytes(v)).collect();
    mesh.triangles =
        bytes.get(middle..).ok_or_else(|| invalid("truncated mesh"))?.as_chunks::<4>().0.iter().map(|&v| u32::from_le_bytes(v)).collect();
    validate_mesh(mesh)
}

struct PackageReader<R: Read + Seek> {
    zip: zip::ZipArchive<R>,
    blobs: BTreeMap<String, Vec<u8>>,
    hydrated: usize,
    document: Option<Arc<[u8]>>,
}

impl<R: Read + Seek> PackageReader<R> {
    fn new(mut source: R) -> Result<Self> {
        let length = source.seek(SeekFrom::End(0)).map_err(invalid)?;
        if length > MAX_DESIGN_BYTES as u64 {
            return Err(invalid("compressed file too large"));
        }
        let tail_length = length.min(65_557) as usize;
        source.seek(SeekFrom::Start(length - tail_length as u64)).map_err(invalid)?;
        let mut tail = vec![0; tail_length];
        source.read_exact(&mut tail).map_err(invalid)?;
        // zip's name index can coalesce duplicate central-directory entries. Our
        // bounded format does not need ZIP64/multiple disks: verify the declared
        // entry count as well so duplicate names cannot silently select a payload.
        let eocd = tail.windows(4).rposition(|v| v == b"PK\x05\x06").ok_or_else(|| invalid("missing ZIP directory"))?;
        let end = tail.get(eocd..).filter(|v| v.len() >= 22).ok_or_else(|| invalid("truncated ZIP directory"))?;
        let u16_at = |offset| -> Result<u16> {
            let array: [u8; 2] = end.get(offset..offset + 2).ok_or_else(|| invalid("truncated ZIP directory"))?.try_into().map_err(invalid)?;
            Ok(u16::from_le_bytes(array))
        };
        let declared = u16_at(10)? as usize;
        if declared > MAX_ENTRIES || u16_at(4)? != 0 || u16_at(6)? != 0 || u16_at(8)? as usize != declared || end.len() != 22 + u16_at(20)? as usize {
            return Err(invalid("too many entries or unsupported ZIP directory"));
        }
        // A design file is a ZIP with nothing in front of it. Saying so skips zip's search for a
        // prefix, which reads back through the stored parts and would load a large embedded
        // source before the preview can be shown.
        let config = zip::read::Config { archive_offset: zip::read::ArchiveOffset::Known(0) };
        let mut zip = zip::ZipArchive::with_config(config, source).map_err(invalid)?;
        if u16_at(4)? != 0
            || u16_at(6)? != 0
            || u16_at(8)? as usize != zip.len()
            || u16_at(10)? as usize != zip.len()
            || end.len() != 22 + u16_at(20)? as usize
        {
            return Err(invalid("duplicate entries or unsupported ZIP directory"));
        }
        let mut names = BTreeSet::new();
        let mut total = 0u64;
        for i in 0..zip.len() {
            let part = zip.by_index_raw(i).map_err(invalid)?;
            let name = part.name().map_err(invalid)?.into_owned();
            if name.is_empty()
                || name.contains('\\')
                || name.split('/').any(|c| c.is_empty() || c == "." || c == "..")
                || !names.insert(name.to_owned())
            {
                return Err(invalid("duplicate or invalid archive entry"));
            }
            total = total.checked_add(part.size()).ok_or_else(|| invalid("expanded size overflow"))?;
            if part.size() > MAX_DESIGN_BYTES as u64 || total > MAX_EXPANDED as u64 {
                return Err(invalid("expanded package too large"));
            }
        }
        Ok(Self { zip, blobs: BTreeMap::new(), hydrated: 0, document: None })
    }
    // Bind provisional caches and authoritative reconstruction to identical metadata,
    // even if another process overwrites the open file in place. Blobs remain hash-checked.
    fn document(&mut self, limit: usize) -> Result<Arc<[u8]>> {
        if self.document.is_none() {
            self.document = Some(self.limited_part("document.json", limit)?.into());
        }
        self.document.as_ref().filter(|d| d.len() <= limit).cloned().ok_or_else(|| invalid("metadata budget"))
    }
    fn part(&mut self, name: &str) -> Result<Vec<u8>> {
        self.limited_part(name, MAX_DESIGN_BYTES)
    }
    fn limited_part(&mut self, name: &str, limit: usize) -> Result<Vec<u8>> {
        let part = self.zip.by_name(name).map_err(|e| invalid(format!("required part `{name}`: {e}")))?;
        let expected = part.size();
        if expected > limit as u64 {
            return Err(invalid("part too large"));
        }
        let mut bytes = Vec::with_capacity(expected as usize);
        part.take(limit as u64 + 1).read_to_end(&mut bytes).map_err(invalid)?;
        if bytes.len() > limit || bytes.len() as u64 != expected {
            return Err(invalid("part too large or length mismatch"));
        }
        Ok(bytes)
    }
    fn blob(&mut self, value: &Value, field: &str, extension: &str) -> Result<&[u8]> {
        let name = value.get(field).and_then(Value::as_str).ok_or_else(|| invalid(format!("missing `{field}` reference")))?;
        let hash = name
            .strip_prefix("blobs/")
            .and_then(|n| n.strip_suffix(&format!(".{extension}")))
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            .ok_or_else(|| invalid("invalid blob reference"))?;
        if !self.blobs.contains_key(name) {
            let bytes = self.part(name)?;
            if digest(&bytes) != hash {
                return Err(invalid(format!("blob hash mismatch: {name}")));
            }
            self.blobs.insert(name.to_owned(), bytes);
        }
        let bytes = self.blobs.get(name).ok_or_else(|| invalid("missing blob"))?;
        // Count repeated references too: a tiny archive must not expand into unbounded
        // owned document strings/arrays when the same blob is referenced repeatedly.
        self.hydrated =
            self.hydrated.checked_add(bytes.len()).filter(|&n| n <= MAX_EXPANDED).ok_or_else(|| invalid("resolved payloads too large"))?;
        Ok(bytes)
    }
}

fn encode_preview(preview: &DesignPreview, document: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    if preview.meshes.is_empty() || preview.meshes.len() > 1024 {
        return Err(invalid("preview body count"));
    }
    let mut data = Vec::new();
    let mut entries = Vec::new();
    let mut triangles = 0usize;
    for mesh in &preview.meshes {
        triangles = triangles.saturating_add(mesh.triangles.len() / 3);
        let size = mesh.positions.len().saturating_mul(8).saturating_add(mesh.triangles.len().saturating_mul(4)).saturating_add(24);
        if data.len().saturating_add(size) > MAX_PREVIEW_BYTES || triangles > MAX_PREVIEW_TRIANGLES || mesh.name.len() > 1024 {
            return Err(invalid("preview budget"));
        }
        let bytes = encode_mesh(mesh)?;
        entries.push(json!({"name":mesh.name,"color":mesh.color,"offset":data.len(),"length":bytes.len()}));
        data.extend_from_slice(&bytes);
    }
    let manifest = serde_json::to_vec(&json!({
        "format":PREVIEW_VERSION, "build":env!("CARGO_PKG_VERSION"),
        "document":digest(document), "payload":digest(&data), "meshes":entries
    }))
    .map_err(invalid)?;
    if manifest.len() > 65_536 {
        return Err(invalid("preview manifest budget"));
    }
    Ok((manifest, data))
}

/// Read only an optional provisional display cache; any corruption/incompatibility is a miss.
/// The normal design reader must still validate and rebuild before enabling the design.
pub fn read_design_preview(bytes: &[u8]) -> Option<DesignPreview> {
    if bytes.len() > MAX_DESIGN_BYTES || !is_design_container(bytes) {
        return None;
    }
    DesignContainerReader::new(Cursor::new(bytes)).ok()?.preview()
}

fn read_preview<R: Read + Seek>(reader: &mut PackageReader<R>) -> Result<DesignPreview> {
    let container: Value = serde_json::from_slice(&reader.limited_part("container.json", 4096)?).map_err(invalid)?;
    if !matches!(container.get("format").and_then(Value::as_str), Some(VERSION | LEGACY_VERSION)) {
        return Err(invalid("unsupported container version"));
    }
    let manifest: Value = serde_json::from_slice(&reader.limited_part("cache/preview.json", 65_536)?).map_err(invalid)?;
    if manifest.get("format").and_then(Value::as_str) != Some(PREVIEW_VERSION)
        || manifest.get("build").and_then(Value::as_str) != Some(env!("CARGO_PKG_VERSION"))
    {
        return Err(invalid("incompatible preview"));
    }
    if manifest.get("document").and_then(Value::as_str) != Some(digest(&reader.document(16 << 20)?).as_str()) {
        return Err(invalid("stale preview"));
    }
    let data = reader.limited_part("cache/preview.bin", MAX_PREVIEW_BYTES)?;
    if manifest.get("payload").and_then(Value::as_str) != Some(digest(&data).as_str()) {
        return Err(invalid("damaged preview"));
    }
    let entries =
        manifest.get("meshes").and_then(Value::as_array).filter(|m| !m.is_empty() && m.len() <= 1024).ok_or_else(|| invalid("preview bodies"))?;
    let mut preview = DesignPreview::default();
    let mut end = 0usize;
    let mut triangles = 0usize;
    for entry in entries {
        let offset = entry.get("offset").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| invalid("preview offset"))?;
        let length = entry.get("length").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| invalid("preview length"))?;
        if offset != end {
            return Err(invalid("overlapping preview bodies"));
        }
        end = end.checked_add(length).ok_or_else(|| invalid("preview overflow"))?;
        let mut mesh = MeshData { name: String::new(), positions: Vec::new(), triangles: Vec::new(), color: None };
        decode_mesh(data.get(offset..end).ok_or_else(|| invalid("truncated preview"))?, &mut mesh)?;
        triangles = triangles.saturating_add(mesh.triangles.len() / 3);
        if triangles > MAX_PREVIEW_TRIANGLES {
            return Err(invalid("preview triangles"));
        }
        mesh.name = entry.get("name").and_then(Value::as_str).filter(|n| n.len() <= 1024).ok_or_else(|| invalid("preview name"))?.into();
        if !entry.get("color").is_none_or(Value::is_null) {
            let color: [f32; 3] = serde_json::from_value(entry.get("color").cloned().unwrap_or(Value::Null)).map_err(invalid)?;
            if color.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
                return Err(invalid("preview color"));
            }
            mesh.color = Some(color);
        }
        preview.meshes.push(mesh);
    }
    if end != data.len() {
        return Err(invalid("trailing preview data"));
    }
    Ok(preview)
}

/// A bounded container reader over a seekable source.
///
/// Opening inspects the directory, not the source blobs. `preview` reads only the
/// optional display entries and document metadata; `read_document` then resolves
/// and validates authoritative blobs against the same retained metadata snapshot.
/// Concurrent source changes either retain that document or fail validation.
/// This type does not turn untrusted cached data into editable geometry.
pub struct DesignContainerReader<R: Read + Seek> {
    reader: PackageReader<R>,
}

impl<R: Read + Seek> DesignContainerReader<R> {
    /// Open a container (not plain JSON), enforcing the same budgets as byte IO.
    pub fn new(source: R) -> Result<Self> {
        Ok(Self { reader: PackageReader::new(source)? })
    }

    /// Optional display-only data. Corruption or incompatibility is a cache miss.
    pub fn preview(&mut self) -> Option<DesignPreview> {
        // An explicitly saved mesh avoids tessellation. Native cache is the default
        // fallback, generating a bounded provisional mesh without source imports.
        read_preview(&mut self.reader).ok().or_else(|| native_cache::geometry_preview(self.geometry()?).ok())
    }

    /// Portable PNG picture only; no source blobs or geometry are evaluated.
    pub fn thumbnail(&mut self) -> Option<Vec<u8>> {
        thumbnail::read(&mut self.reader).ok()
    }

    /// Native geometry is provisional, never a substitute for rebuilding the document.
    pub fn geometry(&mut self) -> Option<Vec<DesignGeometry>> {
        let document = self.reader.document(16 << 20).ok()?;
        native_cache::read_geometry_with(&mut self.reader, &document).ok()
    }

    /// Consume the reader, resolving every required blob before returning a document.
    pub fn read_document(mut self) -> Result<Document> {
        read_document(&mut self.reader)
    }
}

pub(super) fn read(bytes: &[u8]) -> Result<Document> {
    DesignContainerReader::new(Cursor::new(bytes))?.read_document()
}

/// Open from disk (or browser VFS), publishing only provisional display before
/// authoritative blob hydration. Desktop keeps one seekable file handle instead
/// of buffering the complete archive. JSON continues through the normal migrations.
pub fn read_design_file(path: &str, notify: Option<impl FnOnce(DesignPreview)>) -> Result<Document> {
    #[cfg(not(target_arch = "wasm32"))]
    let mut source = {
        crate::vfs::check(path).map_err(invalid)?;
        let metadata = std::fs::metadata(path).map_err(invalid)?;
        if !metadata.is_file() {
            return Err(invalid("not a regular file"));
        }
        if metadata.len() > MAX_DESIGN_BYTES as u64 {
            return Err(invalid("file too large"));
        }
        let file = std::fs::File::open(path).map_err(invalid)?;
        if !file.metadata().map_err(invalid)?.is_file() {
            return Err(invalid("not a regular file"));
        }
        file
    };
    #[cfg(target_arch = "wasm32")]
    let mut source = Cursor::new(crate::vfs::read(path, MAX_DESIGN_BYTES).map_err(invalid)?);
    let size = source.seek(SeekFrom::End(0)).map_err(invalid)?;
    if size > MAX_DESIGN_BYTES as u64 {
        return Err(invalid("file too large"));
    }
    source.seek(SeekFrom::Start(0)).map_err(invalid)?;
    let mut prefix = [0u8; 4];
    if size >= 4 {
        source.read_exact(&mut prefix).map_err(invalid)?;
    }
    source.seek(SeekFrom::Start(0)).map_err(invalid)?;
    if is_design_container(&prefix) {
        let mut reader = DesignContainerReader::new(source)?;
        if let Some(notify) = notify
            && let Some(preview) = reader.preview()
        {
            notify(preview);
        }
        reader.read_document()
    } else {
        let mut bytes = Vec::new();
        source.take(MAX_DESIGN_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(invalid)?;
        crate::read_design(&bytes)
    }
}

fn read_document<R: Read + Seek>(reader: &mut PackageReader<R>) -> Result<Document> {
    let version: Value = serde_json::from_slice(&reader.part("container.json")?).map_err(invalid)?;
    if !matches!(version.get("format").and_then(Value::as_str), Some(VERSION | LEGACY_VERSION)) {
        return Err(invalid("unsupported container version"));
    }
    let bytes = reader.document(MAX_DESIGN_BYTES)?;
    let metadata: Value = serde_json::from_slice(&bytes).map_err(invalid)?;
    let text = std::str::from_utf8(&bytes).map_err(invalid)?;
    // Migrations and all document validation run on the small parametric metadata.
    // Binary arrays have their own equivalent numeric/index validation below.
    let mut doc = Document::from_json(text)?;
    for (index, feature) in doc.features.iter_mut().enumerate() {
        let value = metadata.get("features").and_then(|v| v.get(index)).ok_or_else(|| invalid("missing feature metadata"))?;
        match &mut feature.kind {
            FeatureKind::Import { step, .. } => {
                if !step.is_empty() {
                    return Err(invalid("ambiguous embedded STEP and blob"));
                }
                *step = std::str::from_utf8(reader.blob(value, "step_blob", "step")?).map_err(invalid)?.into();
            }
            FeatureKind::MeshImport { meshes, .. } => {
                for (index, mesh) in meshes.iter_mut().enumerate() {
                    if !mesh.positions.is_empty() || !mesh.triangles.is_empty() {
                        return Err(invalid("ambiguous embedded mesh and blob"));
                    }
                    let value = value.get("meshes").and_then(|v| v.get(index)).ok_or_else(|| invalid("missing mesh metadata"))?;
                    decode_mesh(reader.blob(value, "mesh_blob", "mesh")?, mesh)?;
                }
            }
            _ => {}
        }
    }
    for (index, canvas) in doc.canvases.iter_mut().enumerate() {
        if !canvas.data.is_empty() || !matches!(canvas.format.as_str(), "png" | "jpeg") {
            return Err(invalid("invalid canvas metadata"));
        }
        let value = metadata.get("canvases").and_then(|v| v.get(index)).ok_or_else(|| invalid("missing canvas metadata"))?;
        let bytes = reader.blob(value, "image_blob", &canvas.format)?;
        if bytes.len() > solidcraft_doc::canvas::MAX_CANVAS_BYTES {
            return Err(invalid("canvas too large"));
        }
        canvas.data = solidcraft_doc::canvas::base64_encode(bytes);
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcraft_geom::Vec2;

    struct CountingReader {
        source: Cursor<Vec<u8>>,
        count: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl Read for CountingReader {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let count = self.source.read(bytes)?;
            self.count.set(self.count.get() + count);
            Ok(count)
        }
    }

    impl Seek for CountingReader {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.source.seek(position)
        }
    }

    #[test]
    fn seekable_reader_rejects_oversized_source_before_reading() {
        struct Oversized;
        impl Read for Oversized {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("oversized input must be rejected before any allocation or read");
            }
        }
        impl Seek for Oversized {
            fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
                Ok(MAX_DESIGN_BYTES as u64 + 1)
            }
        }
        assert!(DesignContainerReader::new(Oversized).is_err());
    }

    #[test]
    fn seekable_preview_defers_large_source_blob_until_document_read() {
        let mut doc = Document::new("Deferred source");
        let step = "ISO-10303-21;".repeat(200_000);
        doc.add_feature(FeatureKind::Import { file: "large.step".into(), step: step.clone().into(), components: Vec::new(), skip: Vec::new() }, None)
            .unwrap();
        let preview = DesignPreview { meshes: vec![mesh()] };
        let bytes = write_design_container_with_preview(&doc, DesignCompression::Fast, Some(&preview)).unwrap();
        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let source = CountingReader { source: Cursor::new(bytes), count: count.clone() };
        let mut reader = DesignContainerReader::new(source).unwrap();
        assert_eq!(reader.preview().unwrap().meshes[0].positions, preview.meshes[0].positions);
        // ZIP directory scans may touch a bounded suffix of a stored blob. They
        // must not load its multi-megabyte payload to obtain the saved picture.
        assert!(count.get() < 256 * 1024, "read {} bytes before display", count.get());
        let restored = reader.read_document().unwrap();
        assert_eq!(restored.to_json(), doc.to_json());
        assert!(count.get() >= step.len());
    }

    #[test]
    fn seekable_reader_validates_required_blobs_after_preview() {
        let doc = document();
        let preview = DesignPreview { meshes: vec![mesh()] };
        let mut bytes = write_design_container_with_preview(&doc, DesignCompression::Fast, Some(&preview)).unwrap();
        let marker = b"ISO-10303-21;";
        let offset = bytes.windows(marker.len()).position(|part| part == marker).unwrap();
        bytes[offset] ^= 1;
        let mut reader = DesignContainerReader::new(Cursor::new(bytes)).unwrap();
        assert!(reader.preview().is_some());
        assert!(reader.read_document().is_err());
    }

    #[test]
    fn seekable_reader_without_preview_preserves_authoritative_document() {
        let doc = document();
        let bytes = write_design_container(&doc).unwrap();
        let mut source = Cursor::new(bytes);
        source.set_position(17); // Constructor starts from the whole source, not its cursor.
        let mut reader = DesignContainerReader::new(source).unwrap();
        assert!(reader.preview().is_none());
        assert_eq!(reader.read_document().unwrap().to_json(), doc.to_json());
        assert!(DesignContainerReader::new(Cursor::new(vec![0u8; 1])).is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn file_reader_refuses_nonfiles_before_notifying_preview() {
        let directory = std::env::temp_dir();
        let result = read_design_file(directory.to_str().unwrap(), Some(|_| panic!("non-file supplied a preview")));
        assert!(result.is_err_and(|e| e.to_string().contains("not a regular file")));
    }

    fn native_geometry() -> Vec<DesignGeometry> {
        vec![DesignGeometry {
            name: "Cube".into(),
            body: solidcraft_kernel::box_solid(solidcraft_geom::Vec3::ZERO, solidcraft_geom::Vec3::new(10., 20., 30.)).unwrap(),
        }]
    }

    #[test]
    fn native_cache_round_trip_and_preview_defer_original_sources() {
        let mut doc = Document::new("Native cache");
        let step = "ISO-10303-21;".repeat(200_000);
        doc.add_feature(FeatureKind::Import { file: "large.step".into(), step: step.clone().into(), components: Vec::new(), skip: Vec::new() }, None)
            .unwrap();
        let bodies = native_geometry();
        let bytes = write_design_container_with_cache(&doc, DesignCompression::Fast, None, Some(&bodies)).unwrap();
        let entries = parts(&bytes);
        assert!(entries.contains_key("cache/brep.bin"));
        assert!(!entries.contains_key("cache/preview.bin"));
        assert_eq!(read_design_geometry(&bytes).unwrap()[0].body.face_count(), bodies[0].body.face_count());
        assert_eq!(bytes, write_design_container_with_cache(&doc, DesignCompression::Fast, None, Some(&bodies)).unwrap());
        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut reader = DesignContainerReader::new(CountingReader { source: Cursor::new(bytes), count: count.clone() }).unwrap();
        assert!(!reader.preview().unwrap().meshes.is_empty());
        assert!(count.get() < 256 * 1024, "read {} bytes before native display", count.get());
        assert_eq!(reader.read_document().unwrap().to_json(), doc.to_json());
        assert!(count.get() >= step.len());
    }

    #[test]
    fn native_cache_mismatch_is_only_a_cache_miss() {
        let doc = document();
        let bytes = write_design_container_with_cache(&doc, DesignCompression::Fast, None, Some(&native_geometry())).unwrap();
        let original = parts(&bytes);
        for key in ["format", "producer", "app", "document", "payload"] {
            let mut entries = original.clone();
            let mut manifest: Value = serde_json::from_slice(&entries["cache/key.json"]).unwrap();
            manifest[key] = json!("unsupported-or-stale");
            entries.insert("cache/key.json".into(), serde_json::to_vec(&manifest).unwrap());
            let bytes = package(&entries);
            assert!(read_design_geometry(&bytes).is_none());
            assert!(read_design_preview(&bytes).is_none());
            assert_eq!(crate::read_design(&bytes).unwrap().to_json(), doc.to_json());
        }
        let mut entries = original;
        entries.get_mut("cache/brep.bin").unwrap()[0] ^= 1;
        let bytes = package(&entries);
        assert!(read_design_geometry(&bytes).is_none());
        assert_eq!(crate::read_design(&bytes).unwrap().to_json(), doc.to_json());
    }

    #[test]
    fn metadata_snapshot_survives_in_place_changes_after_preview() {
        use std::{cell::RefCell, rc::Rc};
        struct Shared {
            bytes: Rc<RefCell<Vec<u8>>>,
            position: u64,
        }
        impl Read for Shared {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let bytes = self.bytes.borrow();
                let mut cursor = Cursor::new(bytes.as_slice());
                cursor.set_position(self.position);
                let n = cursor.read(out)?;
                self.position = cursor.position();
                Ok(n)
            }
        }
        impl Seek for Shared {
            fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
                let bytes = self.bytes.borrow();
                let mut cursor = Cursor::new(bytes.as_slice());
                cursor.set_position(self.position);
                self.position = cursor.seek(pos)?;
                Ok(self.position)
            }
        }
        let doc = document();
        let preview = DesignPreview { meshes: vec![mesh()] };
        let bytes = write_design_container_with_preview(&doc, DesignCompression::Fast, Some(&preview)).unwrap();
        let offset = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap().by_name("document.json").unwrap().data_start().unwrap() as usize;
        let bytes = Rc::new(RefCell::new(bytes));
        let mut reader = DesignContainerReader::new(Shared { bytes: bytes.clone(), position: 0 }).unwrap();
        assert!(reader.preview().is_some());
        bytes.borrow_mut()[offset] ^= 0xff;
        assert_eq!(reader.read_document().unwrap().to_json(), doc.to_json());
        assert!(crate::read_design(&bytes.borrow()).is_err());
    }

    #[test]
    fn native_cache_prefixes_and_mutations_never_panic() {
        let bytes = write_design_container_with_cache(&document(), DesignCompression::Fast, None, Some(&native_geometry())).unwrap();
        for n in (0..bytes.len()).step_by((bytes.len() / 128).max(1)) {
            assert!(
                std::panic::catch_unwind(|| {
                    let _ = read_design_preview(&bytes[..n]);
                    let _ = read_design_geometry(&bytes[..n]);
                })
                .is_ok()
            );
            let mut changed = bytes.clone();
            changed[n] ^= 0xff;
            assert!(
                std::panic::catch_unwind(|| {
                    let _ = read_design_preview(&changed);
                    let _ = read_design_geometry(&changed);
                })
                .is_ok()
            );
        }
    }

    fn mesh() -> MeshData {
        MeshData {
            name: "Precise mesh".into(),
            positions: vec![0.123456789012345, -0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            triangles: vec![0, 1, 2],
            color: Some([0.25, 0.5, 0.75]),
        }
    }

    fn document() -> Document {
        let mut doc = Document::new("Payloads");
        for _ in 0..2 {
            doc.add_feature(
                FeatureKind::Import {
                    file: "example.step".into(),
                    step: "ISO-10303-21;\nEND-ISO-10303-21;".into(),
                    components: Vec::new(),
                    skip: Vec::new(),
                },
                None,
            )
            .unwrap();
        }
        let mut other = mesh();
        other.name = "Same triangles, separate appearance".into();
        other.color = None;
        doc.add_feature(FeatureKind::MeshImport { file: "example.obj".into(), meshes: vec![mesh(), other] }, None).unwrap();
        doc.canvases.push(solidcraft_doc::canvas::Canvas {
            id: 99,
            name: "Reference".into(),
            format: "png".into(),
            data: solidcraft_doc::canvas::base64_encode(b"original image bytes"),
            pixels: [1, 1],
            plane: solidcraft_doc::PlaneRef::Origin { name: "XY".into() },
            center: Vec2::new(0.0, 0.0),
            width: 10.0,
            angle: 0.0,
            opacity: 0.5,
            flip: false,
            visible: true,
            component: 0,
        });
        doc
    }

    fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..zip.len())
            .map(|i| {
                let mut entry = zip.by_index(i).unwrap();
                let name = entry.name().unwrap().into_owned();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                (name, bytes)
            })
            .collect()
    }

    fn package(parts: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in parts {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn portable_thumbnail_is_keyed_bounded_and_does_not_read_sources() {
        // IO validates an opaque PNG header/hash; UI tests exercise real PNG decoding.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(240u32.to_be_bytes());
        png.extend(150u32.to_be_bytes());
        let mut doc = Document::from_json(&document().to_json()).unwrap();
        if let FeatureKind::Import { step, .. } = &mut doc.features[0].kind {
            *step = "source data".repeat(250_000).into();
        }
        let bytes = write_design_container_with_thumbnail(&doc, DesignCompression::Fast, None, None, Some(&png)).unwrap();
        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut reader = DesignContainerReader::new(CountingReader { source: Cursor::new(bytes.clone()), count: count.clone() }).unwrap();
        assert_eq!(reader.thumbnail(), Some(png.clone()));
        assert!(count.get() < 256 * 1024);
        assert_eq!(reader.read_document().unwrap(), doc);
        let original = parts(&bytes);
        for field in ["format", "app", "document", "payload"] {
            let mut entries = original.clone();
            let mut key: Value = serde_json::from_slice(&entries["cache/thumbnail.json"]).unwrap();
            key[field] = json!("mismatch");
            entries.insert("cache/thumbnail.json".into(), serde_json::to_vec(&key).unwrap());
            let changed = package(&entries);
            assert!(read_design_thumbnail(&changed).is_none());
            assert_eq!(crate::read_design(&changed).unwrap(), doc);
        }
        let mut entries = original.clone();
        let source = entries.keys().find(|k| k.ends_with(".step")).unwrap().clone();
        entries.insert(source, b"corrupt source".to_vec());
        let changed = package(&entries);
        assert_eq!(read_design_thumbnail(&changed), Some(png));
        assert!(crate::read_design(&changed).is_err());
        for n in [0, 1, 8, bytes.len() / 2, bytes.len() - 1] {
            assert!(read_design_thumbnail(&bytes[..n]).is_none());
        }
        for i in (0..bytes.len()).step_by((bytes.len() / 128).max(1)) {
            let mut changed = bytes.clone();
            changed[i] ^= 0xff;
            assert!(std::panic::catch_unwind(|| read_design_thumbnail(&changed)).is_ok());
        }
        for invalid in [vec![], vec![0; 300_000]] {
            let bytes = write_design_container_with_thumbnail(&doc, DesignCompression::Fast, None, None, Some(&invalid)).unwrap();
            assert!(read_design_thumbnail(&bytes).is_none());
            assert_eq!(crate::read_design(&bytes).unwrap(), doc);
        }
    }

    #[test]
    fn optional_preview_is_bounded_keyed_and_never_authoritative() {
        let doc = Document::from_json(&document().to_json()).unwrap();
        let preview = DesignPreview { meshes: vec![mesh()] };
        let bytes = write_design_container_with_preview(&doc, DesignCompression::Fast, Some(&preview)).unwrap();
        let loaded = read_design_preview(&bytes).unwrap();
        assert_eq!(loaded.meshes, preview.meshes);
        assert_eq!(crate::read_design(&bytes).unwrap(), doc);
        let original = parts(&bytes);
        for field in ["format", "build", "document", "payload"] {
            let mut entries = original.clone();
            let mut manifest: Value = serde_json::from_slice(&entries["cache/preview.json"]).unwrap();
            manifest[field] = json!("obsolete");
            entries.insert("cache/preview.json".into(), serde_json::to_vec(&manifest).unwrap());
            let changed = package(&entries);
            assert!(read_design_preview(&changed).is_none(), "{field}");
            assert_eq!(crate::read_design(&changed).unwrap(), doc);
        }
        let mut entries = original.clone();
        let mut metadata: Value = serde_json::from_slice(&entries["document.json"]).unwrap();
        metadata["name"] = json!("Changed document");
        entries.insert("document.json".into(), serde_json::to_vec(&metadata).unwrap());
        assert!(read_design_preview(&package(&entries)).is_none());
        for damaged in [vec![], vec![0; 128], vec![0; MAX_PREVIEW_BYTES + 1]] {
            let mut entries = original.clone();
            entries.insert("cache/preview.bin".into(), damaged);
            let changed = package(&entries);
            assert!(read_design_preview(&changed).is_none());
            assert_eq!(crate::read_design(&changed).unwrap(), doc);
        }
        // Even a matching checksum does not excuse invalid indices or overlapping slices.
        for overlap in [false, true] {
            let mut entries = original.clone();
            let mut manifest: Value = serde_json::from_slice(&entries["cache/preview.json"]).unwrap();
            if overlap {
                manifest["meshes"][0]["offset"] = json!(1);
            } else {
                let data = entries.get_mut("cache/preview.bin").unwrap();
                let end = data.len();
                data[end - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
                manifest["payload"] = json!(digest(data));
            }
            entries.insert("cache/preview.json".into(), serde_json::to_vec(&manifest).unwrap());
            assert!(read_design_preview(&package(&entries)).is_none());
        }
        let mut too_large = mesh();
        too_large.triangles = vec![0; (MAX_PREVIEW_TRIANGLES + 1) * 3];
        let bytes = write_design_container_with_preview(&doc, DesignCompression::Fast, Some(&DesignPreview { meshes: vec![too_large] })).unwrap();
        assert!(read_design_preview(&bytes).is_none());
        assert_eq!(crate::read_design(&bytes).unwrap(), doc);
        assert!(read_design_preview(&crate::write_design(&doc)).is_none());
    }

    #[test]
    fn round_trip_deduplicates_and_preserves_binary_precision() {
        let source = Document::from_json(&document().to_json()).unwrap();
        let bytes = write_design_container(&source).unwrap();
        assert!(is_design_container(&bytes));
        let reopened = crate::read_design(&bytes).unwrap();
        assert_eq!(source, reopened);
        let FeatureKind::MeshImport { meshes, .. } = &reopened.features[2].kind else { panic!() };
        assert_eq!(meshes[0].positions[1].to_bits(), (-0.0f64).to_bits());
        let entries = parts(&bytes);
        assert_eq!(entries.keys().filter(|n| n.ends_with(".step")).count(), 1);
        assert_eq!(entries.keys().filter(|n| n.ends_with(".mesh")).count(), 1);
        assert_eq!(entries.keys().filter(|n| n.ends_with(".png")).count(), 1);
        let json: Value = serde_json::from_slice(&entries["document.json"]).unwrap();
        assert_eq!(json["features"][2]["meshes"][0]["positions"], json!([]));
        assert_eq!(crate::read_design(&crate::write_design(&reopened)).unwrap(), source);
        assert_eq!(write_design_container(&source).unwrap(), bytes, "deterministic archive");
        let compact = write_design_container_with_compression(&source, DesignCompression::Compact).unwrap();
        assert_eq!(crate::read_design(&compact).unwrap(), source);
    }

    #[test]
    fn draft_container_marker_remains_readable() {
        let doc = Document::from_json(&document().to_json()).unwrap();
        let mut entries = parts(&write_design_container(&doc).unwrap());
        let marker: Value = serde_json::from_slice(&entries["container.json"]).unwrap();
        assert_eq!(marker["format"], VERSION);
        entries.insert("container.json".into(), serde_json::to_vec(&json!({"format": LEGACY_VERSION})).unwrap());
        assert_eq!(crate::read_design(&package(&entries)).unwrap(), doc);
    }

    #[test]
    fn required_blobs_must_exist_and_match_hashes() {
        let bytes = write_design_container(&document()).unwrap();
        let mut entries = parts(&bytes);
        let name = entries.keys().find(|n| n.ends_with(".step")).unwrap().clone();
        let original = entries.remove(&name).unwrap();
        assert!(crate::read_design(&package(&entries)).unwrap_err().to_string().contains("required part"));
        entries.insert(name.clone(), b"damaged".to_vec());
        assert!(crate::read_design(&package(&entries)).unwrap_err().to_string().contains("hash mismatch"));
        entries.insert(name, original);
        entries.insert("cache/obsolete.bin".into(), b"untrusted optional cache".to_vec());
        assert!(crate::read_design(&package(&entries)).is_ok());
    }

    #[test]
    fn refuses_hostile_mesh_lengths_numbers_indices_and_versions() {
        let valid = encode_mesh(&mesh()).unwrap();
        for offset in [8, 16] {
            let mut bytes = valid.clone();
            bytes[offset..offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
            assert!(decode_mesh(&bytes, &mut mesh()).is_err());
        }
        let mut bytes = valid.clone();
        bytes[24..32].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(decode_mesh(&bytes, &mut mesh()).is_err());
        let mut bytes = valid.clone();
        let offset = bytes.len() - 4;
        bytes[offset..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_mesh(&bytes, &mut mesh()).is_err());
        for length in 0..valid.len() {
            assert!(decode_mesh(&valid[..length], &mut mesh()).is_err());
        }
        let mut bytes = valid;
        bytes[7] = b'2';
        assert!(decode_mesh(&bytes, &mut mesh()).is_err());
    }

    #[test]
    fn hostile_archive_paths_versions_and_truncations_are_errors() {
        let bytes = write_design_container(&document()).unwrap();
        let mut entries = parts(&bytes);
        entries.insert("../escape".into(), vec![]);
        assert!(crate::read_design(&package(&entries)).is_err());
        entries.remove("../escape");
        entries.insert("container.json".into(), br#"{"format":"solidcraft-container/999"}"#.to_vec());
        assert!(crate::read_design(&package(&entries)).unwrap_err().to_string().contains("unsupported container version"));
        for length in (0..bytes.len()).step_by(17) {
            assert!(crate::read_design(&bytes[..length]).is_err());
        }
    }

    #[test]
    fn rejects_duplicate_entries_and_declared_expansion_before_reading_payloads() {
        let bytes = write_design_container(&document()).unwrap();
        let mut entries = parts(&bytes);
        entries.insert("cache/one.bin".into(), vec![1]);
        entries.insert("cache/two.bin".into(), vec![2]);
        let mut duplicate = package(&entries);
        let offsets: Vec<_> = duplicate.windows(13).enumerate().filter_map(|(i, v)| (v == b"cache/two.bin").then_some(i)).collect();
        // Both the local header and central-directory name are changed.
        for i in offsets {
            duplicate[i..i + 13].copy_from_slice(b"cache/one.bin");
        }
        assert!(crate::read_design(&duplicate).is_err());
        let mut bomb = bytes.clone();
        let central = bomb.windows(4).position(|v| v == b"PK\x01\x02").unwrap();
        bomb[central + 24..central + 28].copy_from_slice(&((MAX_DESIGN_BYTES + 1) as u32).to_le_bytes());
        assert!(crate::read_design(&bomb).unwrap_err().to_string().contains("expanded package too large"));
        let mut excessive = bytes;
        let end = excessive.windows(4).rposition(|v| v == b"PK\x05\x06").unwrap();
        excessive[end + 8..end + 12].copy_from_slice(&[0xff; 4]);
        assert!(crate::read_design(&excessive).is_err());
    }

    #[test]
    fn rejects_invalid_references_and_repeated_payload_expansion() {
        let bytes = write_design_container(&document()).unwrap();
        let mut entries = parts(&bytes);
        let mut metadata: Value = serde_json::from_slice(&entries["document.json"]).unwrap();
        let reference = metadata["features"][0].clone();
        let mut reader = PackageReader::new(Cursor::new(&bytes)).unwrap();
        reader.hydrated = MAX_EXPANDED;
        assert!(reader.blob(&reference, "step_blob", "step").unwrap_err().to_string().contains("resolved payloads too large"));
        for name in ["../escape.step", "blobs/not-a-hash.step", "blobs/00.png"] {
            metadata["features"][0]["step_blob"] = json!(name);
            entries.insert("document.json".into(), serde_json::to_vec(&metadata).unwrap());
            assert!(crate::read_design(&package(&entries)).is_err());
        }
    }

    #[test]
    fn deterministic_archive_mutations_never_panic() {
        let preview = DesignPreview { meshes: vec![mesh()] };
        let bytes = write_design_container_with_preview(&document(), DesignCompression::Fast, Some(&preview)).unwrap();
        assert!(read_design_preview(&bytes).is_some());
        for end in 0..bytes.len() {
            assert!(
                std::panic::catch_unwind(|| {
                    let _ = crate::read_design(&bytes[..end]);
                    let _ = read_design_preview(&bytes[..end]);
                    if let Ok(mut reader) = DesignContainerReader::new(Cursor::new(&bytes[..end])) {
                        let _ = reader.preview();
                        let _ = reader.read_document();
                    }
                })
                .is_ok(),
                "truncated at {end}"
            );
        }
        let mut seed = 0x303_u64;
        for _ in 0..512 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut changed = bytes.clone();
            let index = seed as usize % changed.len();
            changed[index] ^= (seed >> 32) as u8 | 1;
            assert!(
                std::panic::catch_unwind(|| {
                    let _ = crate::read_design(&changed);
                    let _ = read_design_preview(&changed);
                    if let Ok(mut reader) = DesignContainerReader::new(Cursor::new(&changed)) {
                        let _ = reader.preview();
                        let _ = reader.read_document();
                    }
                })
                .is_ok()
            );
        }
    }

    #[test]
    fn metadata_round_trip_preserves_new_and_existing_fields() {
        let mut doc = Document::new("Empty");
        doc.units = "in".into();
        doc.named_views.push(solidcraft_doc::NamedView { name: "View".into(), ..Default::default() });
        doc.param_comments.insert("test".into(), "metadata".into());
        let setup = solidcraft_doc::render_settings::RenderSetup { exposure_ev: 1.5, ..Default::default() };
        std::sync::Arc::make_mut(&mut doc.render).setups.insert("Studio".into(), setup);
        // Design properties and other tools' data (#1015), kept as they are in both forms.
        doc.metadata = serde_json::from_value(json!({
            "author": 42,
            "revision": "B",
            "com.acme.description": {"shop_id": 88123, "list": [1, 2.5, null, true, "x", {"deep": []}]},
        }))
        .unwrap();
        let expected = crate::read_design(&crate::write_design(&doc)).unwrap();
        assert_eq!(expected, doc);
        assert_eq!(crate::read_design(&write_design_container(&doc).unwrap()).unwrap(), expected);
    }

    /// Public-safe generated data, no geometry evaluation. Print medians without
    /// timing assertions, which would be unreliable on shared build machines.
    #[test]
    #[ignore = "manual IO performance measurement"]
    fn benchmark_design_storage() {
        let mut doc = Document::new("Generated mesh benchmark");
        let mut large = mesh();
        large.positions = (0..300_000).map(|i| (i as f64 * 0.001234567890123).sin() * 100.0).collect();
        large.triangles = (0..300_000).map(|i| (i % 100_000) as u32).collect();
        doc.add_feature(FeatureKind::MeshImport { file: "generated.obj".into(), meshes: vec![large] }, None).unwrap();
        measure_storage(&doc, "mesh");
    }

    #[test]
    #[ignore = "manual IO performance measurement"]
    fn benchmark_step_storage() {
        use solidcraft_geom::Vec3;
        let bodies: Vec<_> = (0..512)
            .map(|i| {
                let corner = Vec3::new((i % 32) as f64 * 12.0, (i / 32) as f64 * 12.0, 0.0);
                solidcraft_kernel::box_solid(corner, corner + Vec3::new(10.0, 10.0, 10.0)).unwrap()
            })
            .collect();
        let step = solidcraft_kernel::step_export(&bodies.iter().collect::<Vec<_>>(), "SolidCraft storage benchmark").unwrap();
        let mut doc = Document::new("Generated STEP benchmark");
        doc.add_feature(FeatureKind::Import { file: "generated.step".into(), step: step.into(), components: Vec::new(), skip: Vec::new() }, None)
            .unwrap();
        measure_storage(&doc, "step");
    }

    fn measure_storage(doc: &Document, label: &str) {
        let json = crate::write_design(doc);
        let container = write_design_container(doc).unwrap();
        let dir = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("target"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("issue303-{label}.json")), &json).unwrap();
        std::fs::write(dir.join(format!("issue303-{label}.solidcraft")), &container).unwrap();
        let mut timings = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for _ in 0..9 {
            let start = std::time::Instant::now();
            std::hint::black_box(crate::write_design(doc));
            timings[0].push(start.elapsed().as_secs_f64() * 1000.0);
            let start = std::time::Instant::now();
            std::hint::black_box(write_design_container(doc).unwrap());
            timings[1].push(start.elapsed().as_secs_f64() * 1000.0);
            let start = std::time::Instant::now();
            let plain = crate::read_design(&json).unwrap();
            timings[2].push(start.elapsed().as_secs_f64() * 1000.0);
            let start = std::time::Instant::now();
            let zipped = crate::read_design(&container).unwrap();
            timings[3].push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(plain, zipped);
        }
        for times in &mut timings {
            times.sort_by(f64::total_cmp);
        }
        eprintln!(
            "{label}: bytes json={} container={}; median ms save_json={:.3} save_container={:.3} read_json={:.3} read_container={:.3}",
            json.len(),
            container.len(),
            timings[0][4],
            timings[1][4],
            timings[2][4],
            timings[3][4]
        );
        let mut saves = [Vec::new(), Vec::new()];
        for round in 0..9 {
            // Alternate ordering; include encoding, flushing, replacement and backup.
            for index in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = std::time::Instant::now();
                let bytes = if index == 0 { crate::write_design(doc) } else { write_design_container(doc).unwrap() };
                crate::write_atomic(&dir.join(format!("issue303-{label}-atomic-{index}.solidcraft")), &bytes, true).unwrap();
                saves[index].push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        for times in &mut saves {
            times.sort_by(f64::total_cmp);
        }
        eprintln!("{label}: median atomic-save ms json={:.3} container={:.3}", saves[0][4], saves[1][4]);
    }
}
