//! glTF 2.0 import (`.glb`, `.gltf` with its `.bin` files next to it or embedded as `data:`
//! URIs): triangle meshes become mesh bodies, the node tree a component plan (#1505).
//!
//! The `gltf` crate reads the JSON document and checks its references; the GLB container and
//! every buffer, view and accessor are read here, with each length checked against the bytes
//! that are really there. The file is untrusted: sizes, counts, nesting and the nodes visited
//! are capped, and anything wrong is an error or a warning, never a panic.
//!
//! Geometry: glTF is in metres with Y up. Coordinates are scaled to millimetres here; turning
//! Y up into Z up is the caller's (the import component's placement, as for STEP). A node's
//! placement becomes an occurrence when it can be one (the rigid part of its transform); what
//! is left (scale, shear, a mirror) is baked into the node's meshes, so the hierarchy is kept
//! even under a scaled parent. A node that places the same mesh (and the same subtree) the
//! same way as an earlier one is another occurrence of that node's component.
//!
//! Materials: one body per mesh and material; the material's base colour (linear in glTF,
//! sRGB here), alpha (blended materials only) and metallic/roughness factors give the body's
//! look. Textures are not read.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use gltf::accessor::{DataType, Dimensions};
use gltf::mesh::Mode;
use solidcraft_doc::appearance::Look;
use solidcraft_doc::{Mat, MeshData, mat_mul};
use solidcraft_geom::Vec3;

use crate::{IoError, MAX_3MF_BYTES, MeshFeature, Result};

/// Largest `.glb` / `.gltf` file read.
pub const MAX_GLTF_BYTES: usize = MAX_3MF_BYTES;
/// Most bytes of external and embedded buffers read for one file.
const MAX_BUFFER_BYTES: usize = 1 << 30;
/// Most vertices and triangles, over all bodies.
const MAX_VERTICES: usize = 20_000_000;
const MAX_TRIANGLES: usize = 20_000_000;
/// Most node placements followed (a node tree that shares or loops is cut off here).
const MAX_PLACEMENTS: usize = 100_000;
/// Deepest nesting kept as components; deeper nodes are baked into their parent's.
const MAX_COMPONENT_DEPTH: usize = 32;
/// Deepest nesting followed at all.
const MAX_DEPTH: usize = 64;
/// Most components one file makes.
const MAX_COMPONENTS: usize = 5_000;
/// Millimetres in a metre.
const MM: f64 = 1000.0;

/// Is this a glTF file (`.glb`, `.gltf`, any case)?
pub fn is_gltf_path(path: &str) -> bool {
    std::path::Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("glb") || e.eq_ignore_ascii_case("gltf"))
}

/// A placement of a component of the plan in another (`parent`, or the import's own component
/// when `None`), by a rigid transform in millimetres.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanPlace {
    pub slot: usize,
    pub parent: Option<usize>,
    pub transform: Mat,
}

/// How a glTF file's nodes become components below the import's own: `slots` are the
/// components (by name), `places` their placements (the first one makes the component, later
/// ones are more occurrences of it), and `bodies` the component each mesh body goes into
/// (`None`: the import's own). Each body's coordinates are already in its component's frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshPlan {
    pub slots: Vec<String>,
    pub places: Vec<PlanPlace>,
    pub bodies: Vec<Option<usize>>,
}

/// A glTF file read for import.
pub struct GltfImport {
    /// The MeshImport feature: one mesh body per glTF mesh and material.
    pub feature: MeshFeature,
    /// Where the bodies go.
    pub plan: MeshPlan,
    /// Each body's look from its material (`None`: no material).
    pub looks: Vec<Option<Look>>,
}

impl GltfImport {
    /// Build every body where its component's first placement puts it, in the import's own
    /// frame, and drop the components.
    pub fn flatten(&mut self) {
        if self.plan.places.is_empty() {
            return;
        }
        let mut world: Vec<Option<Mat>> = vec![None; self.plan.slots.len()];
        for p in &self.plan.places {
            let parent = match p.parent {
                None => Some(solidcraft_doc::IDENTITY),
                Some(q) => world.get(q).copied().flatten(),
            };
            if let (Some(parent), Some(slot @ None)) = (parent, world.get_mut(p.slot)) {
                *slot = Some(mat_mul(&parent, &p.transform));
            }
        }
        if let solidcraft_doc::FeatureKind::MeshImport { meshes, .. } = &mut self.feature.kind {
            for (m, slot) in meshes.iter_mut().zip(&self.plan.bodies) {
                let Some(frame) = slot.and_then(|s| world.get(s).copied().flatten()) else { continue };
                for p in m.positions.as_chunks_mut::<3>().0 {
                    let q = solidcraft_doc::apply_point(&frame, Vec3::new(p[0], p[1], p[2]));
                    *p = [q.x, q.y, q.z];
                }
            }
        }
        self.plan = MeshPlan::default();
    }
}

