//! GPU viewport: shaded triangles and screen-space wide lines with a depth buffer, drawn in an
//! `egui_wgpu` paint callback. Geometry is uploaded when the scene changes; each frame only the
//! camera uniform changes. Where the device runs compute shaders, opaque bodies are drawn
//! straight from their display plans: the plan is evaluated on the GPU into the vertex buffer
//! the pass draws (see [`PlannedBody`]), with nothing built or read back on the CPU.

use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

/// Triangle vertex: position (3 × f32), normal (3 × f32), sRGB colour (4 × u8).
pub const TRI_SIZE: usize = 28;
#[path = "pbr_gpu.rs"]
mod pbr;
pub const PBR_SIZE: usize = pbr::VERTEX_SIZE;
/// Line instance: a, b (3 × f32 each), colour (4 × u8), width in pixels (f32).
pub const LINE_SIZE: usize = 32;
/// Shader uniform including analysis colours and ambient occlusion.
const UNIFORM_SIZE: u64 = 368;

#[path = "gpu_ao.rs"]
mod ao;

/// A body drawn from its display plan, evaluated on the GPU: the plan, and a colour per face
/// (the plan's face numbering; faces past the end take the last colour).
#[derive(Clone)]
pub struct PlannedBody {
    pub plan: Arc<solidcraft_engine::geom::tess::TessPlan>,
    pub face_colors: Vec<[u8; 4]>,
}

/// CPU-side geometry waiting to be uploaded. The model scene changes with the design; the
/// highlight scene (hover, selection, the origin widget) changes often and is small.
#[derive(Default, Clone)]
pub struct GpuScene {
    /// Opaque triangles. A zero normal draws the colour flat (unlit).
    pub tris: Vec<u8>,
    /// Opaque product PBR vertices; linear base color and explicit optical parameters.
    pub pbr: Vec<u8>,
    /// Translucent triangles (alpha from the colour), drawn after the opaque ones without
    /// writing depth.
    pub trans: Vec<u8>,
    /// See-through bodies and faces (appearance opacity below 1), closed and outward-wound:
    /// their far sides are drawn first, then their near sides, so they read like glass.
    pub glass: Vec<u8>,
    /// Depth-tested lines.
    pub lines: Vec<u8>,
    /// Lines drawn only where the model hides them (hidden edges).
    pub hidden: Vec<u8>,
    /// Lines drawn over everything (active sketch, highlights).
    pub overlay: Vec<u8>,
    /// Translucent triangles pushed slightly back in depth, so they only show where nothing
    /// coincides with them (removed material in a preview).
    pub ghost: Vec<u8>,
    /// Translucent triangles drawn through everything (a cut's tool body).
    pub xray: Vec<u8>,
    /// Lines drawn first, under everything (the ground grid: the model hides it, it never
    /// hides the model).
    pub under: Vec<u8>,
    /// Origin and construction planes (translucent, like `trans`) and their axes (lines, like
    /// `lines`): reference geometry, never cut away by a section (#1229).
    pub guide_trans: Vec<u8>,
    pub guide_lines: Vec<u8>,
    /// Opaque bodies drawn from their plans on the GPU, like `tris`.
    pub planned: Vec<PlannedBody>,
    /// The model's bodies, a few at a time, drawn before the loose geometry above (`tris`,
    /// `trans`, `lines` and `hidden` of each block, in order, then the scene's own). A block
    /// that comes back unchanged keeps its buffers: a design changing one body at a time (an
    /// agent adding bodies over MCP) uploads only the blocks that changed, not the whole model.
    pub blocks: Vec<SceneBlock>,
}

#[cfg(test)]
impl GpuScene {
    /// The blocks' geometry moved into the scene's own lists, in drawing order (for tests that
    /// count what the scene holds).
    pub fn merged(mut self) -> GpuScene {
        let mut out = GpuScene { glass: std::mem::take(&mut self.glass), ..Default::default() };
        for block in std::mem::take(&mut self.blocks) {
            out.tris.extend_from_slice(&block.bytes.tris);
            out.trans.extend_from_slice(&block.bytes.trans);
            out.lines.extend_from_slice(&block.bytes.lines);
            out.hidden.extend_from_slice(&block.bytes.hidden);
        }
        out.tris.append(&mut self.tris);
        out.trans.append(&mut self.trans);
        out.lines.append(&mut self.lines);
        out.hidden.append(&mut self.hidden);
        GpuScene { tris: out.tris, trans: out.trans, glass: out.glass, lines: out.lines, hidden: out.hidden, ..self }
    }
}

/// A run of the model's bodies in the scene: their triangles and edges, and a key that names
/// exactly this content (the viewport keeps the bytes while the key comes back).
#[derive(Clone)]
pub struct SceneBlock {
    pub key: u64,
    pub bytes: Arc<BlockBytes>,
}

/// A block's geometry, laid out as in [`GpuScene`].
#[derive(Default)]
pub struct BlockBytes {
    pub tris: Vec<u8>,
    pub trans: Vec<u8>,
    pub glass: Vec<u8>,
    pub lines: Vec<u8>,
    pub hidden: Vec<u8>,
}

impl GpuScene {
    pub fn pbr_tri(&mut self, p: [f32; 3], n: [f32; 3], surface: solidcraft_engine::render::surface::OpaqueSurface) {
        let base = surface.base_color().to_f32();
        let parameters = [surface.metallic() as f32, surface.roughness() as f32, surface.dielectric_ior() as f32];
        for value in p.into_iter().chain(n).chain(base).chain(parameters) {
            self.pbr.extend_from_slice(&value.to_le_bytes());
        }
    }
    pub fn tri(&mut self, p: [f32; 3], n: [f32; 3], c: [u8; 4]) {
        for v in p.iter().chain(&n) {
            self.tris.extend_from_slice(&v.to_le_bytes());
        }
        self.tris.extend_from_slice(&c);
    }
    pub fn trans_tri(&mut self, p: [f32; 3], n: [f32; 3], c: [u8; 4]) {
        for v in p.iter().chain(&n) {
            self.trans.extend_from_slice(&v.to_le_bytes());
        }
        self.trans.extend_from_slice(&c);
    }
    /// A line shown only where the model hides it.
    pub fn hidden_line(&mut self, a: [f32; 3], b: [f32; 3], c: [u8; 4], width: f32) {
        for v in a.iter().chain(&b) {
            self.hidden.extend_from_slice(&v.to_le_bytes());
        }
        self.hidden.extend_from_slice(&c);
        self.hidden.extend_from_slice(&width.to_le_bytes());
    }
    pub fn glass_tri(&mut self, p: [f32; 3], n: [f32; 3], c: [u8; 4]) {
        for v in p.iter().chain(&n) {
            self.glass.extend_from_slice(&v.to_le_bytes());
        }
        self.glass.extend_from_slice(&c);
    }
    pub fn ghost_tri(&mut self, p: [f32; 3], n: [f32; 3], c: [u8; 4]) {
        for v in p.iter().chain(&n) {
            self.ghost.extend_from_slice(&v.to_le_bytes());
        }
        self.ghost.extend_from_slice(&c);
    }
    pub fn xray_tri(&mut self, p: [f32; 3], n: [f32; 3], c: [u8; 4]) {
        for v in p.iter().chain(&n) {
            self.xray.extend_from_slice(&v.to_le_bytes());
        }
        self.xray.extend_from_slice(&c);
    }
    /// A line under everything (see [`GpuScene::under`]).
    pub fn under_line(&mut self, a: [f32; 3], b: [f32; 3], c: [u8; 4], width: f32) {
        for v in a.iter().chain(&b) {
            self.under.extend_from_slice(&v.to_le_bytes());
        }
        self.under.extend_from_slice(&c);
        self.under.extend_from_slice(&width.to_le_bytes());
    }
    /// A translucent triangle of a plane a section does not cut (see [`GpuScene::guide_trans`]).
    pub fn guide_tri(&mut self, p: [f32; 3], c: [u8; 4]) {
        for v in p.iter().chain(&[0.0; 3]) {
            self.guide_trans.extend_from_slice(&v.to_le_bytes());
        }
        self.guide_trans.extend_from_slice(&c);
    }
    /// A depth-tested line a section does not cut (see [`GpuScene::guide_lines`]).
    pub fn guide_line(&mut self, a: [f32; 3], b: [f32; 3], c: [u8; 4], width: f32) {
        for v in a.iter().chain(&b) {
            self.guide_lines.extend_from_slice(&v.to_le_bytes());
        }
        self.guide_lines.extend_from_slice(&c);
        self.guide_lines.extend_from_slice(&width.to_le_bytes());
    }
    pub fn line(&mut self, a: [f32; 3], b: [f32; 3], c: [u8; 4], width: f32, on_top: bool) {
        let out = if on_top { &mut self.overlay } else { &mut self.lines };
        for v in a.iter().chain(&b) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&c);
        out.extend_from_slice(&width.to_le_bytes());
    }
}

