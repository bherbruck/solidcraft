//! 3MF (3D Manufacturing Format, Core specification 1.3) packages: a zip with
//! `[Content_Types].xml`, `_rels/.rels` and the model part `3D/3dmodel.model`.
//!
//! Writing: one mesh object per body (shared vertices, outward winding), named after the body,
//! one build item each, colours as base materials. Objects with face tags also carry, per
//! triangle, the B-rep face it lies on (`ft:face`, SolidCraft's face-tag namespace; spec:
//! docs/formats/3mf-face-tags.md) and are written at full `f64`
//! precision so the receiver gets the tessellation exactly. Reading: every build item becomes one mesh
//! (components flattened with their transforms), converted to millimetres. Packages are
//! untrusted: sizes, entry counts, mesh sizes and component nesting are capped.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Cursor, Read, Write};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use solidcraft_geom::Vec3;

use crate::{IoError, Result};

/// Largest 3MF file we read.
pub const MAX_3MF_BYTES: usize = 512 << 20;
/// Largest uncompressed part we read (zip bombs stop here).
const MAX_PART: u64 = 1 << 30;
const MAX_ENTRIES: usize = 10_000;
/// Most vertices or triangles in one file.
const MAX_ELEMENTS: usize = 20_000_000;
const MAX_COMPONENT_DEPTH: usize = 16;
const MAX_XML_DEPTH: usize = 256;

const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const MODEL_REL: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
/// SolidCraft's face-tag namespace, version 1: `ft:face` on triangles, `ft:faces` on objects
/// (docs/formats/3mf-face-tags.md).
pub const FACE_TAGS_NS: &str = "https://github.com/bherbruck/solidcraft/formats/3mf-face-tags/2026/10";

/// A named triangle mesh (millimetres, counter-clockwise seen from outside).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshObject {
    pub name: String,
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub color: Option<[f32; 3]>,
    /// 0 = opaque, 1 = invisible (written as the colour's alpha).
    pub transparency: f32,
    /// Colours of their own for some triangles (faces with appearances): colour 0..1 and
    /// transparency; `tri_looks` gives each triangle's entry + 1 (0: the object's own), and is
    /// empty when no triangle has one.
    pub looks: Vec<([f32; 3], f32)>,
    pub tri_looks: Vec<u32>,
    /// The B-rep face each triangle lies on, or empty for no face tags. A tagged object is
    /// written with exact (`f64`) coordinates.
    pub tri_faces: Vec<u32>,
    /// Number of B-rep faces (written as `ft:faces` when the object is tagged).
    pub faces: u32,
}

impl MeshObject {
    fn tagged(&self) -> bool {
        !self.tri_faces.is_empty()
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => o.push(c),
        }
    }
    o
}

/// A triangle as written: its vertices, its colour entry (0: the object's own) and its face.
struct Tri {
    v: [u32; 3],
    look: u32,
    face: u32,
}

/// Shared vertices, merged at the precision they are written in (single, or double for tagged
/// objects), and non-degenerate triangles.
fn indexed(m: &MeshObject) -> (Vec<[f64; 3]>, Vec<Tri>) {
    let exact = m.tagged();
    let mut map: HashMap<[u64; 3], u32> = HashMap::new();
    let mut verts: Vec<[f64; 3]> = Vec::new();
    let mut remap = Vec::with_capacity(m.positions.len());
    for p in &m.positions {
        // `+ 0.0` turns -0 into 0, so both merge.
        let v = if exact { [p.x + 0.0, p.y + 0.0, p.z + 0.0] } else { [p.x as f32 as f64 + 0.0, p.y as f32 as f64 + 0.0, p.z as f32 as f64 + 0.0] };
        let key = v.map(f64::to_bits);
        let next = u32::try_from(verts.len()).unwrap_or(u32::MAX);
        let i = *map.entry(key).or_insert_with(|| {
            verts.push(v);
            next
        });
        remap.push(i);
    }
    let tris = m
        .triangles
        .iter()
        .enumerate()
        .filter_map(|(k, t)| {
            let (a, b, c) = (*remap.get(t[0] as usize)?, *remap.get(t[1] as usize)?, *remap.get(t[2] as usize)?);
            (a != b && b != c && c != a).then(|| Tri {
                v: [a, b, c],
                look: m.tri_looks.get(k).copied().unwrap_or(0),
                face: m.tri_faces.get(k).copied().unwrap_or(0),
            })
        })
        .collect();
    (verts, tris)
}

/// Make a tessellation watertight: merge vertices closer than `eps` and split triangles at
/// vertices lying on their open sides (T-junctions between separately meshed faces).
pub fn weld(positions: &[Vec3], triangles: &[[u32; 3]], eps: f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (v, t, _) = weld_tagged(positions, triangles, &[], eps);
    (v, t)
}

