//! Pictures of a [`Scene`] drawn offscreen with the viewport's pipelines ([`crate::view`]): the
//! same shading, edges, silhouettes, section cap and surface analysis the viewport shows, read
//! back as RGBA. `solidcraft-cli snapshot`, MCP screenshots, `ui.render` and thumbnails draw
//! through it, on the application's device ([`Capturer::new`]) or on a headless one
//! ([`Capturer::headless`], [`shared`]). Without a qualifying adapter, the `auto` policy falls
//! to the last tier, the CPU rasterizer ([`headless_rgba`]), and says so: every picture names the
//! renderer that drew it (`gpu`, `gpu-software` or `cpu`); `hardware` and `software` fail with an
//! error that names the policy.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use solidcraft_geom::Vec3;
use solidcraft_render::{AnalysisMode, Camera, Rgb, SECTION_CAP, Scene};
use wgpu::util::DeviceExt;

use crate::view::{AccessMap, Frame, GpuScene, GpuTarget, Renderer, analysis_colors, analysis_uniform};
use crate::{GpuError, Result, popped};

/// The colour format pictures are drawn in: sRGB bytes as they are, like the viewport's usual
/// (non-sRGB) target.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Samples per pixel (MSAA), as the desktop viewport's default; 1 where the device has no 4×.
pub const SAMPLES: u32 = 4;
/// The largest picture side (pixels); the device's texture limit may lower it.
pub const MAX_SIDE: u32 = 8192;
/// The background of a scene without one of its own: the light theme's viewport gradient.
const BACKGROUND: (Rgb, Rgb) = (Rgb(236, 240, 245), Rgb(205, 212, 222));
/// How long a picture may take on the GPU before it is given up.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The viewport's pipelines on one device, drawing pictures offscreen. Pictures are drawn one at
/// a time (callers on several threads take turns).
pub struct Capturer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Shared with surface configuration when this capturer uses a window's device.
    queue_access: Option<Arc<Mutex<()>>>,
    adapter: String,
    /// The adapter is a GPU driver running on the CPU (llvmpipe, lavapipe, WARP).
    software: bool,
    samples: u32,
    /// The depth format: 32-bit float (reversed depth) where the device has it ([`crate::depth_format`]).
    depth: wgpu::TextureFormat,
    max_side: u32,
    state: Mutex<State>,
}

struct State {
    renderer: Renderer,
    background: wgpu::RenderPipeline,
    /// The last picture's scene key and accessibility map version (each picture uploads anew).
    key: u64,
    access_version: u64,
}