pub type SceneSlot = Arc<Mutex<Option<GpuScene>>>;

/// A reference image (canvas or decal): a textured quad, depth-tested against the model.
#[derive(Clone)]
pub struct GpuImage {
    pub id: u64,
    /// Changes when the pixels change (the texture is re-uploaded).
    pub version: u64,
    pub size: [u32; 2],
    /// Straight RGBA8, `size[0] * size[1] * 4` bytes.
    pub rgba: Arc<Vec<u8>>,
    /// Bottom-left, bottom-right, top-right, top-left of the image.
    pub corners: [[f32; 3]; 4],
    pub opacity: f32,
}

/// Image vertex: position (3 × f32), uv (2 × f32), opacity (f32).
const IMG_SIZE: usize = 24;

/// Per-frame parameters.
pub struct ViewportCallback {
    /// Product preview uses its own batches, retaining the Design viewport's uploads.
    pub product: bool,
    pub key: u64,
    pub slot: SceneSlot,
    pub sketch_key: u64,
    pub sketch_slot: SceneSlot,
    /// The highlight scene and its key.
    pub hl_key: u64,
    pub hl_slot: SceneSlot,
    /// The live preview scene and its key.
    pub pv_key: u64,
    pub pv_slot: SceneSlot,
    /// Column-major view-projection.
    pub view_proj: [[f32; 4]; 4],
    /// Direction toward the eye (for lighting).
    pub back: [f32; 3],
    /// Viewport size in physical pixels.
    pub size_px: [f32; 2],
    /// Section cut: (normal, d); fragments with normal·p > d are hidden. A zero normal: none.
    pub clip: [f32; 4],
    /// Colour of the inside of cut bodies (straight sRGBA, 0..1).
    pub cap: [f32; 4],
    /// Canvases and decals.
    pub images: Vec<GpuImage>,
    /// Surface analysis: (mode: 0 none, 1 zebra, 2 draft, 3 curvature, 4 environment,
    /// 5 accessibility; parameter; scale; size) and the pull / access direction.
    pub analysis: [f32; 8],
    /// The analysis colours and the stripe flag ([`analysis_colors`]).
    pub analysis_colors: [f32; 36],
    /// Accessibility: the depth map seen from the access direction.
    pub access: Option<AccessMap>,
    /// Draw silhouette outlines around the model (visual styles with edges).
    pub outline: bool,
    /// Eye position and 1 for a perspective camera (0: orthographic), for the section cap's depth
    /// and the depth bias.
    pub eye: [f32; 4],
    /// World-space occlusion radius; zero skips the depth prepass and all AO sampling.
    pub ao_radius: f32,
    /// Supersampling (#1288): draw the frame this many times the viewport's pixels in each
    /// direction and filter it down (1: off).
    pub ssaa: f32,
}

/// Heights of the model seen from a direction (an orthographic depth map): a point is reachable
/// from that direction when nothing lies above it.
#[derive(Clone)]
pub struct AccessMap {
    pub version: u64,
    pub size: u32,
    /// Row-major heights along the direction (−∞ where nothing is).
    pub depth: Arc<Vec<f32>>,
    /// Map axes: pixel = (p·u + u.w, p·v + v.w) × scale.
    pub u: [f32; 4],
    pub v: [f32; 4],
    pub scale: f32,
}

/// Vertices (or line instances) on the GPU, in as many buffers as the device's buffer size
/// limit needs: a large import's triangles can exceed one buffer (#883).
struct Batch {
    /// Each buffer and its vertex (instance) count.
    parts: Vec<(wgpu::Buffer, u32)>,
    /// Bytes per buffer (all but the last are full).
    chunk: usize,
}

/// A chunk of planned bodies on the GPU: their evaluated mesh and a colour per vertex.
struct PlannedDraw {
    mesh: Arc<solidcraft_gpu::GpuMesh>,
    colors: wgpu::Buffer,
}

/// A scene block on the GPU, with the bytes it was uploaded from.
struct BlockGpu {
    bytes: Arc<BlockBytes>,
    tris: Option<Batch>,
    trans: Option<Batch>,
    lines: Option<Batch>,
    hidden: Option<Batch>,
}

/// The blocks' buffers, then the scene's own, as one batch (the blocks' buffers are shared,
/// not copied).
fn joined<'a>(blocks: impl Iterator<Item = &'a Option<Batch>>, own: Option<Batch>) -> Option<Batch> {
    let mut parts: Vec<(wgpu::Buffer, u32)> = blocks.flatten().flat_map(|b| b.parts.iter().cloned()).collect();
    let chunk = own.as_ref().map(|b| b.chunk).unwrap_or(0);
    parts.extend(own.into_iter().flat_map(|b| b.parts));
    (!parts.is_empty()).then_some(Batch { parts, chunk })
}

#[derive(Default)]
struct Batches {
    /// The scene blocks on the GPU, by key.
    blocks: std::collections::HashMap<u64, BlockGpu>,
    key: Option<u64>,
    tris: Option<Batch>,
    pbr: Option<Batch>,
    trans: Option<Batch>,
    glass: Option<Batch>,
    /// The glass triangles as built, and the view direction they were last sorted for.
    glass_cpu: Vec<u8>,
    glass_sorted_for: Option<[f32; 3]>,
    lines: Option<Batch>,
    hidden: Option<Batch>,
    overlays: Option<Batch>,
    ghost: Option<Batch>,
    xray: Option<Batch>,
    under: Option<Batch>,
    guide_trans: Option<Batch>,
    guide_lines: Option<Batch>,
    planned: Vec<PlannedDraw>,
    /// Evaluated chunks by their plans' addresses, holding the plans so the addresses stay
    /// theirs: a chunk whose plans are unchanged is not evaluated again when the scene is rebuilt.
    resident: std::collections::HashMap<Vec<usize>, (Vec<Arc<solidcraft_engine::geom::tess::TessPlan>>, Arc<solidcraft_gpu::GpuMesh>)>,
}

