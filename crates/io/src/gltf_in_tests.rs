//! glTF import tests. Every file here is built by the tests themselves.

use serde_json::{Value, json};

use super::*;

/// A glTF file under construction: one binary buffer, meshes, nodes and materials.
#[derive(Default)]
pub(crate) struct Fixture {
    pub bin: Vec<u8>,
    pub views: Vec<Value>,
    pub accessors: Vec<Value>,
    pub meshes: Vec<Value>,
    pub nodes: Vec<Value>,
    pub materials: Vec<Value>,
    pub roots: Vec<usize>,
}

/// A cube of side `s` (metres) with its corner at the origin, outward facing.
pub(crate) fn cube(s: f32) -> (Vec<[f32; 3]>, Vec<u16>) {
    let p = (0..8).map(|i| [(i & 1) as f32 * s, ((i >> 1) & 1) as f32 * s, ((i >> 2) & 1) as f32 * s]).collect();
    let t = vec![0, 2, 1, 1, 2, 3, 4, 5, 6, 5, 7, 6, 0, 1, 4, 1, 5, 4, 2, 6, 3, 3, 6, 7, 0, 4, 2, 2, 4, 6, 1, 3, 5, 3, 7, 5];
    (p, t)
}

impl Fixture {
    fn view(&mut self, bytes: &[u8]) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.views.push(json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()}));
        self.bin.extend_from_slice(bytes);
        self.views.len() - 1
    }

    /// A mesh of one primitive per (positions, indices, material).
    pub fn mesh(&mut self, name: &str, prims: &[(&[[f32; 3]], &[u16], Option<usize>)]) -> usize {
        let mut out = Vec::new();
        for (positions, indices, material) in prims {
            let bytes: Vec<u8> = positions.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
            let v = self.view(&bytes);
            let lo = (0..3).map(|k| positions.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min)).collect::<Vec<_>>();
            let hi = (0..3).map(|k| positions.iter().map(|p| p[k]).fold(f32::NEG_INFINITY, f32::max)).collect::<Vec<_>>();
            self.accessors.push(json!({"bufferView": v, "componentType": 5126, "count": positions.len(), "type": "VEC3", "min": lo, "max": hi}));
            let pa = self.accessors.len() - 1;
            let bytes: Vec<u8> = indices.iter().flat_map(|v| v.to_le_bytes()).collect();
            let v = self.view(&bytes);
            self.accessors.push(json!({"bufferView": v, "componentType": 5123, "count": indices.len(), "type": "SCALAR"}));
            let mut p = json!({"attributes": {"POSITION": pa}, "indices": self.accessors.len() - 1});
            if let Some(m) = material {
                p["material"] = json!(m);
            }
            out.push(p);
        }
        self.meshes.push(json!({"name": name, "primitives": out}));
        self.meshes.len() - 1
    }

    pub fn cube_mesh(&mut self, name: &str, s: f32) -> usize {
        let (p, t) = cube(s);
        self.mesh(name, &[(&p, &t, None)])
    }

    pub fn node(&mut self, node: Value) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    pub fn json(&self, uri: Option<&str>) -> Value {
        let mut buffer = json!({"byteLength": self.bin.len()});
        if let Some(u) = uri {
            buffer["uri"] = json!(u);
        }
        let mut j = json!({
            "asset": {"version": "2.0", "generator": "solidcraft tests"},
            "scene": 0,
            "scenes": [{"nodes": self.roots}],
            "nodes": self.nodes,
            "meshes": self.meshes,
            "accessors": self.accessors,
            "bufferViews": self.views,
            "buffers": [buffer],
        });
        if !self.materials.is_empty() {
            j["materials"] = json!(self.materials);
        }
        j
    }

    pub fn glb(&self) -> Vec<u8> {
        glb_container(self.json(None).to_string().as_bytes(), &self.bin)
    }
}

fn no_files(name: &str) -> std::result::Result<Vec<u8>, String> {
    Err(format!("no file `{name}`"))
}