/// Reads an external file the glTF names (`uri`, relative to the glTF file's folder, already
/// percent-decoded and checked to stay in that folder).
pub type Loader<'a> = &'a mut dyn FnMut(&str) -> std::result::Result<Vec<u8>, String>;

/// Read a `.glb` or `.gltf` file (`file` names it; its stem names the meshes when nothing in the
/// file does). External buffers are read through `load`.
pub fn read_gltf(bytes: &[u8], file: &str, load: Loader<'_>) -> Result<GltfImport> {
    if bytes.len() > MAX_GLTF_BYTES {
        return Err(IoError::Invalid(format!("glTF file too large ({} MB)", bytes.len() >> 20)));
    }
    let (json, blob) = if bytes.starts_with(b"glTF") { split_glb(bytes)? } else { (bytes, None) };
    let doc = parse_document(json)?;
    let stem = std::path::Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mesh".into());
    let mut reader = Reader {
        doc: &doc,
        buffers: Buffers { blob, data: HashMap::new(), total: 0, load },
        meshes: Vec::new(),
        looks: Vec::new(),
        plan: MeshPlan::default(),
        keys: HashMap::new(),
        has_mesh: HashMap::new(),
        open: Vec::new(),
        slot_meshes: Vec::new(),
        placements: 0,
        vertices: 0,
        triangles: 0,
        skipped: Vec::new(),
        cut_off: false,
        stem: stem.clone(),
    };
    let roots = scene_roots(&doc);
    if roots.is_empty() {
        return Err(IoError::Invalid("glTF has no nodes to import".into()));
    }
    // One root node is the import's own component (its placement baked in); several are
    // components in it.
    let units = scale(MM);
    if let [root] = roots.as_slice() {
        let m = reader.node(*root).map(|n| mat_mul(&units, &node_matrix(&n))).unwrap_or(units);
        reader.walk_flat(*root, None, &m, 0)?;
    } else {
        for r in &roots {
            reader.walk(*r, None, &units, 0)?;
        }
    }
    if reader.meshes.is_empty() {
        let why = reader.skipped.first().map(|s| format!(": {s}")).unwrap_or_default();
        return Err(IoError::Invalid(format!("glTF has no triangle meshes{why}")));
    }
    let mut warnings = Vec::new();
    if !reader.skipped.is_empty() {
        let n = reader.skipped.len();
        let first = reader.skipped.first().cloned().unwrap_or_default();
        warnings.push(if n == 1 { format!("left out: {first}") } else { format!("{n} parts left out (first: {first})") });
    }
    if reader.cut_off {
        warnings.push(format!("the node tree is too large or too deep: only the first {MAX_PLACEMENTS} placements were read"));
    }
    if doc.textures().len() > 0 {
        warnings.push("textures are not imported: bodies take their materials' base colours".into());
    }
    if doc.skins().len() > 0 || doc.animations().len() > 0 {
        warnings.push("skins and animations are not imported: meshes are placed in their rest pose".into());
    }
    let name = match reader.meshes.as_slice() {
        [only] if !only.name.trim().is_empty() => only.name.clone(),
        _ => stem,
    };
    let file_name = std::path::Path::new(file).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| file.to_string());
    let body_names = reader.meshes.iter().map(|m| m.name.clone()).collect();
    Ok(GltfImport {
        feature: MeshFeature { name, body_names, kind: solidcraft_doc::FeatureKind::MeshImport { file: file_name, meshes: reader.meshes }, warnings },
        plan: reader.plan,
        looks: reader.looks,
    })
}

/// The JSON and binary chunks of a GLB container.
fn split_glb(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    let bad = |m: &str| IoError::Invalid(format!("GLB: {m}"));
    let u32_at = |at: usize| bytes.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let version = u32_at(4).ok_or_else(|| bad("truncated header"))?;
    if version != 2 {
        return Err(bad(&format!("version {version} is not glTF 2.0")));
    }
    let length = u32_at(8).ok_or_else(|| bad("truncated header"))? as usize;
    if length > bytes.len() {
        return Err(bad(&format!("truncated: the header says {length} bytes, the file has {}", bytes.len())));
    }
    let mut at = 12usize;
    let (mut json, mut bin) = (None, None);
    while at + 8 <= length {
        let (Some(len), Some(kind)) = (u32_at(at), u32_at(at + 4)) else { break };
        let start = at + 8;
        let end = start.checked_add(len as usize).filter(|e| *e <= length).ok_or_else(|| bad("a chunk runs past the end of the file"))?;
        let data = bytes.get(start..end).ok_or_else(|| bad("a chunk runs past the end of the file"))?;
        match kind {
            0x4E4F_534A if json.is_none() => json = Some(data),
            0x004E_4942 if json.is_some() && bin.is_none() => bin = Some(data),
            0x4E4F_534A | 0x004E_4942 => return Err(bad("chunks out of order")),
            // Chunks of unknown types are skipped, as the specification asks.
            _ => {}
        }
        at = end;
    }
    Ok((json.ok_or_else(|| bad("no JSON chunk"))?, bin))
}