impl Batches {
    /// Upload a new scene. Planned bodies not yet on the GPU are evaluated there: their
    /// dispatches go into `encoder`, ahead of this frame's render pass.
    fn take(&mut self, device: &wgpu::Device, key: u64, slot: &SceneSlot, tess: Option<(&solidcraft_gpu::TessGpu, &mut wgpu::CommandEncoder)>) {
        if self.key == Some(key) {
            return;
        }
        if let Some(sc) = slot.lock().ok().and_then(|mut s| s.take()) {
            self.planned = match tess {
                Some((tess, encoder)) => self.planned(device, tess, encoder, &sc.planned),
                None => Vec::new(),
            };
            let mut kept = std::mem::take(&mut self.blocks);
            let mut blocks: Vec<&BlockGpu> = Vec::with_capacity(sc.blocks.len());
            for block in &sc.blocks {
                let gpu = match kept.remove(&block.key).filter(|g| Arc::ptr_eq(&g.bytes, &block.bytes)) {
                    Some(g) => g,
                    None => BlockGpu {
                        bytes: block.bytes.clone(),
                        tris: upload(device, "sc_tris", &block.bytes.tris, TRI_SIZE),
                        trans: upload(device, "sc_trans", &block.bytes.trans, TRI_SIZE),
                        lines: upload(device, "sc_lines", &block.bytes.lines, LINE_SIZE),
                        hidden: upload(device, "sc_hidden", &block.bytes.hidden, LINE_SIZE),
                    },
                };
                self.blocks.insert(block.key, gpu);
            }
            for block in &sc.blocks {
                if let Some(g) = self.blocks.get(&block.key) {
                    blocks.push(g);
                }
            }
            self.tris = joined(blocks.iter().map(|b| &b.tris), upload(device, "sc_tris", &sc.tris, TRI_SIZE));
            self.pbr = upload(device, "sc_pbr", &sc.pbr, PBR_SIZE);
            self.trans = joined(blocks.iter().map(|b| &b.trans), upload(device, "sc_trans", &sc.trans, TRI_SIZE));
            self.glass = upload_with(device, "sc_glass", &sc.glass, TRI_SIZE, wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST);
            self.glass_cpu = sc.glass;
            self.glass_sorted_for = None;
            self.lines = joined(blocks.iter().map(|b| &b.lines), upload(device, "sc_lines", &sc.lines, LINE_SIZE));
            self.hidden = joined(blocks.iter().map(|b| &b.hidden), upload(device, "sc_hidden", &sc.hidden, LINE_SIZE));
            self.overlays = upload(device, "sc_overlay", &sc.overlay, LINE_SIZE);
            self.ghost = upload(device, "sc_ghost", &sc.ghost, TRI_SIZE);
            self.xray = upload(device, "sc_xray", &sc.xray, TRI_SIZE);
            self.under = upload(device, "sc_under", &sc.under, LINE_SIZE);
            self.guide_trans = upload(device, "sc_guide_trans", &sc.guide_trans, TRI_SIZE);
            self.guide_lines = upload(device, "sc_guide_lines", &sc.guide_lines, LINE_SIZE);
            self.key = Some(key);
        }
    }

    /// The planned bodies on the GPU: in chunks of at most `MAX_BATCH` vertices (one for any
    /// ordinary scene), each uploaded and evaluated in one dispatch and drawn in one call. A
    /// chunk whose plans are unchanged is not evaluated again; colours are rebuilt each time.
    fn planned(
        &mut self,
        device: &wgpu::Device,
        tess: &solidcraft_gpu::TessGpu,
        encoder: &mut wgpu::CommandEncoder,
        bodies: &[PlannedBody],
    ) -> Vec<PlannedDraw> {
        let mut chunks: Vec<Vec<&PlannedBody>> = Vec::new();
        let mut filled = 0usize;
        for b in bodies {
            let n = b.plan.vertex_count();
            match chunks.last_mut() {
                Some(c) if filled + n <= solidcraft_gpu::MAX_BATCH => c.push(b),
                _ => {
                    chunks.push(vec![b]);
                    filled = 0;
                }
            }
            filled += n;
        }
        let key = |c: &[&PlannedBody]| c.iter().map(|b| Arc::as_ptr(&b.plan) as usize).collect::<Vec<usize>>();
        self.resident.retain(|k, _| chunks.iter().any(|c| key(c) == *k));
        let mut out = Vec::with_capacity(chunks.len());
        for chunk in &chunks {
            let mesh = match self.resident.get(&key(chunk)) {
                Some((_, m)) => m.clone(),
                None => {
                    let plans: Vec<&solidcraft_engine::geom::tess::TessPlan> = chunk.iter().map(|b| &*b.plan).collect();
                    match tess.prepare_many(&plans) {
                        Ok(m) => {
                            tess.record(encoder, &m);
                            let m = Arc::new(m);
                            self.resident.insert(key(chunk), (chunk.iter().map(|b| b.plan.clone()).collect(), m.clone()));
                            m
                        }
                        Err(e) => {
                            log::warn!("bodies are not drawn on the GPU: {e}");
                            continue;
                        }
                    }
                }
            };
            let mut bytes = Vec::with_capacity(mesh.vertex_face.len() * 4);
            for (part, b) in mesh.parts.iter().zip(chunk) {
                let last = b.face_colors.last().copied().unwrap_or([200, 200, 200, 255]);
                let faces = mesh.vertex_face.get(part.vertices.start as usize..part.vertices.end as usize).unwrap_or_default();
                bytes.extend(faces.iter().flat_map(|f| b.face_colors.get(*f as usize).copied().unwrap_or(last)));
            }
            if let Some((colors, _)) = upload(device, "sc_planned_colors", &bytes, 4).and_then(|c| c.parts.into_iter().next()) {
                out.push(PlannedDraw { mesh, colors });
            }
        }
        out
    }
}

struct Resources {
    ao: ao::Resources,
    tri: wgpu::RenderPipeline,
    /// `tri` and `tri_cut` for planned bodies (positions and normals from the GPU evaluation,
    /// colours per vertex, indexed).
    tri_planned: wgpu::RenderPipeline,
    tri_cut_planned: Option<(wgpu::RenderPipeline, wgpu::RenderPipeline)>,
    /// The tessellation compute pipeline, where the device runs compute shaders.
    tess: Option<solidcraft_gpu::TessGpu>,
    /// While a section is shown (when the depth buffer has a stencil): `tri` marking where the
    /// cap shows, then the cap again at the section plane's depth, so it hides the inside.
    tri_cut: Option<(wgpu::RenderPipeline, wgpu::RenderPipeline)>,
    /// Highlight triangles over the model (depth test ≤, no depth write).
    tri_hl: wgpu::RenderPipeline,
    pbr: wgpu::RenderPipeline,
    trans: wgpu::RenderPipeline,
    /// Glass: far sides (front faces culled), then near sides (back faces culled).
    glass_far: wgpu::RenderPipeline,
    glass_near: wgpu::RenderPipeline,
    ghost: wgpu::RenderPipeline,
    xray: wgpu::RenderPipeline,
    line: wgpu::RenderPipeline,
    /// Lines behind the model (depth test >).
    line_hidden: wgpu::RenderPipeline,
    overlay: wgpu::RenderPipeline,
    /// Reference planes and axes: as `trans` and `line`, but not cut by a section.
    guide_trans: wgpu::RenderPipeline,
    guide_line: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    linear_out: bool,
    model: Batches,
    product: Batches,
    empty: Batches,
    sketch: Batches,
    highlight: Batches,
    preview: Batches,
    image: wgpu::RenderPipeline,
    image_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Textures by image id: (version, bind group).
    textures: std::collections::HashMap<u64, (u64, wgpu::BindGroup)>,
    /// This frame's quads: (image id, vertices).
    quads: Vec<(u64, Batch)>,
    access_bgl: wgpu::BindGroupLayout,
    /// The accessibility depth map (a 1 × 1 stand-in when there is none) and its version.
    access_bind: wgpu::BindGroup,
    access_version: u64,
    /// The target the pipelines draw into (format, depth, samples): a supersampled frame is
    /// drawn into textures of the same kind.
    target: GpuTarget,
    /// The device's largest 2D texture side (#869).
    max_texture: u32,
    /// Supersampling (#1288): the filter that draws the supersampled frame into the viewport.
    ss_blit: wgpu::RenderPipeline,
    ss_bgl: wgpu::BindGroupLayout,
    /// The supersampled frame's textures, kept while its size stays.
    ss: Option<Supersampled>,
    /// This frame was drawn supersampled (in `prepare`): `paint` only filters it down.
    ss_ready: bool,
}

/// A supersampled frame: the scene drawn at a multiple of the viewport's pixels.
struct Supersampled {
    size: [u32; 2],
    /// Drawn into (multisampled with the viewport's MSAA).
    color: wgpu::TextureView,
    /// The resolved colour (when `color` is multisampled).
    resolve: Option<wgpu::TextureView>,
    depth: Option<wgpu::TextureView>,
    /// The single-sampled colour, for the filter.
    bind: wgpu::BindGroup,
}