impl Capturer {
    /// The pipelines on an existing device (the application's).
    pub fn new(adapter: &wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> Result<Self> {
        let depth = crate::depth_format(&device, true);
        Self::with_depth(adapter, device, queue, depth)
    }

    /// As [`Capturer::new`], drawing with the depth format `depth` (with a stencil): the fallback
    /// `Depth24PlusStencil8` on a device that has a float one too, to compare the two.
    pub fn with_depth(adapter: &wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue, depth: wgpu::TextureFormat) -> Result<Self> {
        let flags = |f: wgpu::TextureFormat| adapter.get_texture_format_features(f).flags;
        let samples = if flags(FORMAT).sample_count_supported(SAMPLES) && flags(depth).sample_count_supported(SAMPLES) { SAMPLES } else { 1 };
        let scopes = error_scopes(&device);
        let renderer = Renderer::new(&device, GpuTarget { format: FORMAT, depth: Some(depth), samples, tess: false }, None);
        let background = background_pipeline(&device, samples, depth);
        caught(scopes).map_or(Ok(()), |e| Err(GpuError::Pipeline(e)))?;
        let max_side = device.limits().max_texture_dimension_2d.min(MAX_SIDE);
        Ok(Capturer {
            adapter: adapter.get_info().name,
            software: adapter.get_info().device_type == wgpu::DeviceType::Cpu,
            samples,
            depth,
            max_side,
            state: Mutex::new(State { renderer, background, key: 0, access_version: 0 }),
            device,
            queue,
            queue_access: None,
        })
    }

    /// Serialize submissions with surface configuration on a shared window device.
    /// Only submission holds this gate; encoding and readback never block window resize.
    pub fn with_queue_access(mut self, access: Arc<Mutex<()>>) -> Self {
        self.queue_access = Some(access);
        self
    }

    /// A device of its own, without a window (native only): the adapter the `SOLIDCRAFT_GPU`
    /// policy picks ([`crate::select`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless() -> Result<Self> {
        let (adapter, device, queue) = crate::open_headless()?;
        // Errors are caught per picture (error scopes); anything else is logged, never a panic.
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| log::error!("GPU: {e}")));
        Self::new(&adapter, device, queue)
    }

    /// As [`Capturer::headless`], drawing with the depth format `depth` ([`Capturer::with_depth`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless_with_depth(depth: wgpu::TextureFormat) -> Result<Self> {
        let (adapter, device, queue) = crate::open_headless()?;
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| log::error!("GPU: {e}")));
        Self::with_depth(&adapter, device, queue, depth)
    }

    /// The depth format pictures are drawn with; float formats are drawn with reversed depth.
    pub fn depth_format(&self) -> wgpu::TextureFormat {
        self.depth
    }

    /// The adapter's name.
    pub fn adapter_name(&self) -> &str {
        &self.adapter
    }

    /// What a picture's reply names as its renderer: `gpu`, or `gpu-software` on a software
    /// adapter.
    pub fn renderer(&self) -> &'static str {
        if self.software { "gpu-software" } else { "gpu" }
    }

    /// Samples per pixel the pictures are drawn with.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// `scene` seen by `cam`, `width` × `height` pixels: straight RGBA, rows top to bottom.
    pub fn rgba(&self, scene: &Scene, cam: &Camera, width: u32, height: u32) -> Result<Vec<u8>> {
        if !(1..=self.max_side).contains(&width) || !(1..=self.max_side).contains(&height) {
            return Err(GpuError::Render(format!("a picture is 1 to {} pixels a side, not {width} × {height}", self.max_side)));
        }
        let mut state = self.state.lock().map_err(|_| GpuError::Render("an earlier picture failed on this device".into()))?;
        let state = &mut *state;
        state.key = state.key.wrapping_add(1);
        let mode = scene.analysis.map(|a| a.mode);
        let access = match mode {
            Some(AnalysisMode::Access { dir }) => {
                state.access_version = state.access_version.wrapping_add(1);
                AccessMap::build(&scene.meshes.iter().map(|m| m.mesh.as_ref()).collect::<Vec<_>>(), dir, state.access_version)
            }
            _ => None,
        };
        let frame = frame(scene, cam, [width, height], state.key, access);
        let (device, queue) = (&self.device, &self.queue);
        let scopes = error_scopes(device);
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let resolved = texture("sc_capture", FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let multisampled = (self.samples > 1).then(|| texture("sc_capture_ms", FORMAT, self.samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
        let depth = texture("sc_capture_depth", self.depth, self.samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let (resolved_view, depth_view) = (resolved.create_view(&Default::default()), depth.create_view(&Default::default()));
        let multisampled_view = multisampled.as_ref().map(|t| t.create_view(&Default::default()));
        let (top, bottom) = scene.background.unwrap_or(BACKGROUND);
        let background = background_vertices(device, top, bottom);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sc_capture") });
        state.renderer.prepare(device, queue, &mut encoder, &frame);
        {
            let (view, resolve_target, store) = match &multisampled_view {
                Some(ms) => (ms, Some(&resolved_view), wgpu::StoreOp::Discard),
                None => (&resolved_view, None, wgpu::StoreOp::Store),
            };
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("sc_capture"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(crate::view::far_depth(crate::view::reversed_depth(Some(self.depth)))),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            // The background first, as the viewport paints it before the model.
            pass.set_pipeline(&state.background);
            pass.set_vertex_buffer(0, background.slice(..));
            pass.draw(0..6, 0..1);
            state.renderer.paint(&frame, &mut pass, [0.0, 0.0, width as f32, height as f32]);
        }
        let row = width * 4;
        let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sc_capture_read"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            resolved.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &read,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(height) },
            },
            size,
        );
        let submission = {
            let _access = self
                .queue_access
                .as_ref()
                .map(|access| access.lock().map_err(|_| GpuError::Render("surface configuration failed on this device".into())))
                .transpose()?;
            queue.submit([encoder.finish()])
        };
        if let Some(e) = caught(scopes) {
            return Err(GpuError::Render(e));
        }
        let slice = read.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(TIMEOUT) })
            .map_err(|e| GpuError::Render(format!("waiting for the picture: {e}")))?;
        // Another thread polling this shared device (surface configuration does this)
        // may own the completed mapping callback but not have dispatched it yet. A
        // successful poll is not proof that this channel is ready at this instant.
        #[cfg(not(target_arch = "wasm32"))]
        let mapped = rx.recv_timeout(TIMEOUT).map_err(|e| GpuError::Render(format!("waiting for picture readback: {e}")))?;
        #[cfg(target_arch = "wasm32")]
        let mapped = rx.try_recv().map_err(|_| GpuError::Unsupported("reading a picture back without waiting for the browser".into()))?;
        mapped.map_err(|e| GpuError::Render(format!("reading the picture back: {e}")))?;
        let data = slice.get_mapped_range().map_err(|e| GpuError::Render(format!("reading the picture back: {e}")))?;
        let mut rgba = Vec::with_capacity((row as usize) * (height as usize));
        for r in data.chunks(padded as usize).take(height as usize) {
            rgba.extend_from_slice(r.get(..row as usize).unwrap_or_default());
        }
        if rgba.len() != (row as usize) * (height as usize) {
            return Err(GpuError::Render("the picture read back short".into()));
        }
        Ok(rgba)
    }

    /// [`Capturer::rgba`] as PNG bytes.
    pub fn png(&self, scene: &Scene, cam: &Camera, width: u32, height: u32) -> Result<Vec<u8>> {
        let rgba = self.rgba(scene, cam, width, height)?;
        solidcraft_render::png(width as usize, height as usize, rgba).ok_or_else(|| GpuError::Render("PNG encoding failed".into()))
    }
}