/// Required extensions that only change how a file looks (textures, material models): the
/// geometry reads without them.
const LOOK_EXTENSIONS: [&str; 5] = ["KHR_texture_transform", "KHR_texture_basisu", "EXT_texture_webp", "MSFT_texture_dds", "KHR_materials_unlit"];

/// The checked glTF document.
fn parse_document(json: &[u8]) -> Result<gltf::Document> {
    let parsed = solidcraft_kernel::guard("glTF", || {
        let mut root: gltf::json::Root = match gltf::json::deserialize::from_slice(json) {
            Ok(r) => r,
            Err(e) => return Ok(Err(format!("not a glTF file: {e}"))),
        };
        if let Some(asset) = root.asset.version.split('.').next().filter(|major| *major != "2") {
            return Ok(Err(format!("glTF version {asset} (only 2.x is read)")));
        }
        let geometry: Vec<String> = root
            .extensions_required
            .iter()
            .filter(|e| !LOOK_EXTENSIONS.contains(&e.as_str()) && !e.starts_with("KHR_materials_") && e.as_str() != "KHR_mesh_quantization")
            .cloned()
            .collect();
        if !geometry.is_empty() {
            let hint = if geometry.iter().any(|e| e.contains("draco") || e.contains("meshopt")) {
                " (compressed meshes: save the file uncompressed)"
            } else {
                ""
            };
            return Ok(Err(format!("the file needs glTF extensions that aren't supported: {}{hint}", geometry.join(", "))));
        }
        // The ones left only change looks (or are the quantized attributes read below).
        root.extensions_required.clear();
        // gltf-json's primitive check reads the POSITION accessor without a bounds check, so one
        // naming an accessor the file lacks panics inside it. The guard catches that on the
        // desktop, but in the browser a panic aborts the app: refuse such a file before (#469).
        let accessors = root.accessors.len();
        let positions = gltf::json::validation::Checked::Valid(gltf::json::mesh::Semantic::Positions);
        for (m, mesh) in root.meshes.iter().enumerate() {
            for (p, primitive) in mesh.primitives.iter().enumerate() {
                if let Some(a) = primitive.attributes.get(&positions).filter(|a| a.value() >= accessors) {
                    return Ok(Err(format!(
                        "invalid glTF: mesh {m}, primitive {p}: POSITION names accessor {}, but the file has {accessors}",
                        a.value()
                    )));
                }
            }
        }
        Ok(gltf::Document::from_json(root).map_err(|e| format!("invalid glTF: {e}")))
    });
    match parsed {
        Ok(Ok(doc)) => Ok(doc),
        Ok(Err(e)) => Err(IoError::Invalid(e)),
        Err(e) => Err(IoError::Invalid(format!("glTF could not be read: {e}"))),
    }
}

/// The nodes to import: the default scene's (else the first scene's), else every node that is
/// no other's child.
fn scene_roots(doc: &gltf::Document) -> Vec<usize> {
    if let Some(scene) = doc.default_scene().or_else(|| doc.scenes().next()) {
        return scene.nodes().map(|n| n.index()).collect();
    }
    let mut child = vec![false; doc.nodes().len()];
    for n in doc.nodes() {
        for c in n.children() {
            if let Some(x) = child.get_mut(c.index()) {
                *x = true;
            }
        }
    }
    child.iter().enumerate().filter(|(_, c)| !**c).map(|(i, _)| i).collect()
}

/// The file's buffers, read when first needed.
struct Buffers<'a, 'l> {
    blob: Option<&'a [u8]>,
    data: HashMap<usize, std::result::Result<Vec<u8>, String>>,
    total: usize,
    load: Loader<'l>,
}

impl Buffers<'_, '_> {
    fn get(&mut self, buffer: &gltf::Buffer<'_>) -> std::result::Result<&[u8], String> {
        let index = buffer.index();
        if let gltf::buffer::Source::Bin = buffer.source() {
            let blob = self.blob.ok_or("the file has no binary chunk for its buffer")?;
            if blob.len() < buffer.length() {
                return Err(format!("the binary chunk is shorter ({} bytes) than its buffer says ({})", blob.len(), buffer.length()));
            }
            return Ok(blob);
        }
        if !self.data.contains_key(&index) {
            let r = self.read(buffer);
            if let Ok(d) = &r {
                self.total = self.total.saturating_add(d.len());
            }
            self.data.insert(index, r);
        }
        match self.data.get(&index) {
            Some(Ok(d)) => Ok(d.as_slice()),
            Some(Err(e)) => Err(e.clone()),
            None => Err("buffer".into()),
        }
    }