/// [`weld`] keeping a tag per triangle (pieces of a split triangle keep its tag; missing tags
/// are 0).
pub fn weld_tagged(positions: &[Vec3], triangles: &[[u32; 3]], tags: &[u32], eps: f64) -> (Vec<Vec3>, Vec<[u32; 3]>, Vec<u32>) {
    // Merge on a grid of cell `eps` (a vertex joins one already in its own or a neighbour cell).
    let cell = |p: Vec3| [(p.x / eps).round() as i64, (p.y / eps).round() as i64, (p.z / eps).round() as i64];
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut verts: Vec<Vec3> = Vec::new();
    let mut remap = Vec::with_capacity(positions.len());
    for p in positions {
        let c = cell(*p);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &i in grid.get(&[c[0].saturating_add(dx), c[1].saturating_add(dy), c[2].saturating_add(dz)]).into_iter().flatten() {
                        if verts.get(i as usize).is_some_and(|q| q.dist(*p) <= eps) {
                            found = Some(i);
                            break 'search;
                        }
                    }
                }
            }
        }
        let i = match found {
            Some(i) => i,
            None => {
                let i = u32::try_from(verts.len()).unwrap_or(u32::MAX);
                verts.push(*p);
                grid.entry(c).or_default().push(i);
                i
            }
        };
        remap.push(i);
    }
    let (mut tris, mut tags): (Vec<[u32; 3]>, Vec<u32>) = triangles
        .iter()
        .enumerate()
        .filter_map(|(k, t)| {
            let (a, b, c) = (*remap.get(t[0] as usize)?, *remap.get(t[1] as usize)?, *remap.get(t[2] as usize)?);
            (a != b && b != c && c != a).then_some(([a, b, c], tags.get(k).copied().unwrap_or(0)))
        })
        .unzip();
    // T-junctions: split open sides at open-side vertices lying on them.
    for _ in 0..8 {
        let mut half: HashMap<(u32, u32), usize> = HashMap::new();
        for t in &tris {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *half.entry((a, b)).or_default() += 1;
            }
        }
        let open: Vec<(u32, u32)> = half.keys().filter(|(a, b)| !half.contains_key(&(*b, *a))).copied().collect();
        if open.is_empty() || open.len() > 200_000 {
            break;
        }
        let mut cand: Vec<u32> = open.iter().flat_map(|(a, b)| [*a, *b]).collect();
        cand.sort_unstable();
        cand.dedup();
        // Vertices on each open side, ordered from its start.
        let mut splits: HashMap<(u32, u32), Vec<(f64, u32)>> = HashMap::new();
        for &(a, b) in &open {
            let (Some(pa), Some(pb)) = (verts.get(a as usize), verts.get(b as usize)) else { continue };
            let d = *pb - *pa;
            let l2 = d.dot(d);
            if !(l2 > 0.0) {
                continue;
            }
            let mut on: Vec<(f64, u32)> = cand
                .iter()
                .filter(|v| **v != a && **v != b)
                .filter_map(|v| {
                    let p = *verts.get(*v as usize)?;
                    let t = (p - *pa).dot(d) / l2;
                    (t > 1e-9 && t < 1.0 - 1e-9 && (*pa + d * t).dist(p) <= eps * 4.0).then_some((t, *v))
                })
                .collect();
            if !on.is_empty() {
                on.sort_by(|x, y| x.0.total_cmp(&y.0));
                splits.insert((a, b), on);
            }
        }
        if splits.is_empty() {
            break;
        }
        let mut next = Vec::with_capacity(tris.len() + splits.len() * 2);
        let mut next_tags = Vec::with_capacity(next.capacity());
        for (t, tag) in tris.iter().zip(&tags) {
            // Rotate so the split side (if any) is (t0, t1); split one side per pass.
            let rot = [[t[0], t[1], t[2]], [t[1], t[2], t[0]], [t[2], t[0], t[1]]];
            match rot.iter().find_map(|r| splits.get(&(r[0], r[1])).map(|s| (r, s))) {
                Some((r, s)) => {
                    let mut prev = r[0];
                    for (_, v) in s {
                        next.push([prev, *v, r[2]]);
                        next_tags.push(*tag);
                        prev = *v;
                    }
                    next.push([prev, r[1], r[2]]);
                    next_tags.push(*tag);
                }
                None => {
                    next.push(*t);
                    next_tags.push(*tag);
                }
            }
        }
        tris = next;
        tags = next_tags;
    }
    (verts, tris, tags)
}

/// The model part's XML.
pub fn model_xml(objects: &[MeshObject]) -> String {
    model_xml_with(objects, &[])
}