fn read(bytes: &[u8]) -> Result<GltfImport> {
    read_gltf(bytes, "model.glb", &mut no_files)
}

fn meshes(g: &GltfImport) -> &[MeshData] {
    match &g.feature.kind {
        solidcraft_doc::FeatureKind::MeshImport { meshes, .. } => meshes,
        _ => &[],
    }
}

fn bounds(m: &MeshData) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in m.positions.as_chunks::<3>().0 {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// Signed volume (positive for outward-facing triangles).
fn volume(m: &MeshData) -> f64 {
    let p = |i: u32| {
        let i = i as usize * 3;
        Vec3::new(m.positions[i], m.positions[i + 1], m.positions[i + 2])
    };
    m.triangles.as_chunks::<3>().0.iter().map(|t| p(t[0]).dot(p(t[1]).cross(p(t[2]))) / 6.0).sum()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6 * (1.0 + b.abs())
}

#[test]
fn single_mesh_in_millimetres() {
    let mut f = Fixture::default();
    let m = f.cube_mesh("Cube", 0.01);
    let n = f.node(json!({"name": "Cube node", "mesh": m}));
    f.roots = vec![n];
    let g = read(&f.glb()).unwrap();
    assert!(g.feature.warnings.is_empty(), "{:?}", g.feature.warnings);
    assert_eq!(g.feature.name, "Cube");
    let ms = meshes(&g);
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0].triangles.len(), 36);
    let (lo, hi) = bounds(&ms[0]);
    assert!(lo.iter().all(|v| close(*v, 0.0)) && hi.iter().all(|v| close(*v, 10.0)), "{lo:?} {hi:?}");
    assert!(close(volume(&ms[0]), 1000.0), "{}", volume(&ms[0]));
    // One root node is the import's own component.
    assert!(g.plan.slots.is_empty() && g.plan.places.is_empty());
    assert_eq!(g.plan.bodies, vec![None]);
    assert_eq!(g.looks, vec![None]);
    // The axes are the file's: Y up stays Y here (the caller turns the component).
    let mut f = Fixture::default();
    let (p, t) = cube(0.01);
    let tall: Vec<[f32; 3]> = p.iter().map(|q| [q[0], q[1] * 3.0, q[2]]).collect();
    let m = f.mesh("Tall", &[(&tall, &t, None)]);
    let n = f.node(json!({"mesh": m}));
    f.roots = vec![n];
    let g = read(&f.glb()).unwrap();
    let (_, hi) = bounds(&meshes(&g)[0]);
    assert!(close(hi[1], 30.0) && close(hi[2], 10.0), "{hi:?}");
}

#[test]
fn node_hierarchy_becomes_components() {
    let mut f = Fixture::default();
    let m = f.cube_mesh("Block", 0.01);
    let arm = f.node(json!({"name": "Arm", "mesh": m, "translation": [0.0, 0.05, 0.0], "rotation": [0.0, 0.0, 0.70710677, 0.70710677]}));
    let base = f.node(json!({"name": "Base", "mesh": m, "translation": [0.1, 0.0, 0.0], "children": [arm]}));
    let block = f.cube_mesh("Other block", 0.01);
    let other = f.node(json!({"name": "Other", "mesh": block, "translation": [0.0, 0.0, 0.2]}));
    f.roots = vec![base, other];
    let g = read(&f.glb()).unwrap();
    assert_eq!(g.plan.slots, vec!["Base", "Arm", "Other"]);
    assert_eq!(g.plan.places.len(), 3);
    let base_place = &g.plan.places[0];
    assert_eq!((base_place.slot, base_place.parent), (0, None));
    assert!(close(base_place.transform[3][0], 100.0));
    let arm_place = &g.plan.places[1];
    assert_eq!((arm_place.slot, arm_place.parent), (1, Some(0)));
    assert!(close(arm_place.transform[3][1], 50.0));
    // A quarter turn about Z: X goes to Y.
    assert!(close(arm_place.transform[0][1], 1.0) && arm_place.transform[0][0].abs() < 1e-6, "{:?}", arm_place.transform);
    assert_eq!(g.plan.bodies, vec![Some(0), Some(1), Some(2)]);
    // Bodies are in their component's frame.
    for m in meshes(&g) {
        let (lo, hi) = bounds(m);
        assert!(lo.iter().all(|v| close(*v, 0.0)) && hi.iter().all(|v| close(*v, 10.0)), "{} {lo:?} {hi:?}", m.name);
    }
}