    fn read(&mut self, buffer: &gltf::Buffer<'_>) -> std::result::Result<Vec<u8>, String> {
        let gltf::buffer::Source::Uri(uri) = buffer.source() else { return Err("buffer".into()) };
        if buffer.length() > MAX_BUFFER_BYTES.saturating_sub(self.total) {
            return Err(format!("buffers larger than {} MB in all", MAX_BUFFER_BYTES >> 20));
        }
        let data = if let Some(rest) = uri.strip_prefix("data:") {
            let (_, payload) = rest.split_once(";base64,").ok_or("an embedded buffer that isn't base64")?;
            base64(payload).ok_or("an embedded buffer with bad base64")?
        } else {
            let path = relative_path(uri)?;
            let data = (self.load)(&path).map_err(|e| format!("buffer file `{path}`: {e}"))?;
            if data.len() > MAX_BUFFER_BYTES.saturating_sub(self.total) {
                return Err(format!("buffers larger than {} MB in all", MAX_BUFFER_BYTES >> 20));
            }
            data
        };
        if data.len() < buffer.length() {
            return Err(format!("buffer `{}` is shorter ({} bytes) than the file says ({})", short(uri), data.len(), buffer.length()));
        }
        Ok(data)
    }
}

/// A URI for messages: an embedded buffer by its kind only.
fn short(uri: &str) -> String {
    if uri.starts_with("data:") { "(embedded)".into() } else { uri.chars().take(80).collect() }
}

/// A buffer's URI as a path relative to the glTF file, percent-decoded. Absolute paths, other
/// schemes and paths leaving the file's folder are refused.
fn relative_path(uri: &str) -> std::result::Result<String, String> {
    let mut bytes = Vec::with_capacity(uri.len());
    let raw = uri.as_bytes();
    let mut i = 0;
    while let Some(&b) = raw.get(i) {
        let hex = |k: usize| raw.get(k).and_then(|c| (*c as char).to_digit(16));
        match (b, hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                bytes.push((h * 16 + l) as u8);
                i += 3;
            }
            _ => {
                bytes.push(b);
                i += 1;
            }
        }
    }
    let path = String::from_utf8_lossy(&bytes).replace('\\', "/");
    let outside = path.starts_with('/') || path.contains(':') || path.split('/').any(|p| p == "..");
    if path.trim().is_empty() || outside {
        return Err(format!("buffer `{}` is not a file next to the glTF file", short(uri)));
    }
    Ok(path)
}

/// Standard base64 (padding optional).
fn base64(s: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let s = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 3);
    for chunk in s.chunks(4) {
        let mut n = 0u32;
        for (k, c) in chunk.iter().enumerate() {
            n |= u32::from(value(*c)?) << (18 - 6 * k);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        match chunk.len() {
            4 => out.extend_from_slice(&bytes),
            3 => out.extend_from_slice(&bytes[..2]),
            2 => out.push(bytes[0]),
            _ => return None,
        }
    }
    Some(out)
}

/// Uniform scale.
fn scale(s: f64) -> Mat {
    [[s, 0.0, 0.0, 0.0], [0.0, s, 0.0, 0.0], [0.0, 0.0, s, 0.0], [0.0, 0.0, 0.0, 1.0]]
}