/// Most memory (bytes) a supersampled frame's colour and depth may take.
const SS_BUDGET: u64 = 384 << 20;

/// The size (px) to draw a `size` viewport supersampled `factor` times in each direction:
/// within the device's largest texture side (#869) and the memory budget (at `samples` per pixel,
/// four bytes of colour and four of depth each). `None` when that leaves under 1.25×: the frame
/// is drawn as usual.
pub(crate) fn supersampled_size(size: [f32; 2], factor: f32, max_side: u32, samples: u32) -> Option<[u32; 2]> {
    let [w, h] = size;
    if !(w >= 1.0 && h >= 1.0 && factor.is_finite() && factor > 1.0) {
        return None;
    }
    let side = max_side as f32;
    let bytes_per_px = 8.0 * samples.max(1) as f32;
    let by_memory = (SS_BUDGET as f32 / (w * h * bytes_per_px)).sqrt();
    let f = factor.min(side / w).min(side / h).min(by_memory);
    (f >= 1.25).then(|| [((w * f).floor() as u32).clamp(1, max_side), ((h * f).floor() as u32).clamp(1, max_side)])
}

/// The filter: each viewport pixel the mean of the supersampled pixels under it (four bilinear
/// taps a quarter of its footprint from its centre: exactly the 2 × 2 box at 2×).
const SS_SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
struct O { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs(@builtin(vertex_index) i: u32) -> O {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: O;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}
@fragment
fn fs(i: O) -> @location(0) vec4<f32> {
    let d = vec2<f32>(dpdx(i.uv.x), dpdy(i.uv.y)) * 0.25;
    let a = textureSampleLevel(src, smp, i.uv + vec2<f32>(-d.x, -d.y), 0.0);
    let b = textureSampleLevel(src, smp, i.uv + vec2<f32>(d.x, -d.y), 0.0);
    let c = textureSampleLevel(src, smp, i.uv + vec2<f32>(-d.x, d.y), 0.0);
    let e = textureSampleLevel(src, smp, i.uv + vec2<f32>(d.x, d.y), 0.0);
    return (a + b + c + e) * 0.25;
}
"#;

fn access_texture(device: &wgpu::Device, queue: Option<&wgpu::Queue>, bgl: &wgpu::BindGroupLayout, size: u32, depth: &[f32]) -> wgpu::BindGroup {
    let extent = wgpu::Extent3d { width: size.max(1), height: size.max(1), depth_or_array_layers: 1 };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sc_access"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    if let Some(q) = queue
        && depth.len() == (size as usize) * (size as usize)
    {
        let bytes: Vec<u8> = depth.iter().flat_map(|x| x.to_le_bytes()).collect();
        q.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * size), rows_per_image: Some(size) },
            extent,
        );
    }
    let view = tex.create_view(&Default::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sc_access"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view) }],
    })
}

const SHADER: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/viewport.wgsl"));

fn tri_layout() -> wgpu::VertexBufferLayout<'static> {
    const A: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4];
    wgpu::VertexBufferLayout { array_stride: TRI_SIZE as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &A }
}

/// A planned body's vertices as the GPU evaluation writes them: the position, then the normal
/// at [`solidcraft_gpu::NORMAL_OFFSET`] (each padded to four floats).
fn pbr_layout() -> wgpu::VertexBufferLayout<'static> {
    const A: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x3];
    wgpu::VertexBufferLayout { array_stride: PBR_SIZE as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &A }
}

fn planned_layout() -> wgpu::VertexBufferLayout<'static> {
    const A: [wgpu::VertexAttribute; 2] = [
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: solidcraft_gpu::NORMAL_OFFSET, shader_location: 1 },
    ];
    wgpu::VertexBufferLayout { array_stride: solidcraft_gpu::VERTEX_STRIDE, step_mode: wgpu::VertexStepMode::Vertex, attributes: &A }
}

/// A planned body's colours, one per vertex.
fn color_layout() -> wgpu::VertexBufferLayout<'static> {
    const A: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![2 => Unorm8x4];
    wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Vertex, attributes: &A }
}

fn line_layout() -> wgpu::VertexBufferLayout<'static> {
    const A: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Float32];
    wgpu::VertexBufferLayout { array_stride: LINE_SIZE as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &A }
}

/// The target configuration: colour format, depth format and MSAA sample count of the app.
#[derive(Clone, Copy, Debug)]
pub struct GpuTarget {
    pub format: wgpu::TextureFormat,
    pub depth: Option<wgpu::TextureFormat>,
    pub samples: u32,
    /// The device evaluates display plans (compute shaders): bodies may be drawn from them.
    pub tess: bool,
}