#[test]
fn repeated_meshes_are_instances() {
    let mut f = Fixture::default();
    let m = f.cube_mesh("Bolt", 0.005);
    f.roots = (0..4).map(|k| f.node(json!({"name": format!("Bolt.{k}"), "mesh": m, "translation": [k as f32 * 0.02, 0.0, 0.0]}))).collect();
    let g = read(&f.glb()).unwrap();
    assert_eq!(g.plan.slots, vec!["Bolt"], "named after the mesh: the nodes' names differ");
    assert_eq!(g.plan.places.len(), 4);
    assert!(g.plan.places.iter().all(|p| p.slot == 0 && p.parent.is_none()));
    assert!(close(g.plan.places[3].transform[3][0], 60.0));
    assert_eq!(meshes(&g).len(), 1, "the mesh is built once");
    // The same mesh scaled differently is not the same part.
    f.nodes[3]["scale"] = json!([2.0, 2.0, 2.0]);
    let g = read(&f.glb()).unwrap();
    assert_eq!(g.plan.slots.len(), 2);
    assert_eq!(meshes(&g).len(), 2);
}

#[test]
fn scale_and_mirror_are_baked_and_the_tree_kept() {
    let mut f = Fixture::default();
    let m = f.cube_mesh("Cube", 0.01);
    let child = f.node(json!({"name": "Child", "mesh": m, "translation": [0.1, 0.0, 0.0]}));
    let mirrored = f.node(json!({"name": "Mirror", "mesh": m, "scale": [-1.0, 1.0, 1.0]}));
    let scaled = f.node(json!({"name": "Half", "scale": [0.5, 0.5, 0.5], "children": [child]}));
    f.roots = vec![scaled, mirrored];
    let g = read(&f.glb()).unwrap();
    assert_eq!(g.plan.slots, vec!["Half", "Child", "Mirror"]);
    // The child sits half as far out, and is half the size.
    assert!(close(g.plan.places[1].transform[3][0], 50.0), "{:?}", g.plan.places[1].transform);
    let ms = meshes(&g);
    assert!(close(volume(&ms[0]), 125.0), "{}", volume(&ms[0]));
    // A mirrored mesh still faces out.
    assert!(close(volume(&ms[1]), 1000.0), "{}", volume(&ms[1]));
    // Placed, it lies on the other side of the origin.
    let place = &g.plan.places[2].transform;
    let xs: Vec<f64> = ms[1].positions.as_chunks::<3>().0.iter().map(|p| solidcraft_doc::apply_point(place, Vec3::new(p[0], p[1], p[2])).x).collect();
    assert!(
        close(xs.iter().copied().fold(f64::INFINITY, f64::min), -10.0) && close(xs.iter().copied().fold(f64::NEG_INFINITY, f64::max), 0.0),
        "{xs:?}"
    );
}