/// A node's transform (in the file's units).
fn node_matrix(node: &gltf::Node<'_>) -> Mat {
    match node.transform() {
        gltf::scene::Transform::Matrix { matrix } => matrix.map(|c| c.map(f64::from)),
        gltf::scene::Transform::Decomposed { translation: t, rotation: r, scale: s } => {
            let [mut x, mut y, mut z, mut w] = r.map(f64::from);
            let n = (x * x + y * y + z * z + w * w).sqrt();
            if n.is_finite() && n > 1e-12 {
                (x, y, z, w) = (x / n, y / n, z / n, w / n);
            } else {
                (x, y, z, w) = (0.0, 0.0, 0.0, 1.0);
            }
            let [sx, sy, sz] = s.map(f64::from);
            let rot = [
                [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
                [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
                [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
            ];
            let col = |c: usize, k: f64| {
                let r = rot.get(c).copied().unwrap_or([0.0; 3]);
                [r[0] * k, r[1] * k, r[2] * k, 0.0]
            };
            [col(0, sx), col(1, sy), col(2, sz), [f64::from(t[0]), f64::from(t[1]), f64::from(t[2]), 1.0]]
        }
    }
}

/// `m` as a rigid placement and what is left: `m = place · rest`, `rest` without translation.
/// `None` when `m` is not finite or its translation is out of range.
fn split_rigid(m: &Mat) -> Option<(Mat, Mat)> {
    if !m.iter().flatten().all(|v| v.is_finite()) || m[3][..3].iter().any(|v| v.abs() > 1e9) {
        return None;
    }
    let col = |i: usize| Vec3::new(m[i][0], m[i][1], m[i][2]);
    let frame = (|| {
        let x = col(0).normalized()?;
        let y = (col(1) - x * col(1).dot(x)).normalized()?;
        Some((x, y, x.cross(y)))
    })();
    let (x, y, z) = frame.unwrap_or((Vec3::X, Vec3::Y, Vec3::Z));
    let place = [[x.x, x.y, x.z, 0.0], [y.x, y.y, y.z, 0.0], [z.x, z.y, z.z, 0.0], [m[3][0], m[3][1], m[3][2], 1.0]];
    // The inverse of a rotation is its transpose; the translation goes with the placement.
    let inverse_rotation = [[x.x, y.x, z.x, 0.0], [x.y, y.y, z.y, 0.0], [x.z, y.z, z.z, 0.0], [0.0, 0.0, 0.0, 1.0]];
    let mut linear = *m;
    linear[3] = [0.0, 0.0, 0.0, 1.0];
    Some((place, mat_mul(&inverse_rotation, &linear)))
}

fn hash_mat(m: &Mat, h: &mut impl Hasher) {
    for v in m.iter().flatten() {
        // Equal to within rounding is equal.
        ((v * 1e6).round() as i64).hash(h);
    }
}

fn det3(m: &Mat) -> f64 {
    let a = |r: usize, c: usize| m[c][r];
    a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0))
}

fn clean_name(s: &str) -> String {
    s.trim().chars().filter(|c| !c.is_control()).take(128).collect()
}

struct Reader<'d, 'a, 'l> {
    doc: &'d gltf::Document,
    buffers: Buffers<'a, 'l>,
    meshes: Vec<MeshData>,
    looks: Vec<Option<Look>>,
    plan: MeshPlan,
    /// Instance keys of the components made, to their slots.
    keys: HashMap<u64, usize>,
    has_mesh: HashMap<usize, bool>,
    /// Each component's mesh's name.
    slot_meshes: Vec<Option<String>>,
    /// The components being filled (the current node's ancestors).
    open: Vec<usize>,
    placements: usize,
    vertices: usize,
    triangles: usize,
    /// What was left out, and why.
    skipped: Vec<String>,
    cut_off: bool,
    stem: String,
}