/// The model part's XML with model metadata after `Application`: (3MF metadata name, such as
/// `Title` or `Designer`, and its text).
pub fn model_xml_with(objects: &[MeshObject], metadata: &[(&str, &str)]) -> String {
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let ft = if objects.iter().any(MeshObject::tagged) { format!(" xmlns:ft=\"{FACE_TAGS_NS}\"") } else { String::new() };
    let _ = writeln!(x, "<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE_NS}\"{ft}>");
    x.push_str(" <metadata name=\"Application\">SolidCraft</metadata>\n");
    for (name, text) in metadata {
        let _ = writeln!(x, " <metadata name=\"{}\">{}</metadata>", esc(name), esc(text));
    }
    x.push_str(" <resources>\n");
    let colored = objects.iter().any(|o| o.color.is_some() || o.transparency > 0.0 || !o.looks.is_empty());
    // Base material group id 1 (each object's own colour, then its faces' colours); objects
    // from 2. A see-through colour carries its alpha (#RRGGBBAA).
    let display = |c: [f32; 3], transparency: f32| {
        let c = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        let alpha = if transparency > 0.0 { format!("{:02X}", ((1.0 - transparency.clamp(0.0, 1.0)) * 255.0).round() as u8) } else { String::new() };
        format!("#{:02X}{:02X}{:02X}{alpha}", c[0], c[1], c[2])
    };
    let mut first = Vec::with_capacity(objects.len());
    if colored {
        x.push_str("  <basematerials id=\"1\">\n");
        let mut n = 0usize;
        for o in objects {
            first.push(n);
            let _ =
                writeln!(x, "   <base name=\"{}\" displaycolor=\"{}\"/>", esc(&o.name), display(o.color.unwrap_or([0.7, 0.7, 0.7]), o.transparency));
            n += 1;
            for (i, (c, t)) in o.looks.iter().enumerate() {
                let _ = writeln!(x, "   <base name=\"{} face {}\" displaycolor=\"{}\"/>", esc(&o.name), i + 1, display(*c, *t));
                n += 1;
            }
        }
        x.push_str("  </basematerials>\n");
    }
    for (k, o) in objects.iter().enumerate() {
        let id = k + 2;
        let own = first.get(k).copied().unwrap_or(0);
        let mat = if colored { format!(" pid=\"1\" pindex=\"{own}\"") } else { String::new() };
        let faces = if o.tagged() { format!(" ft:faces=\"{}\"", o.faces.max(1)) } else { String::new() };
        let _ = writeln!(x, "  <object id=\"{id}\" type=\"model\" name=\"{}\"{mat}{faces}>", esc(&o.name));
        x.push_str("   <mesh>\n    <vertices>\n");
        let (verts, tris) = indexed(o);
        for v in &verts {
            // Shortest round-trip form: f64 comes back exactly, f32 as written before.
            if o.tagged() {
                let _ = writeln!(x, "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", v[0], v[1], v[2]);
            } else {
                let _ = writeln!(x, "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", v[0] as f32, v[1] as f32, v[2] as f32);
            }
        }
        x.push_str("    </vertices>\n    <triangles>\n");
        for t in &tris {
            let [a, b, c] = t.v;
            let _ = write!(x, "     <triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"");
            if colored && t.look > 0 && (t.look as usize) <= o.looks.len() {
                let _ = write!(x, " pid=\"1\" p1=\"{}\"", own + t.look as usize);
            }
            if o.tagged() {
                let _ = write!(x, " ft:face=\"{}\"", t.face);
            }
            x.push_str("/>\n");
        }
        x.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    x.push_str(" </resources>\n <build>\n");
    for k in 0..objects.len() {
        let _ = writeln!(x, "  <item objectid=\"{}\"/>", k + 2);
    }
    x.push_str(" </build>\n</model>\n");
    x
}

/// A 3MF package of meshes.
pub fn write_3mf(objects: &[MeshObject]) -> Result<Vec<u8>> {
    write_3mf_with(objects, &[])
}

/// A 3MF package of meshes with model metadata (see [`model_xml_with`]).
pub fn write_3mf_with(objects: &[MeshObject], metadata: &[(&str, &str)]) -> Result<Vec<u8>> {
    if objects.is_empty() {
        return Err(IoError::Empty);
    }
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n</Types>\n";
    let rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"{MODEL_REL}\"/>\n</Relationships>\n"
    );
    let model = model_xml_with(objects, metadata);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in [("[Content_Types].xml", content_types.as_bytes()), ("_rels/.rels", rels.as_bytes()), ("3D/3dmodel.model", model.as_bytes())]
    {
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default())
            .large_file(data.len() as u64 >= u32::MAX as u64);
        zip.start_file(name, opts).map_err(|e| IoError::Invalid(format!("3MF: {e}")))?;
        zip.write_all(data).map_err(|e| IoError::Invalid(format!("3MF: {e}")))?;
    }
    zip.finish().map(|c| c.into_inner()).map_err(|e| IoError::Invalid(format!("3MF: {e}")))
}