#[test]
fn materials_give_looks() {
    let mut f = Fixture {
        materials: vec![
            json!({"name": "Red metal", "pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0, 0.0, 1.0], "metallicFactor": 1.0, "roughnessFactor": 0.25}}),
            json!({"name": "Glass", "alphaMode": "BLEND", "pbrMetallicRoughness": {"baseColorFactor": [0.214, 0.214, 0.214, 0.5]}}),
        ],
        ..Default::default()
    };
    let (p, t) = cube(0.01);
    let q: Vec<[f32; 3]> = p.iter().map(|v| [v[0] + 0.02, v[1], v[2]]).collect();
    let m = f.mesh("Two", &[(&p, &t, Some(0)), (&q, &t, Some(1))]);
    let n = f.node(json!({"mesh": m}));
    f.roots = vec![n];
    let g = read(&f.glb()).unwrap();
    let ms = meshes(&g);
    assert_eq!(ms.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Two (Red metal)", "Two (Glass)"]);
    let red = g.looks[0].as_ref().unwrap();
    assert_eq!((red.name.as_str(), red.color, red.opacity), ("Red metal", [255, 0, 0], 1.0));
    let pbr = red.pbr.unwrap();
    assert!(close(pbr.metallic(), 1.0) && close(pbr.roughness(), 0.25));
    assert_eq!(ms[0].color, Some([1.0, 0.0, 0.0]));
    // Linear 0.214 is sRGB 128; a blended material is see-through and not PBR.
    let glass = g.looks[1].as_ref().unwrap();
    assert!((i32::from(glass.color[0]) - 128).abs() <= 1, "{:?}", glass.color);
    assert!(close(glass.opacity, 0.5) && glass.pbr.is_none());
}

#[test]
fn gltf_with_external_and_embedded_buffers() {
    let mut f = Fixture::default();
    let m = f.cube_mesh("Cube", 0.01);
    let n = f.node(json!({"mesh": m}));
    f.roots = vec![n];
    let text = f.json(Some("cube%20data.bin")).to_string();
    let bin = f.bin.clone();
    let mut asked = Vec::new();
    let mut load = |name: &str| {
        asked.push(name.to_string());
        Ok(bin.clone())
    };
    let g = read_gltf(text.as_bytes(), "dir/model.gltf", &mut load).unwrap();
    assert_eq!(meshes(&g).len(), 1);
    assert_eq!(asked, ["cube data.bin"]);
    // Missing: a clear error naming the file.
    let e = read_gltf(text.as_bytes(), "model.gltf", &mut no_files).err().unwrap().to_string();
    assert!(e.contains("cube data.bin"), "{e}");
    // Outside the file's folder: refused before anything is read.
    for uri in ["../secret.bin", "/etc/passwd", "C:/x.bin", "file:///x.bin", "a/%2e%2e/../b.bin"] {
        let text = f.json(Some(uri)).to_string();
        let mut called = false;
        let mut load = |_: &str| {
            called = true;
            Ok(Vec::new())
        };
        let r = read_gltf(text.as_bytes(), "model.gltf", &mut load);
        assert!(r.is_err() && !called, "{uri}");
    }
    // Embedded as a data URI.
    let b64 = {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in f.bin.chunks(3) {
            let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
            for k in 0..4 {
                s.push(if k <= c.len() { A[((n >> (18 - 6 * k)) & 63) as usize] as char } else { '=' });
            }
        }
        s
    };
    let text = f.json(Some(&format!("data:application/octet-stream;base64,{b64}"))).to_string();
    let g = read_gltf(text.as_bytes(), "model.gltf", &mut no_files).unwrap();
    assert!(close(volume(&meshes(&g)[0]), 1000.0));
    // The GLB route through mesh_import_feature builds every body where it is.
    let mut f2 = Fixture::default();
    let m = f2.cube_mesh("Cube", 0.01);
    f2.roots = vec![f2.node(json!({"mesh": m, "translation": [1.0, 0.0, 0.0]})), f2.node(json!({"mesh": m, "translation": [2.0, 0.0, 0.0]}))];
    let mf = crate::mesh_import_feature(&f2.glb(), "two.glb").unwrap();
    let solidcraft_doc::FeatureKind::MeshImport { meshes, .. } = &mf.kind else { panic!() };
    assert_eq!(meshes.len(), 1);
    assert!(close(bounds(&meshes[0]).0[0], 1000.0));
    assert!(crate::is_mesh_path("a.GLB") && crate::is_mesh_path("b.gltf") && is_gltf_path("c.Gltf") && !is_gltf_path("d.obj"));
}

#[test]
fn strips_fans_and_unsupported_parts() {
    let mut f = Fixture::default();
    // A square as a strip and as a fan, and a line primitive that is skipped.
    let sq: Vec<[f32; 3]> = vec![[0.0, 0.0, 0.0], [0.01, 0.0, 0.0], [0.0, 0.01, 0.0], [0.01, 0.01, 0.0]];
    let m = f.mesh("Sheet", &[(&sq, &[0, 1, 2, 3], None)]);
    f.meshes[m]["primitives"][0]["mode"] = json!(5);
    let n = f.node(json!({"mesh": m}));
    f.roots = vec![n];
    let g = read(&f.glb()).unwrap();
    assert_eq!(meshes(&g)[0].triangles.len(), 6);
    f.meshes[m]["primitives"][0]["mode"] = json!(1);
    let e = read(&f.glb()).err().unwrap().to_string();
    assert!(e.contains("no triangle meshes"), "{e}");
}

/// The plan makes each component at its first placement, inside one made before (never inside
/// itself), and puts bodies only in components it makes.
fn plan_ok(p: &MeshPlan) -> bool {
    let mut made = 0;
    for place in &p.places {
        if place.slot > made || place.parent.is_some_and(|q| q >= made || q == place.slot) {
            return false;
        }
        made += usize::from(place.slot == made);
    }
    made == p.slots.len() && p.bodies.iter().flatten().all(|b| *b < made)
}

fn assert_no_panic(bytes: &[u8], what: &str) {
    let r = std::panic::catch_unwind(|| read(bytes).map(|g| (meshes(&g).len(), plan_ok(&g.plan), g.looks.len() == meshes(&g).len())));
    assert!(r.is_ok(), "panicked on {what}");
    if let Ok(Ok((_, plan, looks))) = r {
        assert!(plan && looks, "inconsistent plan on {what}");
    }
}

/// A primitive whose POSITION names an accessor the file lacks is refused before the glTF crate
/// sees it: the crate's own check indexes the accessors unchecked and panics, which the guard
/// catches on the desktop but which aborts the app in the browser (#469, found by mutation).
#[test]
fn a_position_naming_a_missing_accessor_is_refused_before_the_gltf_crate() {
    let mut f = Fixture::default();
    let m = f.cube_mesh("Cube", 0.01);
    let n = f.node(json!({"mesh": m}));
    f.roots = vec![n];
    let mut j = f.json(None);
    j["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(99);
    let glb = glb_container(j.to_string().as_bytes(), &f.bin);
    let e = read(&glb).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains("POSITION names accessor 99, but the file has 2"), "{e}");
    assert!(!solidcraft_kernel::guarded(), "no guard left open");
}

#[test]
fn hostile_gltf_never_panics() {
    let mut f = Fixture::default();
    let m = f.cube_mesh("Cube", 0.01);
    let child = f.node(json!({"name": "C", "mesh": m, "translation": [0.0, 0.1, 0.0]}));
    let n = f.node(json!({"mesh": m, "children": [child], "scale": [2.0, 1.0, 1.0]}));
    f.roots = vec![n, child];
    let glb = f.glb();
    assert!(read(&glb).is_ok());
    // Every truncation is an error, never a panic.
    for k in 0..glb.len() {
        assert_no_panic(&glb[..k], &format!("truncated to {k}"));
        assert!(read(&glb[..k]).is_err(), "truncated to {k}");
    }
    // Flipped bytes anywhere.
    for k in 0..glb.len() {
        let mut bad = glb.clone();
        bad[k] ^= 0xA5;
        assert_no_panic(&bad, &format!("byte {k} flipped"));
    }
    // Lengths in the container that lie.
    for (at, v) in [(8, 0u32), (8, 5), (8, u32::MAX), (12, 0), (12, u32::MAX), (4, 1)] {
        let mut bad = glb.clone();
        bad[at..at + 4].copy_from_slice(&v.to_le_bytes());
        assert_no_panic(&bad, &format!("{v} at {at}"));
        assert!(read(&bad).is_err(), "{v} at {at}");
    }
    // Documents that lie.
    let base = f.json(None);
    let cases: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
        ("huge count", Box::new(|j| j["accessors"][0]["count"] = json!(u32::MAX))),
        ("past the view", Box::new(|j| j["accessors"][0]["byteOffset"] = json!(1000))),
        ("view past buffer", Box::new(|j| j["bufferViews"][0]["byteLength"] = json!(1 << 20))),
        ("buffer longer than chunk", Box::new(|j| j["buffers"][0]["byteLength"] = json!(1 << 20))),
        ("bad accessor index", Box::new(|j| j["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(99))),
        ("bad mesh index", Box::new(|j| j["nodes"][0]["mesh"] = json!(99))),
        ("bad child", Box::new(|j| j["nodes"][1]["children"] = json!([99]))),
        ("loop", Box::new(|j| j["nodes"][0]["children"] = json!([1, 0]))),
        ("two-node loop", Box::new(|j| j["nodes"][0]["children"] = json!([1]))),
        ("index past vertices", Box::new(|j| j["accessors"][1]["componentType"] = json!(5125))),
        ("float indices", Box::new(|j| j["accessors"][1]["componentType"] = json!(5126))),
        ("stride too small", Box::new(|j| j["bufferViews"][0]["byteStride"] = json!(4))),
        ("nan transform", Box::new(|j| j["nodes"][1]["matrix"] = json!([f32::MAX, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 1e30, 0, 0, 1]))),
        ("zero scale", Box::new(|j| j["nodes"][1]["scale"] = json!([0.0, 0.0, 0.0]))),
        ("draco", Box::new(|j| j["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]))),
        ("version 1", Box::new(|j| j["asset"]["version"] = json!("1.0"))),
        ("no scenes", Box::new(|j| j.as_object_mut().unwrap().retain(|k, _| k != "scenes" && k != "scene"))),
        (
            "sparse",
            Box::new(|j| {
                j["accessors"][0]["sparse"] = json!({"count": 1, "indices": {"bufferView": 1, "componentType": 5123}, "values": {"bufferView": 0}})
            }),
        ),
        ("no buffer", Box::new(|j| j["buffers"] = json!([]))),
        ("not json", Box::new(|j| *j = json!("nope"))),
    ];
    for (what, edit) in &cases {
        let mut j = base.clone();
        edit(&mut j);
        let bytes = glb_container(j.to_string().as_bytes(), &f.bin);
        assert_no_panic(&bytes, what);
        let text = j.to_string();
        let r = std::panic::catch_unwind(|| read_gltf(text.as_bytes(), "x.gltf", &mut no_files).map(|g| meshes(&g).len()));
        assert!(r.is_ok(), "panicked on {what} (gltf)");
    }
    let mut j = base.clone();
    j["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
    let e = read(&glb_container(j.to_string().as_bytes(), &f.bin)).err().unwrap().to_string();
    assert!(e.contains("KHR_draco_mesh_compression") && e.contains("uncompressed"), "{e}");
    // Looks-only extensions don't stop a file.
    j["extensionsRequired"] = json!(["KHR_texture_transform", "KHR_materials_emissive_strength"]);
    assert!(read(&glb_container(j.to_string().as_bytes(), &f.bin)).is_ok());
    // A deep chain is cut off or flattened, not followed forever.
    let mut f = Fixture::default();
    let m = f.cube_mesh("Cube", 0.01);
    let mut last = f.node(json!({"mesh": m}));
    for _ in 0..200 {
        last = f.node(json!({"mesh": m, "translation": [0.0, 0.0, 0.01], "children": [last]}));
    }
    f.roots = vec![last, 0];
    let g = read(&f.glb()).unwrap();
    assert!(g.plan.slots.len() <= MAX_COMPONENTS);
    assert!(g.feature.warnings.iter().any(|w| w.contains("too large or too deep")), "{:?}", g.feature.warnings);
    for junk in [&b""[..], b"glTF", b"glTF\x02\0\0\0", b"{", b"\xff\xfe", b"[]", b"{\"asset\":{\"version\":\"2.0\"}}"] {
        assert_no_panic(junk, &format!("{junk:?}"));
        assert!(read(junk).is_err());
    }
}