impl<'d> Reader<'d, '_, '_> {
    fn node(&self, index: usize) -> Option<gltf::Node<'d>> {
        self.doc.nodes().nth(index)
    }

    /// Does the subtree of node `index` hold a mesh? (Loops count as no.)
    fn subtree_has_mesh(&mut self, index: usize, depth: usize) -> bool {
        if let Some(&known) = self.has_mesh.get(&index) {
            return known;
        }
        if depth > MAX_DEPTH {
            return false;
        }
        // In progress: a loop back here adds nothing.
        self.has_mesh.insert(index, false);
        let Some(node) = self.node(index) else { return false };
        let children: Vec<usize> = node.children().map(|c| c.index()).collect();
        let mut any = node.mesh().is_some();
        for c in children {
            any |= self.subtree_has_mesh(c, depth + 1);
        }
        self.has_mesh.insert(index, any);
        any
    }

    fn budget(&mut self, depth: usize) -> bool {
        self.placements += 1;
        if self.placements > MAX_PLACEMENTS || depth > MAX_DEPTH {
            self.cut_off = true;
            return false;
        }
        true
    }

    /// Node `index` under component `parent` (`None`: the import's own), whose frame sees the
    /// node's parent through `outer` (the parent's leftover scale, with the units).
    fn walk(&mut self, index: usize, parent: Option<usize>, outer: &Mat, depth: usize) -> Result<()> {
        if !self.subtree_has_mesh(index, 0) || !self.budget(depth) {
            return Ok(());
        }
        let Some(node) = self.node(index) else { return Ok(()) };
        let local = mat_mul(outer, &node_matrix(&node));
        let split = split_rigid(&local);
        let Some((place, rest)) = split.filter(|_| depth < MAX_COMPONENT_DEPTH && self.plan.slots.len() < MAX_COMPONENTS) else {
            // No component of its own: built where its parent is.
            return self.walk_flat(index, parent, &local, depth);
        };
        let key = self.key(index, &rest, 0);
        if let Some(&slot) = self.keys.get(&key) {
            if self.open.contains(&slot) {
                // A node inside itself (the file's tree loops): not followed.
                self.cut_off = true;
                return Ok(());
            }
            // The same mesh and subtree placed again: another occurrence. Placed by nodes of
            // other names, it goes by its mesh's.
            let name = node.name().map(clean_name).unwrap_or_default();
            if let (Some(own), Some(Some(mesh))) = (self.plan.slots.get_mut(slot), self.slot_meshes.get(slot))
                && *own != name
            {
                own.clone_from(mesh);
            }
            self.plan.places.push(PlanPlace { slot, parent, transform: place });
            return Ok(());
        }
        let slot = self.plan.slots.len();
        let name = node.name().map(clean_name).filter(|n| !n.is_empty());
        let name = name.or_else(|| node.mesh().and_then(|m| m.name().map(clean_name)).filter(|n| !n.is_empty()));
        self.plan.slots.push(name.unwrap_or_else(|| format!("Node {index}")));
        self.slot_meshes.push(node.mesh().and_then(|m| m.name().map(clean_name)).filter(|n| !n.is_empty()));
        self.plan.places.push(PlanPlace { slot, parent, transform: place });
        self.keys.insert(key, slot);
        self.node_meshes(index, Some(slot), &rest)?;
        let children: Vec<usize> = node.children().map(|c| c.index()).collect();
        self.open.push(slot);
        for c in children {
            self.walk(c, Some(slot), &rest, depth + 1)?;
        }
        self.open.pop();
        Ok(())
    }

    /// Node `index` built in component `parent`, `local` being its whole placement there (scale
    /// and all): its meshes are baked with it.
    fn walk_flat(&mut self, index: usize, parent: Option<usize>, local: &Mat, depth: usize) -> Result<()> {
        if !self.subtree_has_mesh(index, 0) || !self.budget(depth) {
            return Ok(());
        }
        let Some(node) = self.node(index) else { return Ok(()) };
        self.node_meshes(index, parent, local)?;
        let children: Vec<usize> = node.children().map(|c| c.index()).collect();
        for c in children {
            if depth + 1 < MAX_COMPONENT_DEPTH && self.plan.slots.len() < MAX_COMPONENTS {
                self.walk(c, parent, local, depth + 1)?;
            } else {
                let Some(child) = self.node(c) else { continue };
                let m = mat_mul(local, &node_matrix(&child));
                self.walk_flat(c, parent, &m, depth + 1)?;
            }
        }
        Ok(())
    }

    /// What makes two placements the same component: the node's mesh, how it is baked, and the
    /// same children placed the same way below it.
    fn key(&mut self, index: usize, rest: &Mat, depth: usize) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let Some(node) = self.node(index) else { return 0 };
        node.mesh().map(|m| m.index()).hash(&mut h);
        hash_mat(rest, &mut h);
        if depth < 8 {
            let children: Vec<usize> = node.children().map(|c| c.index()).collect();
            for c in children {
                if !self.subtree_has_mesh(c, 0) {
                    continue;
                }
                let Some(child) = self.node(c) else { continue };
                let local = mat_mul(rest, &node_matrix(&child));
                match split_rigid(&local) {
                    Some((place, child_rest)) => {
                        hash_mat(&place, &mut h);
                        self.key(c, &child_rest, depth + 1).hash(&mut h);
                    }
                    None => c.hash(&mut h),
                }
            }
        } else {
            // Too deep to compare: only the very same node is the same.
            index.hash(&mut h);
        }
        h.finish()
    }

    /// Bodies of node `index`'s mesh, baked with `bake` (its frame in component `slot`).
    fn node_meshes(&mut self, index: usize, slot: Option<usize>, bake: &Mat) -> Result<()> {
        let Some(node) = self.node(index) else { return Ok(()) };
        let Some(mesh) = node.mesh() else { return Ok(()) };
        let base = mesh
            .name()
            .map(clean_name)
            .filter(|n| !n.is_empty())
            .or_else(|| node.name().map(clean_name).filter(|n| !n.is_empty()))
            .unwrap_or_else(|| self.stem.clone());
        if !bake.iter().flatten().all(|v| v.is_finite()) || det3(bake).abs() < 1e-18 {
            self.skipped.push(format!("mesh `{base}` (its node scales it to nothing)"));
            return Ok(());
        }
        let flip = det3(bake) < 0.0;
        // Primitives by material, in the order they first appear.
        let mut groups: Vec<(Option<usize>, Vec<Vec3>, Vec<[u32; 3]>)> = Vec::new();
        for (k, prim) in mesh.primitives().enumerate() {
            let tris = match self.primitive(&prim) {
                Ok(Some(t)) => t,
                Ok(None) => continue,
                Err(e) => {
                    self.skipped.push(format!("mesh `{base}` part {k}: {e}"));
                    continue;
                }
            };
            let material = prim.material().index();
            let at = match groups.iter().position(|g| g.0 == material) {
                Some(at) => at,
                None => {
                    groups.push((material, Vec::new(), Vec::new()));
                    groups.len() - 1
                }
            };
            let Some(group) = groups.get_mut(at) else { continue };
            let offset = u32::try_from(group.1.len()).map_err(|_| IoError::Invalid("glTF mesh too large".into()))?;
            group.1.extend(tris.0.iter().map(|p| solidcraft_doc::apply_point(bake, *p)));
            group.2.extend(tris.1.iter().map(|t| {
                let t = t.map(|i| i + offset);
                if flip { [t[0], t[2], t[1]] } else { t }
            }));
        }
        let several = groups.len() > 1;
        for (k, (material, positions, triangles)) in groups.into_iter().enumerate() {
            if triangles.is_empty() {
                continue;
            }
            let look = material.and_then(|m| self.doc.materials().nth(m)).map(|m| look_of(&m));
            let name = if several {
                let suffix = look.as_ref().map(|l| l.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("{}", k + 1));
                format!("{base} ({suffix})")
            } else {
                base.clone()
            };
            if let Err(e) = solidcraft_kernel::mesh_body(&positions, &triangles) {
                self.skipped.push(format!("mesh `{name}`: {e}"));
                continue;
            }
            self.meshes.push(MeshData {
                name,
                positions: positions.iter().flat_map(|p| [p.x, p.y, p.z]).collect(),
                triangles: triangles.iter().flatten().copied().collect(),
                color: look.as_ref().map(Look::rgb),
            });
            self.looks.push(look);
            self.plan.bodies.push(slot);
        }
        Ok(())
    }

    /// A primitive's vertices (file units) and triangles; `None` for points and lines.
    #[allow(clippy::type_complexity)]
    fn primitive(&mut self, prim: &gltf::Primitive<'_>) -> std::result::Result<Option<(Vec<Vec3>, Vec<[u32; 3]>)>, String> {
        let mode = prim.mode();
        if !matches!(mode, Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan) {
            return Ok(None);
        }
        let acc = prim.get(&gltf::Semantic::Positions).ok_or("no positions")?;
        let raw = self.accessor(&acc, Dimensions::Vec3)?;
        let positions: Vec<Vec3> = raw.as_chunks::<3>().0.iter().map(|c| Vec3::new(c[0], c[1], c[2])).collect();
        let n = positions.len();
        let indices: Vec<u32> = match prim.indices() {
            Some(a) => {
                if !matches!(a.data_type(), DataType::U8 | DataType::U16 | DataType::U32) {
                    return Err("indices that aren't unsigned integers".into());
                }
                self.accessor(&a, Dimensions::Scalar)?.into_iter().map(|v| v as u32).collect()
            }
            None => (0..u32::try_from(n).map_err(|_| "too many vertices")?).collect(),
        };
        if let Some(bad) = indices.iter().find(|i| **i as usize >= n) {
            return Err(format!("index {bad} past its {n} vertices"));
        }
        let at = |k: usize| indices.get(k).copied().unwrap_or(0);
        let mut tris = Vec::new();
        match mode {
            Mode::TriangleStrip => {
                for k in 0..indices.len().saturating_sub(2) {
                    tris.push(if k % 2 == 0 { [at(k), at(k + 1), at(k + 2)] } else { [at(k + 1), at(k), at(k + 2)] });
                }
            }
            Mode::TriangleFan => {
                for k in 1..indices.len().saturating_sub(1) {
                    tris.push([at(0), at(k), at(k + 1)]);
                }
            }
            _ => tris.extend(indices.as_chunks::<3>().0.iter().copied()),
        }
        // Degenerate triangles (a repeated corner) close nothing.
        tris.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
        if tris.is_empty() {
            return Ok(None);
        }
        // Only the vertices used.
        let mut map: HashMap<u32, u32> = HashMap::new();
        let mut used = Vec::new();
        for t in &mut tris {
            for i in t.iter_mut() {
                let next = used.len() as u32;
                *i = *map.entry(*i).or_insert_with(|| {
                    used.push(positions.get(*i as usize).copied().unwrap_or(Vec3::ZERO));
                    next
                });
            }
        }
        self.vertices = self.vertices.saturating_add(used.len());
        self.triangles = self.triangles.saturating_add(tris.len());
        if self.vertices > MAX_VERTICES || self.triangles > MAX_TRIANGLES {
            return Err(format!("more than {MAX_TRIANGLES} triangles or {MAX_VERTICES} vertices in all"));
        }
        Ok(Some((used, tris)))
    }

    /// An accessor's values (`dims` per element) as numbers; normalized integers as fractions.
    fn accessor(&mut self, acc: &gltf::Accessor<'_>, dims: Dimensions) -> std::result::Result<Vec<f64>, String> {
        if acc.sparse().is_some() {
            return Err("sparse accessors aren't supported".into());
        }
        if acc.dimensions() != dims {
            return Err(format!("accessor {} has {:?} values, not {dims:?}", acc.index(), acc.dimensions()));
        }
        let view = acc.view().ok_or("an accessor without data")?;
        let width = dims.multiplicity();
        let size = acc.data_type().size();
        let elem = width * size;
        let stride = view.stride().unwrap_or(elem);
        let count = acc.count();
        if stride < elem || count > MAX_VERTICES.max(3 * MAX_TRIANGLES) {
            return Err(format!("accessor {} is malformed", acc.index()));
        }
        // Every element must lie in the view, and the view in its buffer.
        let span = if count == 0 { 0 } else { (count - 1).checked_mul(stride).and_then(|s| s.checked_add(elem)).ok_or("accessor too large")? };
        let end = acc.offset().checked_add(span).ok_or("accessor too large")?;
        if end > view.length() {
            return Err(format!("accessor {} runs past its buffer view", acc.index()));
        }
        let view_end = view.offset().checked_add(view.length()).ok_or("buffer view too large")?;
        let data = self.buffers.get(&view.buffer())?;
        if view_end > data.len() || view_end > view.buffer().length() {
            return Err(format!("buffer view {} runs past its buffer (the file may be truncated)", view.index()));
        }
        let base = view.offset() + acc.offset();
        let normalized = acc.normalized();
        let ty = acc.data_type();
        let mut out = Vec::with_capacity(count * width);
        for e in 0..count {
            for c in 0..width {
                let at = base + e * stride + c * size;
                let b = data.get(at..at + size).ok_or("accessor out of range")?;
                let v = match ty {
                    DataType::F32 => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    DataType::U32 => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    DataType::U16 => {
                        let v = f64::from(u16::from_le_bytes([b[0], b[1]]));
                        if normalized { v / 65535.0 } else { v }
                    }
                    DataType::I16 => {
                        let v = f64::from(i16::from_le_bytes([b[0], b[1]]));
                        if normalized { (v / 32767.0).max(-1.0) } else { v }
                    }
                    DataType::U8 => {
                        let v = f64::from(b[0]);
                        if normalized { v / 255.0 } else { v }
                    }
                    DataType::I8 => {
                        let v = f64::from(b[0] as i8);
                        if normalized { (v / 127.0).max(-1.0) } else { v }
                    }
                };
                if !v.is_finite() {
                    return Err(format!("accessor {} holds a number that isn't finite", acc.index()));
                }
                out.push(v);
            }
        }
        Ok(out)
    }
}