impl Resources {
    fn new(device: &wgpu::Device, t: GpuTarget, tess: Option<solidcraft_gpu::TessGpu>) -> Self {
        let source = format!("{SHADER}\n{}", pbr::SOURCE);
        let module =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sc_viewport"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sc_viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc_viewport"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sc_viewport_u"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sc_viewport"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let depth = |compare: wgpu::CompareFunction, write: bool| -> Option<wgpu::DepthStencilState> {
            t.depth.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        buf: wgpu::VertexBufferLayout<'static>,
                        ds: Option<wgpu::DepthStencilState>,
                        blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), buffers: &[Some(buf)], compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
                depth_stencil: ds,
                multisample: wgpu::MultisampleState { count: t.samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: t.format, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let glass = |label: &str, cull: wgpu::Face| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_tri"),
                    buffers: &[Some(tri_layout())],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: Some(cull), ..Default::default() },
                depth_stencil: depth(wgpu::CompareFunction::LessEqual, false),
                multisample: wgpu::MultisampleState { count: t.samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_tri"),
                    targets: &[Some(wgpu::ColorTargetState { format: t.format, blend: alpha, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let image_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sc_image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let image_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc_image"),
            bind_group_layouts: &[Some(&bgl), Some(&image_bgl)],
            immediate_size: 0,
        });
        const IA: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32];
        let image = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sc_image"),
            layout: Some(&image_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_img"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: IMG_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &IA,
                })],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: depth(wgpu::CompareFunction::LessEqual, false),
            multisample: wgpu::MultisampleState { count: t.samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_img"),
                targets: &[Some(wgpu::ColorTargetState { format: t.format, blend: alpha, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let access_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sc_access"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let ao = ao::Resources::new(device, &bgl, &module);
        let solid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc_solid"),
            bind_group_layouts: &[Some(&bgl), Some(&access_bgl), Some(&ao.layout)],
            immediate_size: 0,
        });
        let built = [Some(tri_layout())];
        let evaluated = [Some(planned_layout()), Some(color_layout())];
        let solid = |label: &str, fs: &str, cull: Option<wgpu::Face>, ds: Option<wgpu::DepthStencilState>, planned: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&solid_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_tri"),
                    buffers: if planned { &evaluated[..] } else { &built[..] },
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: cull, ..Default::default() },
                depth_stencil: ds,
                multisample: wgpu::MultisampleState { count: t.samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: t.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // While a section is shown: the stencil marking and the cap (see `Resources::tri_cut`).
        let cut = |planned: bool| {
            t.depth.filter(|f| f.has_stencil_aspect()).map(|format| {
                // The visible surface marks the stencil: 1 where it is a back face (the cap).
                use wgpu::StencilOperation::{Keep, Replace, Zero};
                let face = |pass_op| wgpu::StencilFaceState { compare: wgpu::CompareFunction::Always, fail_op: Keep, depth_fail_op: Keep, pass_op };
                let mark = wgpu::DepthStencilState {
                    format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState { front: face(Zero), back: face(Replace), read_mask: 0xff, write_mask: 0xff },
                    bias: Default::default(),
                };
                // Then the cap's back faces again, only there, at the section plane's depth.
                let only = wgpu::StencilFaceState { compare: wgpu::CompareFunction::Equal, ..face(Keep) };
                let cap = wgpu::DepthStencilState {
                    format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState { front: only, back: only, read_mask: 0xff, write_mask: 0 },
                    bias: Default::default(),
                };
                (
                    solid("sc_tris_mark", "fs_solid", None, Some(mark), planned),
                    solid("sc_tris_cap", "fs_solid_cut", Some(wgpu::Face::Front), Some(cap), planned),
                )
            })
        };
        let access_bind = access_texture(device, None, &access_bgl, 1, &[]);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sc_image"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let ss_module =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sc_ssaa"), source: wgpu::ShaderSource::Wgsl(SS_SHADER.into()) });
        let ss_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sc_ssaa"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let ss_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc_ssaa"),
            bind_group_layouts: &[Some(&ss_bgl)],
            immediate_size: 0,
        });
        // The frame is premultiplied (drawn over transparent black), so it goes over the
        // viewport's background premultiplied; the viewport's depth is left as it is.
        let ss_blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sc_ssaa"),
            layout: Some(&ss_layout),
            vertex: wgpu::VertexState { module: &ss_module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: depth(wgpu::CompareFunction::Always, false),
            multisample: wgpu::MultisampleState { count: t.samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &ss_module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: t.format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        Resources {
            ao,
            target: t,
            max_texture: device.limits().max_texture_dimension_2d,
            ss_blit,
            ss_bgl,
            ss: None,
            ss_ready: false,
            image,
            image_bgl,
            sampler,
            textures: Default::default(),
            quads: Vec::new(),
            tri: solid("sc_tris", "fs_solid", None, depth(wgpu::CompareFunction::Less, true), false),
            tri_cut: cut(false),
            tri_planned: solid("sc_tris_planned", "fs_solid", None, depth(wgpu::CompareFunction::Less, true), true),
            tri_cut_planned: cut(true),
            tess,
            access_bgl,
            access_bind,
            access_version: 0,
            tri_hl: pipeline("sc_tris_hl", "vs_tri", "fs_tri", tri_layout(), highlight_depth(t.depth), alpha),
            pbr: pipeline("sc_pbr", "vs_pbr", "fs_pbr", pbr_layout(), depth(wgpu::CompareFunction::Less, true), None),
            trans: pipeline("sc_trans", "vs_tri", "fs_tri", tri_layout(), depth(wgpu::CompareFunction::LessEqual, false), alpha),
            glass_far: glass("sc_glass_far", wgpu::Face::Front),
            glass_near: glass("sc_glass_near", wgpu::Face::Back),
            ghost: pipeline("sc_ghost", "vs_ghost", "fs_tri", tri_layout(), depth(wgpu::CompareFunction::LessEqual, false), alpha),
            xray: pipeline("sc_xray", "vs_tri", "fs_tri", tri_layout(), depth(wgpu::CompareFunction::Always, false), alpha),
            line: pipeline("sc_lines", "vs_line", "fs_line", line_layout(), depth(wgpu::CompareFunction::LessEqual, false), alpha),
            line_hidden: pipeline("sc_lines_hidden", "vs_line", "fs_line", line_layout(), depth(wgpu::CompareFunction::Greater, false), alpha),
            overlay: pipeline("sc_overlay", "vs_line", "fs_line_all", line_layout(), depth(wgpu::CompareFunction::Always, false), alpha),
            guide_trans: pipeline("sc_guide_trans", "vs_tri", "fs_tri_all", tri_layout(), depth(wgpu::CompareFunction::LessEqual, false), alpha),
            guide_line: pipeline("sc_guide_lines", "vs_line", "fs_line_all", line_layout(), depth(wgpu::CompareFunction::LessEqual, false), alpha),
            uniform,
            bind,
            linear_out: t.format.is_srgb(),
            model: Batches::default(),
            product: Batches::default(),
            empty: Batches::default(),
            sketch: Batches::default(),
            highlight: Batches::default(),
            preview: Batches::default(),
        }
    }
}

/// Depth state of faces lit over the model (hover, selection): tested, not written, and pulled a
/// little toward the eye. They lie on the model's own faces, but the GPU evaluates a body's plan
/// in f32 while the highlight's copy of the same triangles is evaluated on the CPU, so the two
/// depths differ by rounding and an unbiased equal test lights the face in speckles and wedges.
/// The bias is a few depth steps plus the face's own slope: far less than any real gap.
fn highlight_depth(format: Option<wgpu::TextureFormat>) -> Option<wgpu::DepthStencilState> {
    format.map(|format| wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: Default::default(),
        bias: HIGHLIGHT_BIAS,
    })
}

/// See [`highlight_depth`]. Negative: toward the eye (depth grows away from it).
const HIGHLIGHT_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState { constant: -8, slope_scale: -1.0, clamp: 0.0 };

fn upload(device: &wgpu::Device, label: &str, bytes: &[u8], stride: usize) -> Option<Batch> {
    upload_with(device, label, bytes, stride, wgpu::BufferUsages::VERTEX)
}

fn upload_with(device: &wgpu::Device, label: &str, bytes: &[u8], stride: usize, usage: wgpu::BufferUsages) -> Option<Batch> {
    if bytes.len() < stride || stride == 0 {
        return None;
    }
    let chunk = buffer_chunk(stride, device.limits().max_buffer_size);
    let parts = bytes
        .chunks(chunk)
        .filter(|part| part.len() >= stride)
        .map(|part| {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: part, usage });
            (buffer, u32::try_from(part.len() / stride).unwrap_or(0))
        })
        .collect();
    Some(Batch { parts, chunk })
}

/// Bytes per GPU buffer for vertices of `stride` bytes: within the device's limit and a
/// whole number of triangles and line pairs, so no primitive straddles two buffers.
fn buffer_chunk(stride: usize, max_buffer_size: u64) -> usize {
    let unit = stride.max(1) * 6;
    let max = usize::try_from(max_buffer_size).unwrap_or(usize::MAX).min(1 << 30);
    (max / unit).max(1) * unit
}

/// Triangles (3 vertices of `TRI_SIZE` bytes each, position first) ordered far to near along
/// the view direction `back` (toward the eye), so blended glass draws back to front.
pub fn sort_back_to_front(bytes: &[u8], back: [f32; 3]) -> Vec<u8> {
    let tri = TRI_SIZE * 3;
    let depth = |t: &[u8]| -> f32 {
        let f = |o: usize| t.get(o..o + 4).and_then(|b| b.try_into().ok()).map_or(0.0, f32::from_le_bytes);
        (0..3).map(|v| (0..3).map(|k| f(v * TRI_SIZE + k * 4) * back[k]).sum::<f32>()).sum::<f32>()
    };
    let mut tris: Vec<(f32, &[u8])> = bytes.chunks_exact(tri).map(|t| (depth(t), t)).collect();
    tris.sort_by(|a, b| a.0.total_cmp(&b.0));
    tris.into_iter().flat_map(|(_, t)| t.iter().copied()).collect()
}

impl Batches {
    /// Re-sort the glass for a new view direction (when it turned more than about 2 degrees).
    fn sort_glass(&mut self, queue: &wgpu::Queue, back: [f32; 3]) {
        let (Some(g), false) = (&self.glass, self.glass_cpu.is_empty()) else { return };
        let turned = self.glass_sorted_for.is_none_or(|s| s[0] * back[0] + s[1] * back[1] + s[2] * back[2] < 0.9994);
        if turned {
            let sorted = sort_back_to_front(&self.glass_cpu, back);
            for ((buffer, _), bytes) in g.parts.iter().zip(sorted.chunks(g.chunk.max(1))) {
                queue.write_buffer(buffer, 0, bytes);
            }
            self.glass_sorted_for = Some(back);
        }
    }
}