// ---------------------------------------------------------------- reading

fn bad<T>(msg: impl Into<String>) -> Result<T> {
    Err(IoError::Invalid(format!("3MF: {}", msg.into())))
}

fn read_part(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Option<Vec<u8>>> {
    let want = name.trim_start_matches('/').to_ascii_lowercase();
    let Some(idx) = (0..zip.len()).find(|i| {
        zip.name_for_index(*i).and_then(|n| n.ok()).is_some_and(|n| n.replace('\\', "/").trim_start_matches('/').to_ascii_lowercase() == want)
    }) else {
        return Ok(None);
    };
    let mut f = zip.by_index(idx).map_err(|e| IoError::Invalid(format!("3MF {name}: {e}")))?;
    if f.size() > MAX_PART {
        return bad(format!("{name} expands to {} bytes", f.size()));
    }
    let mut out = Vec::with_capacity(f.size().min(16 << 20) as usize);
    (&mut f).take(MAX_PART + 1).read_to_end(&mut out).map_err(|e| IoError::Invalid(format!("3MF {name}: {e}")))?;
    if out.len() as u64 > MAX_PART {
        return bad(format!("{name} expands to more than 1 GB"));
    }
    Ok(Some(out))
}

fn attrs(e: &BytesStart) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for a in e.attributes().flatten() {
        let k = String::from_utf8_lossy(a.key.local_name().as_ref()).to_string();
        let v = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map(|v| v.to_string())
            .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).to_string());
        m.insert(k, v);
    }
    m
}

/// Row-vector affine transform `[m00 m01 m02 m10 … m32]` (3MF order).
type Xf = [f64; 12];
const IDENTITY: Xf = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

fn parse_xf(s: Option<&String>) -> Result<Xf> {
    let Some(s) = s else { return Ok(IDENTITY) };
    let v: Vec<f64> = s
        .split_whitespace()
        .map(|t| t.parse::<f64>().ok().filter(|x| x.is_finite() && x.abs() < 1e12))
        .collect::<Option<_>>()
        .ok_or_else(|| IoError::Invalid(format!("3MF: bad transform `{s}`")))?;
    let mut m = IDENTITY;
    if v.len() != 12 {
        return bad(format!("transform with {} numbers", v.len()));
    }
    m.copy_from_slice(&v);
    Ok(m)
}

fn apply(m: &Xf, p: Vec3) -> Vec3 {
    Vec3::new(p.x * m[0] + p.y * m[3] + p.z * m[6] + m[9], p.x * m[1] + p.y * m[4] + p.z * m[7] + m[10], p.x * m[2] + p.y * m[5] + p.z * m[8] + m[11])
}

/// `a` then `b`.
fn compose(a: &Xf, b: &Xf) -> Xf {
    let r = |i: usize| Vec3::new(a[i * 3], a[i * 3 + 1], a[i * 3 + 2]);
    let lin = |v: Vec3| Vec3::new(v.x * b[0] + v.y * b[3] + v.z * b[6], v.x * b[1] + v.y * b[4] + v.z * b[7], v.x * b[2] + v.y * b[5] + v.z * b[8]);
    let (x, y, z, t) = (lin(r(0)), lin(r(1)), lin(r(2)), apply(b, r(3)));
    [x.x, x.y, x.z, y.x, y.y, y.z, z.x, z.y, z.z, t.x, t.y, t.z]
}

fn det(m: &Xf) -> f64 {
    m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6])
}

#[derive(Default)]
struct Object {
    name: String,
    positions: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    components: Vec<(u32, Xf)>,
    material: Option<(u32, usize)>,
}

#[derive(Default)]
struct Model {
    unit: f64,
    objects: HashMap<u32, Object>,
    materials: HashMap<u32, Vec<Option<[f32; 3]>>>,
    build: Vec<(u32, Xf)>,
    warnings: Vec<String>,
}

fn color(s: &str) -> Option<[f32; 3]> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 && h.len() != 8 {
        return None;
    }
    let c = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).map(|v| v as f32 / 255.0);
    Some([c(0)?, c(2)?, c(4)?])
}

fn unit_mm(u: &str) -> Option<f64> {
    Some(match u {
        "micron" => 1e-3,
        "millimeter" => 1.0,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => return None,
    })
}