/// Linear to sRGB (0–1).
fn srgb(c: f32) -> f32 {
    let c = if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.8 };
    if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// A material's look: base colour, alpha when blended, metallic/roughness when opaque.
fn look_of(m: &gltf::Material<'_>) -> Look {
    let pbr = m.pbr_metallic_roughness();
    let [r, g, b, a] = pbr.base_color_factor();
    let color = [r, g, b].map(|c| (srgb(c) * 255.0).round() as u8);
    let opacity = match m.alpha_mode() {
        gltf::material::AlphaMode::Blend if a.is_finite() => f64::from(a).clamp(0.0, 1.0),
        _ => 1.0,
    };
    let name = m.name().map(clean_name).filter(|n| !n.is_empty()).unwrap_or_else(|| "glTF material".into());
    let finite = |v: f32, d: f64| if v.is_finite() { f64::from(v).clamp(0.0, 1.0) } else { d };
    let pbr = (opacity >= 1.0)
        .then(|| {
            solidcraft_doc::pbr_appearance::PbrParameters::new(finite(pbr.metallic_factor(), 1.0), finite(pbr.roughness_factor(), 1.0), 1.5).ok()
        })
        .flatten();
    Look { name, color, opacity, pbr }
}

/// A GLB container of `json` and an optional binary chunk (each padded to four bytes).
pub fn glb_container(json: &[u8], bin: &[u8]) -> Vec<u8> {
    let pad = |n: usize| (4 - n % 4) % 4;
    let json_len = json.len() + pad(json.len());
    let bin_len = bin.len() + pad(bin.len());
    let total = 12 + 8 + json_len + if bin.is_empty() { 0 } else { 8 + bin_len };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_len as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(json);
    out.extend(std::iter::repeat_n(b' ', pad(json.len())));
    if !bin.is_empty() {
        out.extend_from_slice(&(bin_len as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(bin);
        out.extend(std::iter::repeat_n(0u8, pad(bin.len())));
    }
    out
}

#[cfg(test)]
#[path = "gltf_in_tests.rs"]
pub(crate) mod tests;