/// The tessellation pipeline (`shader`: its source), where display plans are evaluated: on a
/// device that runs compute shaders with the storage buffers the evaluation binds (not WebGL2)
/// and builds the pipeline. Elsewhere `None`, and bodies are built on the CPU: a device whose
/// shader compiler rejects the shader (D3D12's FXC once did) only loses GPU evaluation.
fn tessellation(adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, shader: &str) -> Option<solidcraft_gpu::TessGpu> {
    let compute = adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
        && device.limits().max_storage_buffers_per_shader_stage >= 4;
    if !compute {
        return None;
    }
    let info = adapter.get_info();
    if hangs_on_gpu_tessellation(info.backend, &info.name) || std::env::var_os("SOLIDCRAFT_CPU_TESS").is_some() {
        log::warn!("bodies are built on the CPU on {} ({:?})", info.name, info.backend);
        return None;
    }
    match solidcraft_gpu::TessGpu::with_shader(info.name.clone(), device.clone(), queue.clone(), shader) {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("bodies are built on the CPU on {} ({:?}): {e}", info.name, info.backend);
            None
        }
    }
}

/// A device whose driver hangs on the evaluation shader, so bodies are built on the CPU there:
/// the AMD FirePro D300/D500/D700 (Mac Pro 2013, Tahiti) under Metal, where macOS resets the GPU
/// and the whole desktop freezes. `SOLIDCRAFT_CPU_TESS=1` forces CPU evaluation on any device.
fn hangs_on_gpu_tessellation(backend: wgpu::Backend, name: &str) -> bool {
    backend == wgpu::Backend::Metal && name.contains("FirePro")
}

/// Create the pipelines. Call once with the app's render state and its depth/stencil/MSAA
/// settings. Without a stencil the section cap does not hide the inside of cut bodies. Without
/// `gpu_meshing` (safe mode) bodies are meshed on the CPU.
pub fn install(rs: &egui_wgpu::RenderState, depth_bits: u8, stencil_bits: u8, samples: u32, gpu_meshing: bool) -> GpuTarget {
    let tess = if gpu_meshing { tessellation(&rs.adapter, &rs.device, &rs.queue, solidcraft_gpu::SHADER) } else { None };
    let t = GpuTarget { format: rs.target_format, depth: egui_wgpu::depth_format_from_bits(depth_bits, stencil_bits), samples, tess: tess.is_some() };
    let res = Resources::new(&rs.device, t, tess);
    rs.renderer.write().callback_resources.insert(res);
    t
}

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<Resources>() else { return Vec::new() };
        if self.product {
            res.product.take(device, self.key, &self.slot, None);
            res.product.sort_glass(queue, self.back);
            queue.write_buffer(&res.uniform, 0, &uniform_bytes(self, self.size_px[0], self.size_px[1], res.linear_out));
            return Vec::new();
        }
        res.model.take(device, self.key, &self.slot, res.tess.as_ref().map(|t| (t, &mut *encoder)));
        res.sketch.take(device, self.sketch_key, &self.sketch_slot, None);
        res.highlight.take(device, self.hl_key, &self.hl_slot, None);
        res.preview.take(device, self.pv_key, &self.pv_slot, None);
        res.model.sort_glass(queue, self.back);
        queue.write_buffer(&res.uniform, 0, &uniform_bytes(self, self.size_px[0], self.size_px[1], res.linear_out));
        if let Some(a) = &self.access
            && a.version != res.access_version
        {
            res.access_bind = access_texture(device, Some(queue), &res.access_bgl, a.size, &a.depth);
            res.access_version = a.version;
        }
        res.ao.prepare(device, encoder, &res.bind, self.ao_radius, self.size_px, [&res.model, &res.preview]);
        // Images: upload new or changed textures, drop removed ones, rebuild the quads.
        res.textures.retain(|id, _| self.images.iter().any(|i| i.id == *id));
        res.quads.clear();
        for img in &self.images {
            let [w, h] = img.size;
            if w == 0 || h == 0 || img.rgba.len() != (w as usize) * (h as usize) * 4 {
                continue;
            }
            if res.textures.get(&img.id).is_none_or(|t| t.0 != img.version) {
                let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("sc_image"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    &img.rgba,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                    size,
                );
                let view = tex.create_view(&Default::default());
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("sc_image"),
                    layout: &res.image_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&res.sampler) },
                    ],
                });
                res.textures.insert(img.id, (img.version, bind));
            }
            let c = img.corners;
            let uv = [[0.0_f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
            let mut bytes = Vec::with_capacity(6 * IMG_SIZE);
            for k in [0, 1, 2, 0, 2, 3] {
                for v in c[k].iter().chain(&uv[k]).chain(std::iter::once(&img.opacity)) {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
            }
            if let Some(b) = upload(device, "sc_image_quad", &bytes, IMG_SIZE) {
                res.quads.push((img.id, b));
            }
        }
        res.ss_ready = self.ssaa > 1.0 && res.supersample(device, encoder, self);
        Vec::new()
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(res) = resources.get::<Resources>() else { return };
        let vp = info.viewport_in_pixels();
        if vp.width_px <= 0 || vp.height_px <= 0 {
            return;
        }
        pass.set_viewport(vp.left_px as f32, vp.top_px as f32, vp.width_px as f32, vp.height_px as f32, 0.0, 1.0);
        // Drawn supersampled in `prepare`: filter it down into the viewport.
        if res.ss_ready
            && let Some(ss) = &res.ss
        {
            pass.set_pipeline(&res.ss_blit);
            pass.set_bind_group(0, &ss.bind, &[]);
            pass.draw(0..3, 0..1);
            return;
        }
        res.draw(self, pass);
    }
}

impl Resources {
    /// Draw the frame's scenes into `pass` (its viewport already set).
    fn draw(&self, cb: &ViewportCallback, pass: &mut wgpu::RenderPass<'static>) {
        let res = self;
        pass.set_bind_group(0, &res.bind, &[]);
        let tris = |pass: &mut wgpu::RenderPass<'static>, p: &wgpu::RenderPipeline, b: &Option<Batch>| {
            if let Some(b) = b {
                pass.set_pipeline(p);
                for (buffer, count) in &b.parts {
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
        };
        let lines = |pass: &mut wgpu::RenderPass<'static>, p: &wgpu::RenderPipeline, b: &Option<Batch>| {
            if let Some(b) = b {
                pass.set_pipeline(p);
                for (buffer, count) in &b.parts {
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..6, 0..*count);
                }
            }
        };
        let (m, sk, h, pv) =
            if cb.product { (&res.product, &res.empty, &res.empty, &res.empty) } else { (&res.model, &res.sketch, &res.highlight, &res.preview) };
        lines(pass, &res.overlay, &m.under);
        lines(pass, &res.overlay, &sk.under);
        pass.set_bind_group(1, &res.access_bind, &[]);
        pass.set_bind_group(2, &res.ao.bind, &[]);
        let planned = |pass: &mut wgpu::RenderPass<'static>, p: &wgpu::RenderPipeline| {
            for d in &m.planned {
                pass.set_pipeline(p);
                pass.set_vertex_buffer(0, d.mesh.vertices.slice(..));
                pass.set_vertex_buffer(1, d.colors.slice(..));
                pass.set_index_buffer(d.mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..d.mesh.index_count, 0, 0..1);
            }
        };
        let sectioned = cb.clip[..3].iter().map(|c| c * c).sum::<f32>() > 0.25;
        match (res.tri_cut.as_ref().filter(|_| sectioned), res.tri_cut_planned.as_ref()) {
            (Some((mark, cap)), Some((mark_planned, cap_planned))) => {
                pass.set_stencil_reference(1);
                tris(pass, mark, &m.tris);
                tris(pass, mark, &pv.tris);
                planned(pass, mark_planned);
                tris(pass, cap, &m.tris);
                tris(pass, cap, &pv.tris);
                planned(pass, cap_planned);
            }
            _ => {
                tris(pass, &res.tri, &m.tris);
                tris(pass, &res.tri, &pv.tris);
                planned(pass, &res.tri_planned);
            }
        }
        tris(pass, &res.pbr, &m.pbr);
        tris(pass, &res.tri_hl, &h.tris);
        lines(pass, &res.line, &m.lines);
        lines(pass, &res.line, &sk.lines);
        lines(pass, &res.line_hidden, &m.hidden);
        lines(pass, &res.line_hidden, &h.hidden);
        lines(pass, &res.line, &pv.lines);
        lines(pass, &res.line, &h.lines);
        lines(pass, &res.guide_line, &h.guide_lines);
        for (id, q) in res.quads.iter().filter(|_| !cb.product) {
            if let Some((_, bind)) = res.textures.get(id) {
                pass.set_pipeline(&res.image);
                pass.set_bind_group(1, bind, &[]);
                for (buffer, count) in &q.parts {
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
        }
        tris(pass, &res.ghost, &pv.ghost);
        tris(pass, &res.glass_far, &m.glass);
        tris(pass, &res.glass_near, &m.glass);
        tris(pass, &res.trans, &m.trans);
        tris(pass, &res.trans, &sk.trans);
        tris(pass, &res.trans, &h.trans);
        tris(pass, &res.guide_trans, &h.guide_trans);
        tris(pass, &res.xray, &pv.xray);
        lines(pass, &res.overlay, &m.overlays);
        lines(pass, &res.overlay, &sk.overlays);
        lines(pass, &res.overlay, &h.overlays);
    }
}