fn parse_model(xml: &[u8]) -> Result<Model> {
    let mut rd = Reader::from_reader(xml);
    rd.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut m = Model { unit: 1.0, ..Default::default() };
    let mut depth = 0usize;
    let mut cur: Option<(u32, Object)> = None;
    let mut cur_mat: Option<u32> = None;
    let mut elements = 0usize;
    let mut seen_model = false;
    loop {
        let ev = rd.read_event_into(&mut buf).map_err(|e| IoError::Invalid(format!("3MF model XML: {e}")))?;
        let (e, empty) = match &ev {
            Event::Start(e) => (Some(e.clone()), false),
            Event::Empty(e) => (Some(e.clone()), true),
            Event::End(end) => {
                depth = depth.saturating_sub(1);
                match end.local_name().as_ref() {
                    b"object" => {
                        if let Some((id, o)) = cur.take() {
                            m.objects.insert(id, o);
                        }
                    }
                    b"basematerials" => cur_mat = None,
                    _ => {}
                }
                (None, false)
            }
            Event::Eof => break,
            _ => (None, false),
        };
        let Some(e) = e else {
            buf.clear();
            continue;
        };
        if !empty {
            depth += 1;
            if depth > MAX_XML_DEPTH {
                return bad("XML nested too deeply");
            }
        }
        elements += 1;
        if elements > MAX_ELEMENTS * 2 {
            return bad("model too large");
        }
        let a = attrs(&e);
        let num = |k: &str| a.get(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
        let id = |k: &str| a.get(k).and_then(|v| v.trim().parse::<u32>().ok());
        match e.local_name().as_ref() {
            b"model" => {
                seen_model = true;
                if let Some(u) = a.get("unit") {
                    m.unit = unit_mm(u).ok_or_else(|| IoError::Invalid(format!("3MF: unknown unit `{u}`")))?;
                }
            }
            b"basematerials" => {
                let gid = id("id").ok_or_else(|| IoError::Invalid("3MF: base materials without id".into()))?;
                m.materials.insert(gid, Vec::new());
                cur_mat = if empty { None } else { Some(gid) };
            }
            b"base" => {
                if let Some(g) = cur_mat.and_then(|g| m.materials.get_mut(&g)) {
                    g.push(a.get("displaycolor").and_then(|c| color(c)));
                }
            }
            b"object" => {
                let oid = id("id").ok_or_else(|| IoError::Invalid("3MF: object without id".into()))?;
                let o = Object {
                    name: a.get("name").cloned().unwrap_or_default(),
                    material: id("pid").map(|p| (p, a.get("pindex").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0))),
                    ..Default::default()
                };
                if empty {
                    m.objects.insert(oid, o);
                } else {
                    cur = Some((oid, o));
                }
            }
            b"vertex" => {
                let Some((_, o)) = cur.as_mut() else { return bad("vertex outside an object") };
                if o.positions.len() >= MAX_ELEMENTS {
                    return bad("too many vertices");
                }
                let (Some(x), Some(y), Some(z)) = (num("x"), num("y"), num("z")) else { return bad("vertex without x, y, z") };
                o.positions.push(Vec3::new(x, y, z));
            }
            b"triangle" => {
                let Some((_, o)) = cur.as_mut() else { return bad("triangle outside an object") };
                if o.triangles.len() >= MAX_ELEMENTS {
                    return bad("too many triangles");
                }
                let (Some(a1), Some(a2), Some(a3)) = (id("v1"), id("v2"), id("v3")) else { return bad("triangle without v1, v2, v3") };
                o.triangles.push([a1, a2, a3]);
            }
            b"component" => {
                let Some((_, o)) = cur.as_mut() else { return bad("component outside an object") };
                if a.keys().any(|k| k == "path") {
                    m.warnings.push("components in other model parts (production extension) are not read".into());
                    buf.clear();
                    continue;
                }
                let target = id("objectid").ok_or_else(|| IoError::Invalid("3MF: component without objectid".into()))?;
                o.components.push((target, parse_xf(a.get("transform"))?));
            }
            b"item" => {
                if a.keys().any(|k| k == "path") {
                    m.warnings.push("build items in other model parts (production extension) are not read".into());
                    buf.clear();
                    continue;
                }
                let target = id("objectid").ok_or_else(|| IoError::Invalid("3MF: build item without objectid".into()))?;
                m.build.push((target, parse_xf(a.get("transform"))?));
            }
            _ => {}
        }
        buf.clear();
    }
    if !seen_model {
        return bad("no <model> element");
    }
    Ok(m)
}