/// The process's headless capturer (native only), opened on first use under the `SOLIDCRAFT_GPU`
/// policy and kept; the error when no adapter qualifies or the device fails (not retried).
#[cfg(not(target_arch = "wasm32"))]
pub fn shared() -> std::result::Result<&'static Capturer, &'static GpuError> {
    static SHARED: std::sync::OnceLock<Result<Capturer>> = std::sync::OnceLock::new();
    SHARED.get_or_init(Capturer::headless).as_ref()
}

/// What draws this process's headless pictures (native only): the shared headless device, or the
/// CPU rasterizer where the `SOLIDCRAFT_GPU` policy lands there (`cpu`, or `auto` with no
/// qualifying adapter).
#[cfg(not(target_arch = "wasm32"))]
pub enum Headless {
    Gpu(&'static Capturer),
    Cpu,
}

/// [`Headless`] under the `SOLIDCRAFT_GPU` policy; an error when the policy is `hardware` or
/// `software` and no adapter of that tier opens (or the variable names no policy).
#[cfg(not(target_arch = "wasm32"))]
pub fn headless() -> std::result::Result<Headless, String> {
    use crate::select::Policy;
    let policy = Policy::from_env()?;
    if policy == Policy::Cpu {
        return Ok(Headless::Cpu);
    }
    match shared() {
        Ok(c) => Ok(Headless::Gpu(c)),
        Err(e) if policy == Policy::Auto => {
            log::warn!("pictures are drawn by the CPU rasterizer: {e}");
            Ok(Headless::Cpu)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// `scene` drawn by [`headless`]: straight RGBA and the renderer that drew it (`gpu`,
/// `gpu-software` or `cpu`). The CPU rasterizer draws no surface analysis, highlights or legends.
#[cfg(not(target_arch = "wasm32"))]
pub fn headless_rgba(scene: &Scene, cam: &Camera, width: u32, height: u32) -> std::result::Result<(Vec<u8>, &'static str), String> {
    match headless()? {
        Headless::Gpu(c) => c.rgba(scene, cam, width, height).map(|rgba| (rgba, c.renderer())).map_err(|e| e.to_string()),
        Headless::Cpu => {
            if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
                return Err(format!("a picture is 1 to {MAX_SIDE} pixels a side, not {width} × {height}"));
            }
            let canvas = solidcraft_render::render(scene, cam, width as usize, height as usize);
            Ok((canvas.rgba, "cpu"))
        }
    }
}

/// The frame parameters of a picture: the scene's own geometry as the model scene (key `key`),
/// the camera, the section, the analysis, and the silhouette outlines of the default visual style.
fn frame(scene: &Scene, cam: &Camera, [width, height]: [u32; 2], key: u64, access: Option<AccessMap>) -> Frame {
    let empty = || Arc::new(Mutex::new(None));
    let eye = cam.eye().to_f32();
    let cap = scene.cap.unwrap_or(SECTION_CAP);
    let mode = scene.analysis.map(|a| a.mode);
    Frame {
        product: false,
        key,
        slot: Arc::new(Mutex::new(Some(scene_geometry(scene)))),
        sketch_key: 0,
        sketch_slot: empty(),
        hl_key: 0,
        hl_slot: empty(),
        pv_key: 0,
        pv_slot: empty(),
        view_proj: cam.view_proj(f64::from(width) / f64::from(height.max(1)), scene.radius.max(1.0)).to_f32(),
        back: cam.back().to_f32(),
        size_px: [width as f32, height as f32],
        clip: scene.clip.map_or([0.0; 4], |(n, d)| [n.x as f32, n.y as f32, n.z as f32, d as f32]),
        cap: [f32::from(cap.0) / 255.0, f32::from(cap.1) / 255.0, f32::from(cap.2) / 255.0, 1.0],
        images: Vec::new(),
        analysis: analysis_uniform(mode, access.as_ref()),
        analysis_colors: scene.analysis.map_or([0.0; 36], |a| analysis_colors(&a.colors)),
        access,
        outline: true,
        eye: [eye[0], eye[1], eye[2], if cam.fov > 0.0 { 1.0 } else { 0.0 }],
        ssaa: 1.0,
    }
}

/// A scene's meshes and lines as the viewport's model scene: faces in their colours (smooth
/// normals; a face normal where a vertex has none), lines as wide lines (over everything when
/// `on_top`), the grid under everything.
pub fn scene_geometry(scene: &Scene) -> GpuScene {
    let mut sc = GpuScene::default();
    let c4 = |c: Rgb| [c.0, c.1, c.2, 255];
    for m in &scene.meshes {
        let faces: HashMap<u32, Rgb> = m.face_colors.iter().copied().collect();
        let mesh = &m.mesh;
        for (ti, t) in mesh.triangles.iter().enumerate() {
            let col = c4(mesh.tri_face.get(ti).and_then(|f| faces.get(f)).copied().unwrap_or(m.color));
            let [Some(a), Some(b), Some(c)] = t.map(|k| mesh.positions.get(k as usize).copied()) else { continue };
            let flat = (b - a).cross(c - a).normalized().unwrap_or(Vec3::Z);
            for (k, p) in t.iter().zip([a, b, c]) {
                let n = mesh.normals.get(*k as usize).copied().filter(|n| n.is_finite() && n.len() > 0.5).unwrap_or(flat);
                sc.tri(p.to_f32(), n.to_f32(), col);
            }
        }
    }
    for l in &scene.grid {
        for w in l.points.windows(2) {
            sc.under_line(w[0].to_f32(), w[1].to_f32(), c4(l.color), l.width as f32);
        }
    }
    for l in &scene.lines {
        for w in l.points.windows(2) {
            sc.line(w[0].to_f32(), w[1].to_f32(), c4(l.color), l.width as f32, l.on_top);
        }
    }
    sc
}

/// Error scopes catching what a picture's GPU work may raise (popped by [`caught`]).
fn error_scopes(device: &wgpu::Device) -> [wgpu::ErrorScopeGuard; 3] {
    [wgpu::ErrorFilter::OutOfMemory, wgpu::ErrorFilter::Internal, wgpu::ErrorFilter::Validation].map(|f| device.push_error_scope(f))
}

/// The first error the scopes caught.
fn caught(scopes: [wgpu::ErrorScopeGuard; 3]) -> Option<String> {
    let mut failed = None;
    for scope in scopes.into_iter().rev() {
        if let Some(e) = popped(scope.pop()) {
            failed.get_or_insert(e.to_string());
        }
    }
    failed
}

/// The background gradient: the viewport's top colour at the top, its bottom one at the bottom,
/// mixed in sRGB as the window paints it.
const BACKGROUND_SHADER: &str = r#"
struct O { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@vertex
fn vs(@location(0) p: vec2<f32>, @location(1) c: vec4<f32>) -> O {
    var o: O;
    o.pos = vec4<f32>(p, 0.0, 1.0);
    o.color = c;
    return o;
}
@fragment
fn fs(i: O) -> @location(0) vec4<f32> {
    return i.color;
}
"#;

/// Background vertex: NDC position (2 × f32) and sRGB colour (4 × u8).
const BACKGROUND_VERTEX: u64 = 12;

fn background_vertices(device: &wgpu::Device, top: Rgb, bottom: Rgb) -> wgpu::Buffer {
    let mut bytes = Vec::with_capacity(6 * BACKGROUND_VERTEX as usize);
    let corner = |x: f32, y: f32| (x, y, if y > 0.0 { top } else { bottom });
    for (x, y, c) in [corner(-1.0, 1.0), corner(1.0, 1.0), corner(1.0, -1.0), corner(-1.0, 1.0), corner(1.0, -1.0), corner(-1.0, -1.0)] {
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        bytes.extend_from_slice(&[c.0, c.1, c.2, 255]);
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sc_capture_background"),
        contents: &bytes,
        usage: wgpu::BufferUsages::VERTEX,
    })
}

fn background_pipeline(device: &wgpu::Device, samples: u32, depth: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sc_capture_background"),
        source: wgpu::ShaderSource::Wgsl(BACKGROUND_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sc_capture_background"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    const A: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Unorm8x4];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sc_capture_background"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: BACKGROUND_VERTEX, step_mode: wgpu::VertexStepMode::Vertex, attributes: &A })],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