impl Resources {
    /// Draw the frame supersampled (`cb.ssaa` times the viewport's pixels each way, within the
    /// device's texture limit and the memory budget) into textures of the viewport's kind.
    /// `false` when it cannot be (too small a gain, a zero-sized viewport): drawn as usual.
    fn supersample(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, cb: &ViewportCallback) -> bool {
        let Some(size) = supersampled_size(cb.size_px, cb.ssaa, self.max_texture, self.target.samples) else {
            self.ss = None;
            return false;
        };
        if self.ss.as_ref().is_none_or(|ss| ss.size != size) {
            self.ss = Some(self.supersampled(device, size));
        }
        let Some(ss) = &self.ss else { return false };
        let depth_format = self.target.depth;
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sc_ssaa"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &ss.color,
                    depth_slice: None,
                    resolve_target: ss.resolve.as_ref(),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: ss.depth.as_ref().map(|view| wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: depth_format
                        .filter(|f| f.has_stencil_aspect())
                        .map(|_| wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        pass.set_viewport(0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0);
        self.draw(cb, &mut pass);
        true
    }

    /// Textures for a supersampled frame of `size` pixels.
    fn supersampled(&self, device: &wgpu::Device, size: [u32; 2]) -> Supersampled {
        let t = self.target;
        let texture = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let samples = t.samples.max(1);
        let sampled = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let (color, resolve) = if samples > 1 {
            (texture("sc_ssaa_ms", t.format, samples, wgpu::TextureUsages::RENDER_ATTACHMENT), Some(texture("sc_ssaa", t.format, 1, sampled)))
        } else {
            (texture("sc_ssaa", t.format, 1, sampled), None)
        };
        let depth = t.depth.map(|f| texture("sc_ssaa_depth", f, samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sc_ssaa"),
            layout: &self.ss_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(resolve.as_ref().unwrap_or(&color)) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        Supersampled { size, color, resolve, depth, bind }
    }
}

/// A device of its own for an offscreen render or check (`None` without a GPU adapter).
#[cfg(all(test, not(target_arch = "wasm32")))]
fn offscreen_device() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .ok()?;
    Some((adapter, device, queue))
}

/// Whether [`install`] would evaluate plans on the GPU with `shader`, on an offscreen device
/// (`None` without a GPU adapter).
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn offscreen_tessellates(shader: &str) -> Option<bool> {
    let (adapter, device, queue) = offscreen_device()?;
    Some(tessellation(&adapter, &device, &queue, shader).is_some())
}

/// Render `scene` offscreen on a device of its own, through the viewport's real resources and
/// frame callback (`prepare`, then `paint`), and read the pixels back: straight RGBA, `size` ×
/// `size`. `None` without a GPU adapter. With `tess` (the tessellation shader), planned bodies
/// are evaluated on the GPU where the device builds that pipeline, as [`install`] sets it up.
/// `highlight` is drawn as the viewport's highlight scene (hover and selection). `analysis` is the
/// surface analysis uniform (`viewport::analysis_uniform`; zeros for none) and `colors` its
/// colours (`analysis_colors`).
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_offscreen(
    scene: GpuScene,
    highlight: Option<GpuScene>,
    view_proj: [[f32; 4]; 4],
    back: [f32; 3],
    eye: [f32; 4],
    size: u32,
    tess: Option<&str>,
    analysis: [f32; 8],
    colors: [f32; 36],
) -> Option<Vec<u8>> {
    offscreen(
        scene,
        highlight,
        view_proj,
        back,
        eye,
        size,
        Offscreen { tess, product: false, srgb: false, analysis, analysis_colors: colors, ssaa: 1.0 },
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn render_product_offscreen(
    scene: GpuScene,
    view_proj: [[f32; 4]; 4],
    back: [f32; 3],
    eye: [f32; 4],
    size: u32,
    srgb: bool,
) -> Option<Vec<u8>> {
    offscreen(
        scene,
        None,
        view_proj,
        back,
        eye,
        size,
        Offscreen { tess: None, product: true, srgb, analysis: [0.0; 8], analysis_colors: [0.0; 36], ssaa: 1.0 },
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
struct Offscreen<'a> {
    tess: Option<&'a str>,
    product: bool,
    srgb: bool,
    analysis: [f32; 8],
    analysis_colors: [f32; 36],
    ssaa: f32,
}

/// [`render_offscreen`] of a scene supersampled `ssaa` times (1: off).
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn render_offscreen_ssaa(
    scene: GpuScene,
    view_proj: [[f32; 4]; 4],
    back: [f32; 3],
    eye: [f32; 4],
    size: u32,
    ssaa: f32,
) -> Option<Vec<u8>> {
    offscreen(
        scene,
        None,
        view_proj,
        back,
        eye,
        size,
        Offscreen { tess: None, product: false, srgb: false, analysis: [0.0; 8], analysis_colors: [0.0; 36], ssaa },
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
fn offscreen(
    scene: GpuScene,
    highlight: Option<GpuScene>,
    view_proj: [[f32; 4]; 4],
    back: [f32; 3],
    eye: [f32; 4],
    size: u32,
    options: Offscreen<'_>,
) -> Option<Vec<u8>> {
    let Offscreen { tess, product, srgb, analysis, analysis_colors, ssaa } = options;
    let (adapter, device, queue) = offscreen_device()?;
    eprintln!("offscreen adapter: {:?}", adapter.get_info().backend);
    let format = if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
    let depth = wgpu::TextureFormat::Depth24PlusStencil8;
    let compute = tess.and_then(|shader| tessellation(&adapter, &device, &queue, shader));
    let target = GpuTarget { format, depth: Some(depth), samples: 1, tess: compute.is_some() };
    let mut resources = egui_wgpu::CallbackResources::default();
    resources.insert(Resources::new(&device, target, compute));
    let none = || Arc::new(Mutex::new(None));
    let cb = ViewportCallback {
        product,
        key: 1,
        slot: Arc::new(Mutex::new(Some(scene))),
        sketch_key: 0,
        sketch_slot: none(),
        hl_key: u64::from(highlight.is_some()),
        hl_slot: Arc::new(Mutex::new(highlight)),
        pv_key: 0,
        pv_slot: none(),
        view_proj,
        back,
        size_px: [size as f32; 2],
        clip: [0.0; 4],
        cap: [0.0; 4],
        images: Vec::new(),
        analysis,
        analysis_colors,
        access: None,
        outline: false,
        ao_radius: 0.0,
        eye,
        ssaa,
    };
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = texture(format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth_tex = texture(depth, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let (color_view, depth_view) = (color.create_view(&Default::default()), depth_tex.create_view(&Default::default()));
    let mut encoder = device.create_command_encoder(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [size, size], pixels_per_point: 1.0 };
    let extra = egui_wgpu::CallbackTrait::prepare(&cb, &device, &queue, &screen, &mut encoder, &mut resources);
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("offscreen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size as f32, size as f32));
        let info = egui::PaintCallbackInfo { viewport: rect, clip_rect: rect, pixels_per_point: 1.0, screen_size_px: [size, size] };
        egui_wgpu::CallbackTrait::paint(&cb, info, &mut pass, &resources);
    }
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offscreen read"),
        size: u64::from(size * size * 4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &read,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: Some(size) },
        },
        wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
    );
    queue.submit(extra.into_iter().chain([encoder.finish()]));
    let slice = read.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let bytes = slice.get_mapped_range().ok()?.to_vec();
    Some(bytes)
}

/// Uniform bytes for a frame.
pub fn uniform_bytes(cb: &ViewportCallback, w: f32, h: f32, linear: bool) -> Vec<u8> {
    let mut v: Vec<f32> = cb.view_proj.iter().flatten().copied().collect();
    v.extend([cb.back[0], cb.back[1], cb.back[2], if linear { 1.0 } else { 0.0 }]);
    v.extend([w.max(1.0), h.max(1.0), 2e-4, if cb.outline { 1.0 } else { 0.0 }]);
    v.extend(cb.clip);
    v.extend(cb.cap);
    v.extend(cb.analysis);
    match &cb.access {
        Some(a) => {
            v.extend(a.u);
            v.extend(a.v);
        }
        None => v.extend([0.0; 8]),
    }
    v.extend(cb.eye);
    v.extend([if cb.ao_radius.is_finite() { cb.ao_radius.max(0.0) } else { 0.0 }, 0.0, 0.0, 0.0]);
    v.extend(cb.analysis_colors);
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// The shader's analysis colours (`ana_c`: draft positive, between, negative; reachable,
/// blocked; curvature flat, middle, tight; alpha 1) and flags (`ana_f.x`: 1 to stripe the bad
/// faces).
pub fn analysis_colors(c: &crate::theme::AnalysisColors) -> [f32; 36] {
    let mut v = [0.0; 36];
    let colors = c.draft.iter().chain(&c.access).chain(&c.curvature).flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 1.0]);
    for (slot, x) in v.iter_mut().zip(colors) {
        *slot = x;
    }
    v[32] = if c.hatch { 1.0 } else { 0.0 };
    v
}

#[cfg(test)]
mod tests {
    use egui_wgpu::wgpu::{self, naga};

    /// Only the FirePro under Metal skips GPU evaluation; the same card under another backend
    /// and other Metal GPUs keep it.
    #[test]
    fn firepro_under_metal_builds_bodies_on_the_cpu() {
        assert!(super::hangs_on_gpu_tessellation(wgpu::Backend::Metal, "AMD FirePro D500"));
        assert!(super::hangs_on_gpu_tessellation(wgpu::Backend::Metal, "AMD FirePro D700"));
        assert!(!super::hangs_on_gpu_tessellation(wgpu::Backend::Metal, "Apple M2"));
        assert!(!super::hangs_on_gpu_tessellation(wgpu::Backend::Metal, "AMD Radeon Pro 560X"));
        assert!(!super::hangs_on_gpu_tessellation(wgpu::Backend::Vulkan, "AMD FirePro W5100"));
    }

    /// The viewport shader parses and validates (a bad shader would only fail at run time).
    #[test]
    fn shader_is_valid() {
        let module = naga::front::wgsl::parse_str(super::SHADER).unwrap_or_else(|e| panic!("{}", e.emit_to_string(super::SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap();
    }

    /// Derivatives and implicit-lod sampling run only in uniform control flow: browsers' WGSL
    /// compilers (Tint) reject them after a branch, and the whole viewport shader then fails,
    /// a black screen on the web (naga accepts it). Every fragment entry point makes such calls
    /// (directly or through a helper) before its first branch or return.
    #[test]
    fn derivatives_only_in_uniform_control_flow() {
        // Without comments (their words are not code).
        let src: String = super::SHADER.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
        let src = src.as_str();
        let derivs = ["dpdx(", "dpdy(", "fwidth(", "textureSample("];
        // Functions: name -> body.
        let mut fns: Vec<(String, String, bool)> = Vec::new();
        let mut rest = src;
        let mut fragment = false;
        while let Some(at) = rest.find("fn ") {
            let before = &rest[..at];
            if before.contains("@fragment") {
                fragment = true;
            }
            let Some(open) = rest[at..].find('{') else { break };
            let name = rest[at + 3..].split('(').next().unwrap_or("").trim().to_string();
            let start = at + open;
            let mut depth = 0;
            let mut end = start;
            for (k, ch) in rest[start..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = start + k;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            fns.push((name, rest[start..=end].to_string(), fragment));
            fragment = false;
            rest = &rest[end + 1..];
        }
        // Helpers that take derivatives (directly or through another helper).
        let mut uses: Vec<String> = Vec::new();
        for _ in 0..8 {
            for (n, body, _) in &fns {
                if !uses.contains(n) && (derivs.iter().any(|d| body.contains(d)) || uses.iter().any(|u| body.contains(&format!("{u}(")))) {
                    uses.push(n.clone());
                }
            }
        }
        for (n, body, frag) in &fns {
            let calls: Vec<usize> = derivs
                .iter()
                .map(|d| d.to_string())
                .chain(uses.iter().map(|u| format!("{u}(")))
                .flat_map(|c| body.match_indices(&c).map(|(i, _)| i).collect::<Vec<_>>())
                .collect();
            if calls.is_empty() {
                continue;
            }
            if !*frag {
                // A helper: its own derivatives must come before any branch or return in it.
                let first = ["if (", "return", "discard"].iter().filter_map(|k| body.find(k)).min().unwrap_or(usize::MAX);
                for d in derivs {
                    if let Some(i) = body.find(d) {
                        assert!(i < first, "{n}: {d} after a branch");
                    }
                }
                continue;
            }
            let first = ["if (", "return", "discard"].iter().filter_map(|k| body.find(k)).min().unwrap_or(usize::MAX);
            for c in calls {
                assert!(c < first, "fragment {n}: a derivative after a branch or return: {}", &body[c..(c + 40).min(body.len())]);
            }
        }
    }
}

#[cfg(test)]
mod glass_tests {
    use super::*;

    /// A large import's triangles once went into one buffer past the device's limit (524 MB
    /// against 256 MB) and the app crashed (#883): they are split into whole triangles.
    #[test]
    fn large_scenes_are_split_into_buffers_within_the_device_limit() {
        let limit = 256u64 << 20;
        for stride in [TRI_SIZE, LINE_SIZE, 4] {
            let chunk = buffer_chunk(stride, limit);
            assert!(chunk as u64 <= limit && chunk > limit as usize / 2);
            assert_eq!(chunk % (stride * 3), 0, "whole triangles");
            assert_eq!(chunk % (stride * 2), 0, "whole line pairs");
        }
        let scene = 524_434_680usize;
        assert_eq!(scene.div_ceil(buffer_chunk(TRI_SIZE, limit)), 2);
        assert!(buffer_chunk(TRI_SIZE, 1) >= TRI_SIZE * 6, "a tiny limit still holds one unit");
    }

    #[test]
    fn glass_is_drawn_far_to_near() {
        // Two triangles at z = 5 and z = -5, given near first; looking down from +Z.
        let mut sc = GpuScene::default();
        for z in [5.0f32, -5.0] {
            for p in [[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z]] {
                sc.glass_tri(p, [0.0, 0.0, 1.0], [255, 0, 0, 100]);
            }
        }
        let sorted = sort_back_to_front(&sc.glass, [0.0, 0.0, 1.0]);
        let z0 = f32::from_le_bytes(sorted[8..12].try_into().unwrap());
        assert_eq!(z0, -5.0, "the far triangle first");
        assert_eq!(sorted.len(), sc.glass.len());
        // Seen from below, the order flips.
        let sorted = sort_back_to_front(&sc.glass, [0.0, 0.0, -1.0]);
        assert_eq!(f32::from_le_bytes(sorted[8..12].try_into().unwrap()), 5.0);
    }
}