/// Append object `id` (and its components) placed by `xf` to `out`.
fn flatten(m: &Model, id: u32, xf: &Xf, out: &mut MeshObject, stack: &mut Vec<u32>, total: &mut usize) -> Result<()> {
    if stack.len() > MAX_COMPONENT_DEPTH || stack.contains(&id) {
        return bad("components nested too deeply or recursive");
    }
    let o = m.objects.get(&id).ok_or_else(|| IoError::Invalid(format!("3MF: missing object {id}")))?;
    *total = total.saturating_add(o.triangles.len());
    if *total > MAX_ELEMENTS {
        return bad("too many triangles after placing components");
    }
    let n = o.positions.len();
    // One object can be placed by thousands of build items and components, and each placement
    // needs its own transformed copy of the vertices. Copying only the vertices the triangles
    // reach keeps that cost tied to the triangle budget above; copying the whole vertex array
    // let an object with many vertices and one triangle multiply into tens of gigabytes.
    let mut used: Vec<u32> = o.triangles.iter().flatten().copied().collect();
    used.sort_unstable();
    used.dedup();
    if used.last().is_some_and(|v| *v as usize >= n) {
        return bad(format!("object {id}: triangle refers to a missing vertex"));
    }
    let base = u32::try_from(out.positions.len()).map_err(|_| IoError::Invalid("3MF: too many vertices".into()))?;
    if base as usize + used.len() > u32::MAX as usize {
        return bad("too many vertices");
    }
    for v in &used {
        let Some(p) = o.positions.get(*v as usize) else {
            return bad(format!("object {id}: triangle refers to a missing vertex"));
        };
        out.positions.push(apply(xf, *p) * m.unit);
    }
    // Vertices keep their relative order, so a mesh whose triangles use all of them is
    // numbered exactly as it was in the file.
    let at = |v: u32| used.binary_search(&v).ok().map(|i| base + i as u32);
    let flip = det(xf) < 0.0;
    for t in &o.triangles {
        let mut q = [0u32; 3];
        for (slot, v) in q.iter_mut().zip(t) {
            let Some(i) = at(*v) else {
                return bad(format!("object {id}: triangle refers to a missing vertex"));
            };
            *slot = i;
        }
        out.triangles.push(if flip { [q[0], q[2], q[1]] } else { q });
    }
    if out.color.is_none() {
        out.color = o.material.and_then(|(g, i)| m.materials.get(&g).and_then(|g| g.get(i).copied().flatten()));
    }
    stack.push(id);
    for (c, cx) in &o.components {
        flatten(m, *c, &compose(cx, xf), out, stack, total)?;
    }
    stack.pop();
    Ok(())
}

/// Meshes of a 3MF package (one per build item, in millimetres) and warnings.
pub fn read_3mf(bytes: &[u8]) -> Result<(Vec<MeshObject>, Vec<String>)> {
    if bytes.len() > MAX_3MF_BYTES {
        return bad(format!("file too large ({} MB)", bytes.len() >> 20));
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| IoError::Invalid(format!("3MF is not a zip package: {e}")))?;
    if zip.len() > MAX_ENTRIES {
        return bad(format!("{} zip entries", zip.len()));
    }
    // The model part named by the package relationships, else the usual name.
    let mut root = "3D/3dmodel.model".to_string();
    if let Some(rels) = read_part(&mut zip, "_rels/.rels")? {
        let mut rd = Reader::from_reader(rels.as_slice());
        let mut buf = Vec::new();
        loop {
            match rd.read_event_into(&mut buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.local_name().as_ref() == b"Relationship" => {
                    let a = attrs(&e);
                    if a.get("Type").is_some_and(|t| t.ends_with("/3dmodel"))
                        && let Some(t) = a.get("Target")
                    {
                        root = t.trim_start_matches('/').to_string();
                        break;
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
            buf.clear();
        }
    }
    let xml = read_part(&mut zip, &root)?.ok_or_else(|| IoError::Invalid(format!("3MF: no model part `{root}`")))?;
    let m = parse_model(&xml)?;
    let mut out = Vec::new();
    let mut warnings = m.warnings.clone();
    let mut total = 0usize;
    for (k, (id, xf)) in m.build.iter().enumerate() {
        let mut mo = MeshObject::default();
        match flatten(&m, *id, xf, &mut mo, &mut Vec::new(), &mut total) {
            Ok(()) if !mo.triangles.is_empty() => {
                let name = m.objects.get(id).map(|o| o.name.trim().to_string()).filter(|n| !n.is_empty());
                mo.name = name.unwrap_or_else(|| format!("Mesh{}", k + 1));
                out.push(mo);
            }
            Ok(()) => warnings.push(format!("build item {} has no triangles", k + 1)),
            Err(e) => warnings.push(format!("build item {}: {e}", k + 1)),
        }
    }
    if out.is_empty() {
        return bad(warnings.first().cloned().unwrap_or_else(|| "no meshes to build".into()));
    }
    Ok((out, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tetra(name: &str) -> MeshObject {
        MeshObject {
            name: name.into(),
            positions: vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 10.0, 0.0), Vec3::new(0.0, 0.0, 10.0)],
            triangles: vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
            color: None,
            ..Default::default()
        }
    }

    /// A see-through object writes its alpha; triangles of faces with colours of their own
    /// point at their entries.
    #[test]
    fn face_colours_and_alpha() {
        let mut t = tetra("T");
        t.color = Some([0.125, 0.25, 0.75]);
        t.transparency = 0.5;
        t.looks = vec![([1.0, 0.0, 0.0], 0.0)];
        t.tri_looks = vec![0, 1, 0, 0];
        let model = model_xml(&[t]);
        assert!(model.contains("displaycolor=\"#2040BF80\""), "{model}");
        assert!(model.contains("displaycolor=\"#FF0000\""), "{model}");
        assert_eq!(model.matches("p1=\"1\"").count(), 1, "{model}");
    }

    fn package(model: &str, rels: Option<&str>) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        if let Some(r) = rels {
            zip.start_file("_rels/.rels", o).unwrap();
            zip.write_all(r.as_bytes()).unwrap();
        }
        zip.start_file("3D/3dmodel.model", o).unwrap();
        zip.write_all(model.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn package_is_well_formed() {
        let mut t = tetra("A <&> \"b\"");
        t.color = Some([1.0, 0.5, 0.0]);
        let bytes = write_3mf(&[t, tetra("B")]).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        for part in ["[Content_Types].xml", "_rels/.rels", "3D/3dmodel.model"] {
            let data = read_part(&mut zip, part).unwrap().unwrap();
            // Every part parses as XML to the end.
            let mut rd = Reader::from_reader(data.as_slice());
            let mut buf = Vec::new();
            let mut depth = 0i32;
            loop {
                match rd.read_event_into(&mut buf).unwrap() {
                    Event::Start(_) => depth += 1,
                    Event::End(_) => depth -= 1,
                    Event::Eof => break,
                    _ => {}
                }
                buf.clear();
            }
            assert_eq!(depth, 0, "{part}");
        }
        let model = String::from_utf8(read_part(&mut zip, "3D/3dmodel.model").unwrap().unwrap()).unwrap();
        assert!(model.contains("unit=\"millimeter\"") && model.contains(CORE_NS));
        assert!(model.contains("displaycolor=\"#FF8000\"") && model.contains("pid=\"1\" pindex=\"0\""));
        assert!(model.contains("name=\"A &lt;&amp;&gt; &quot;b&quot;\""));
        assert_eq!(model.matches("<item ").count(), 2);
        let (objs, w) = read_3mf(&bytes).unwrap();
        assert!(w.is_empty());
        assert_eq!(objs.len(), 2);
        assert_eq!(objs[0].name, "A <&> \"b\"");
        assert_eq!(objs[0].color, Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(objs[0].triangles, tetra("").triangles);
    }

    #[test]
    fn components_transforms_and_units() {
        let model = r#"<?xml version="1.0"?>
<model unit="centimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources>
<object id="1" type="model" name="Tet"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="1"/></vertices>
<triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="1" v2="2" v3="3"/><triangle v1="0" v2="3" v3="2"/></triangles></mesh></object>
<object id="2" type="model" name="Pair"><components><component objectid="1"/><component objectid="1" transform="1 0 0 0 1 0 0 0 1 5 0 0"/></components></object>
</resources><build><item objectid="2" transform="-1 0 0 0 1 0 0 0 1 0 0 2"/></build></model>"#;
        let (objs, _) = read_3mf(&package(model, None)).unwrap();
        assert_eq!(objs.len(), 1);
        let o = &objs[0];
        assert_eq!(o.name, "Pair");
        assert_eq!(o.triangles.len(), 8);
        // Mirrored by the item, in mm: x in [-60, 0], z in [20, 30]; winding flipped back.
        let xs: Vec<f64> = o.positions.iter().map(|p| p.x).collect();
        assert!(xs.iter().any(|x| (*x + 60.0).abs() < 1e-9) && xs.iter().all(|x| *x <= 1e-9));
        assert!(o.positions.iter().all(|p| p.z >= 20.0 - 1e-9 && p.z <= 30.0 + 1e-9));
        let v6: f64 = o.triangles.iter().map(|t| o.positions[t[0] as usize].dot(o.positions[t[1] as usize].cross(o.positions[t[2] as usize]))).sum();
        assert!((v6 / 6.0 - 2.0 * 1000.0 / 6.0).abs() < 1e-6, "{}", v6 / 6.0);
    }

    #[test]
    fn hostile_packages_are_errors() {
        let head = r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources>"#;
        let cases = [
            format!(
                "{head}<object id=\"1\"><components><component objectid=\"1\"/></components></object></resources><build><item objectid=\"1\"/></build></model>"
            ),
            format!(
                "{head}<object id=\"1\"><mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/></vertices><triangles><triangle v1=\"0\" v2=\"7\" v3=\"9\"/></triangles></mesh></object></resources><build><item objectid=\"1\"/></build></model>"
            ),
            format!("{head}<object id=\"1\"><mesh><vertices><vertex x=\"NaN\" y=\"0\" z=\"0\"/></vertices></mesh></object></resources></model>"),
            format!("{head}</resources><build><item objectid=\"9\"/></build></model>"),
            format!("{head}<object id=\"1\"><components><component objectid=\"1\" transform=\"1 2\"/></components></object></resources></model>"),
            r#"<model unit="parsec"/>"#.to_string(),
            "<model><resources><object".to_string(),
            format!("{}{}", "<a>".repeat(1000), "</a>".repeat(1000)),
            "not xml at all".to_string(),
        ];
        for c in &cases {
            assert!(read_3mf(&package(c, None)).is_err(), "{c}");
        }
        assert!(read_3mf(b"").is_err());
        assert!(read_3mf(b"PK\x03\x04garbage").is_err());
        assert!(
            read_3mf(&package("<model/>", Some("<Relationships><Relationship Type=\"x/3dmodel\" Target=\"/missing.model\"/></Relationships>")))
                .is_err()
        );
    }

    /// Every placement needs its own transformed copy of the vertices, so an object with many
    /// vertices and one triangle used to cost its whole vertex array per build item: 100k
    /// vertices times 10,000 items asked for about 140 GB and the process aborted on the
    /// allocation, which no panic guard can intercept.
    #[test]
    fn many_placements_of_a_big_object_stay_bounded() {
        let t = std::time::Instant::now();
        let verts: String = (0..100_000).map(|i| format!("<vertex x=\"{}\" y=\"{}\" z=\"0\"/>", i % 100, i / 100)).collect();
        let model = format!(
            "{}<object id=\"1\" type=\"model\"><mesh><vertices>{verts}</vertices>\
             <triangles><triangle v1=\"0\" v2=\"1\" v3=\"2\"/></triangles></mesh></object></resources><build>{}</build></model>",
            r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources>"#,
            "<item objectid=\"1\"/>".repeat(10_000)
        );
        let (objs, _) = read_3mf(&package(&model, None)).unwrap();
        assert_eq!(objs.len(), 10_000);
        // Only the three vertices the one triangle reaches are copied per placement.
        assert!(objs.iter().all(|o| o.positions.len() == 3 && o.triangles.len() == 1), "{:?}", objs.first().map(|o| o.positions.len()));
        assert!(t.elapsed() < std::time::Duration::from_secs(30), "took {:?}", t.elapsed());
    }

    /// Triangles are budgeted across every placement, so fan-out cannot outrun the cap.
    #[test]
    fn placements_share_the_triangle_budget() {
        let verts: String = (0..30_000).map(|i| format!("<vertex x=\"{}\" y=\"0\" z=\"0\"/>", i % 100)).collect();
        let tris: String = (0..10_000).map(|i| format!("<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", 3 * i, 3 * i + 1, 3 * i + 2)).collect();
        let model = format!(
            "{}<object id=\"1\" type=\"model\"><mesh><vertices>{verts}</vertices><triangles>{tris}</triangles></mesh></object>\
             </resources><build>{}</build></model>",
            r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources>"#,
            "<item objectid=\"1\"/>".repeat(4_000)
        );
        let (objs, warnings) = read_3mf(&package(&model, None)).unwrap();
        assert!(objs.len() < 4_000, "the shared budget must stop the later items: {} built", objs.len());
        assert!(warnings.iter().any(|w| w.contains("too many triangles")), "{warnings:?}");
    }

    /// A body far from the origin (a pattern spacing of "1e12 mm" gets there) welds without
    /// overflowing the grid's cell indices.
    #[test]
    fn weld_survives_extreme_coordinates() {
        let o = Vec3::new(1e12, 0.0, 0.0);
        let positions = [o, o + Vec3::new(0.1, 0.0, 0.0), o + Vec3::new(0.0, 0.1, 0.0), o + Vec3::new(0.0, 0.0, 0.1)];
        let triangles = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
        let (v, t) = weld(&positions, &triangles, 1e-7);
        assert_eq!((v.len(), t.len()), (4, 4));
    }
}
