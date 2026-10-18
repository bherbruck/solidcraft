//! SolidCraft's GPU work: the viewport's render pipelines ([`view`]), pictures drawn with them
//! offscreen ([`capture`]), and display tessellation plans evaluated in a compute shader.
//!
//! Plan evaluation: the surface point and normal at every vertex's parameters, in a compute
//! shader (`tess.wgsl`), the GPU twin of [`TessPlan::evaluate`]. Shared edge nodes keep their
//! exact positions, so faces still meet exactly; evaluation runs in f32.
//!
//! Use [`TessGpu::new`] with the application's device, or [`TessGpu::headless`] (native only)
//! to open one without a window, as the CLI and tests do. A viewport draws a plan straight from
//! the GPU: [`TessGpu::prepare`] uploads it, [`TessGpu::record`] adds the evaluation to the
//! frame's command encoder, and the render pass reads [`GpuMesh::vertices`] as its vertex buffer
//! with [`GpuMesh::indices`]; nothing comes back to the CPU.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
use solidcraft_geom::Mesh;
use solidcraft_geom::Vec3;
use solidcraft_geom::tess::{EvalCurve, EvalSurface, TessPlan};
use wgpu::util::DeviceExt;

pub mod capture;
mod pbr;
pub mod pick;
#[cfg(not(target_arch = "wasm32"))]
pub mod select;
pub mod view;

/// The tessellation compute shader (WGSL).
pub const SHADER: &str = include_str!("tess.wgsl");

/// The device feature for a 32-bit float depth buffer with a stencil, which the viewport draws
/// with reversed depth ([`view::reversed_depth`]) where the device has it. A device is opened
/// with it whenever its adapter offers it ([`depth_features`]).
pub const FLOAT_DEPTH_STENCIL: wgpu::Features = wgpu::Features::DEPTH32FLOAT_STENCIL8;

/// The depth features to open a device on `adapter` with: [`FLOAT_DEPTH_STENCIL`] if it has it.
pub fn depth_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    adapter.features() & FLOAT_DEPTH_STENCIL
}

/// The depth format to draw with on `device`: 32-bit float (reversed depth) where it can, with a
/// stencil when `stencil` (the section cap needs one), else 24 bits with a stencil.
pub fn depth_format(device: &wgpu::Device, stencil: bool) -> wgpu::TextureFormat {
    match (stencil, device.features().contains(FLOAT_DEPTH_STENCIL)) {
        (false, _) => wgpu::TextureFormat::Depth32Float,
        (true, true) => wgpu::TextureFormat::Depth32FloatStencil8,
        (true, false) => wgpu::TextureFormat::Depth24PlusStencil8,
    }
}

/// The error a popped error scope caught. Native devices answer at once; a browser's answer
/// comes later, and is not waited for (no error is assumed).
fn popped(f: impl std::future::Future<Output = Option<wgpu::Error>>) -> Option<wgpu::Error> {
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
        std::task::Poll::Ready(e) => e,
        std::task::Poll::Pending => None,
    }
}

/// An adapter and device without a window (native only): the adapter [`select::pick`] chooses
/// under the `SOLIDCRAFT_GPU` policy. The environment may narrow the backends and pick the D3D12
/// shader compiler (`WGPU_BACKEND`, `WGPU_DX12_COMPILER`).
#[cfg(not(target_arch = "wasm32"))]
fn open_headless() -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let policy = select::Policy::from_env().map_err(GpuError::Policy)?;
    let (adapter, _) = select::pick(policy).ok_or(GpuError::NoAdapter(policy.name()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("solidcraft headless"),
        required_features: depth_features(&adapter),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .map_err(|e| GpuError::Device(e.to_string()))?;
    Ok((adapter, device, queue))
}

/// Most vertices evaluated in one dispatch (bounds the buffers; larger plans go in batches).
pub const MAX_BATCH: usize = 1 << 20;
const WORKGROUP: u32 = 64;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no GPU adapter qualifies under the `{0}` policy (SOLIDCRAFT_GPU); `solidcraft-cli gpu` lists the adapters this machine offers")]
    NoAdapter(&'static str),
    #[error("{0}")]
    Policy(String),
    #[error("the GPU device could not be opened: {0}")]
    Device(String),
    #[error("the GPU evaluation failed: {0}")]
    Failed(String),
    #[error("the GPU render failed: {0}")]
    Render(String),
    #[error("the GPU tessellation pipeline could not be created: {0}")]
    Pipeline(String),
    #[error("not supported yet: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, GpuError>;

/// Bytes per evaluated vertex in [`GpuMesh::vertices`]: the position (x, y, z, 1) then the
/// normal (x, y, z, 0), each four f32.
pub const VERTEX_STRIDE: u64 = 32;
/// Byte offset of the normal within a vertex.
pub const NORMAL_OFFSET: u64 = 16;

/// A plan on the GPU: its inputs, and the buffers the evaluation writes and a render pass
/// reads. Evaluated once [`TessGpu::record`] has run in a submitted encoder.
pub struct GpuMesh {
    /// Per vertex: position and normal (see [`VERTEX_STRIDE`]); a vertex and storage buffer.
    pub vertices: wgpu::Buffer,
    /// The plan's triangles, three u32 vertex indices each.
    pub indices: wgpu::Buffer,
    pub index_count: u32,
    pub vertex_count: u32,
    /// Each vertex's face (its plan's face numbering), for colouring by face.
    pub vertex_face: Vec<u32>,
    /// Per plan (in the order given), its vertices and indices.
    pub parts: Vec<Part>,
    bind: wgpu::BindGroup,
    // The inputs, kept until the evaluation has run.
    _inputs: [wgpu::Buffer; 4],
}

/// One plan's share of a [`GpuMesh`]: its vertices and its entries in the index buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub vertices: std::ops::Range<u32>,
    pub indices: std::ops::Range<u32>,
}

/// The compute pipeline, ready to evaluate plans on one device.
pub struct TessGpu {
    adapter: String,
    device: wgpu::Device,
    /// For the blocking [`TessGpu::evaluate`] (native only).
    #[cfg(not(target_arch = "wasm32"))]
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl TessGpu {
    /// The pipeline on an existing device (`adapter`: its name, for reports). An error (never
    /// a panic) when the device cannot build it, for example where the D3D12 driver's older shader
    /// compiler (FXC) rejects the shader: the caller then builds bodies on the CPU.
    pub fn new(adapter: impl Into<String>, device: wgpu::Device, queue: wgpu::Queue) -> Result<Self> {
        Self::with_shader(adapter, device, queue, SHADER)
    }

    /// [`TessGpu::new`] with another shader source: tests use it to make pipeline creation fail
    /// as it does on a device whose compiler rejects the real shader.
    #[doc(hidden)]
    pub fn with_shader(adapter: impl Into<String>, device: wgpu::Device, queue: wgpu::Queue, shader: &str) -> Result<Self> {
        // Errors here would go to the device's uncaptured-error handler, which panics by default:
        // capture them, so a device that cannot run the shader only loses GPU tessellation.
        let scopes = [wgpu::ErrorFilter::OutOfMemory, wgpu::ErrorFilter::Internal, wgpu::ErrorFilter::Validation].map(|f| device.push_error_scope(f));
        let (pipeline, layout) = Self::build(&device, shader);
        let mut failed = None;
        for scope in scopes.into_iter().rev() {
            if let Some(e) = popped(scope.pop()) {
                failed.get_or_insert(e.to_string());
            }
        }
        if let Some(e) = failed {
            return Err(GpuError::Pipeline(e));
        }
        // A viewport records the evaluation into its own frame (see `TessGpu::record`).
        #[cfg(target_arch = "wasm32")]
        let _ = queue;
        Ok(TessGpu {
            adapter: adapter.into(),
            device,
            #[cfg(not(target_arch = "wasm32"))]
            queue,
            pipeline,
            layout,
        })
    }

    fn build(device: &wgpu::Device, shader: &str) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("solidcraft tessellation"),
            source: wgpu::ShaderSource::Wgsl(shader.into()),
        });
        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let storage = |read_only| wgpu::BufferBindingType::Storage { read_only };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("solidcraft tessellation"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Uniform),
                entry(1, storage(true)),
                entry(2, storage(true)),
                entry(3, storage(true)),
                entry(4, storage(false)),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("solidcraft tessellation"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("solidcraft tessellation"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        (pipeline, layout)
    }

    /// A device of its own, without a window (native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless() -> Result<Self> {
        let (adapter, device, queue) = open_headless()?;
        Self::new(adapter.get_info().name, device, queue)
    }

    /// The adapter's name, for reports.
    pub fn adapter_name(&self) -> &str {
        &self.adapter
    }

    /// Evaluate `plan` on the GPU into the same mesh [`TessPlan::evaluate`] gives (positions
    /// and normals in f32, fixed edge nodes exact). Blocks until the GPU is done (native).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn evaluate(&self, plan: &TessPlan) -> Result<Mesh> {
        let packed = pack(&[plan])?;
        let mut mesh = Mesh::default();
        let mut base = 0usize;
        for chunk in packed.vertices.chunks(MAX_BATCH * VERTEX_WORDS) {
            let n = chunk.len() / VERTEX_WORDS;
            let out = self.run(&packed, chunk, n)?;
            for k in 0..n {
                let at = |i: usize| out.get(8 * k + i).copied().map_or(0.0, f64::from);
                mesh.positions.push(Vec3::new(at(0), at(1), at(2)));
                mesh.normals.push(Vec3::new(at(4), at(5), at(6)));
            }
            base += n;
        }
        if base != packed.count {
            return Err(GpuError::Failed("vertex count mismatch".into()));
        }
        // Fixed nodes exactly (the shader has them in f32), and the plan's topology.
        for (k, p) in packed.fixed {
            if let Some(slot) = mesh.positions.get_mut(k) {
                *slot = p;
            }
        }
        let mut offset = 0u32;
        for f in &plan.faces {
            let n = u32::try_from(f.uv.len()).map_err(|_| GpuError::Failed("too many vertices".into()))?;
            for t in &f.triangles {
                if t.iter().all(|&i| i < n) {
                    mesh.triangles.push([offset + t[0], offset + t[1], offset + t[2]]);
                    mesh.tri_face.push(f.face);
                }
            }
            offset += n;
        }
        mesh.edges = plan.edges.clone();
        mesh.edge_faces = plan.edge_faces.clone();
        Ok(mesh)
    }

    /// Upload `plan` and create the buffers its evaluation writes; nothing runs until
    /// [`TessGpu::record`]. Refused (so the caller draws its CPU mesh) for a plan with more than
    /// [`MAX_BATCH`] vertices or a face without an evaluable surface.
    pub fn prepare(&self, plan: &TessPlan) -> Result<GpuMesh> {
        self.prepare_many(&[plan])
    }

    /// Like [`TessGpu::prepare`] for several plans at once: one upload, one buffer and one
    /// dispatch for all of them (a viewport's whole scene), each plan's vertices and triangles in
    /// [`GpuMesh::parts`]. Refused past [`MAX_BATCH`] vertices in all.
    pub fn prepare_many(&self, plans: &[&TessPlan]) -> Result<GpuMesh> {
        let packed = pack(plans)?;
        if packed.count > MAX_BATCH {
            return Err(GpuError::Unsupported(format!("{} vertices in one dispatch", packed.count)));
        }
        let d = &self.device;
        let count = word(packed.count)?;
        let init = |label: &str, contents: &[u8], usage| {
            // wgpu rejects empty bindings: pad to one word.
            let contents = if contents.is_empty() { &[0u8; 16][..] } else { contents };
            d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        let words = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let floats = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let params = init("tess params", &words(&[count, 0, 0, 0]), wgpu::BufferUsages::UNIFORM);
        let surfaces = init("tess surfaces", &words(&packed.surfaces), wgpu::BufferUsages::STORAGE);
        let data = init("tess data", &floats(&packed.data), wgpu::BufferUsages::STORAGE);
        let verts = init("tess vertices", &words(&packed.vertices), wgpu::BufferUsages::STORAGE);
        let vertices = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tess mesh vertices"),
            size: (packed.count.max(1) as u64) * VERTEX_STRIDE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let mut tri: Vec<u32> = Vec::new();
        let mut vertex_face = Vec::with_capacity(packed.count);
        let mut parts = Vec::with_capacity(plans.len());
        let mut offset = 0u32;
        for plan in plans {
            let (v0, i0) = (offset, word(tri.len())?);
            for f in &plan.faces {
                let n = word(f.uv.len())?;
                for t in f.triangles.iter().filter(|t| t.iter().all(|&i| i < n)) {
                    tri.extend([offset + t[0], offset + t[1], offset + t[2]]);
                }
                vertex_face.extend(std::iter::repeat_n(f.face, f.uv.len()));
                offset += n;
            }
            parts.push(Part { vertices: v0..offset, indices: i0..word(tri.len())? });
        }
        let indices = init("tess mesh indices", &words(&tri), wgpu::BufferUsages::INDEX);
        let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tess mesh"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: surfaces.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: data.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: verts.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: vertices.as_entire_binding() },
            ],
        });
        Ok(GpuMesh {
            vertices,
            indices,
            index_count: word(tri.len())?,
            vertex_count: count,
            vertex_face,
            parts,
            bind,
            _inputs: [params, surfaces, data, verts],
        })
    }

    /// Add `mesh`'s evaluation to `encoder` (one dispatch, no wait, nothing read back): once the
    /// encoder is submitted, render passes after it read the evaluated vertices.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder, mesh: &GpuMesh) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("tess mesh"), timestamp_writes: None });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &mesh.bind, &[]);
        pass.dispatch_workgroups(mesh.vertex_count.div_ceil(WORKGROUP).max(1), 1, 1);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn run(&self, packed: &Packed, vertices: &[u32], n: usize) -> Result<Vec<f32>> {
        let d = &self.device;
        let bytes_u32 = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let bytes_f32 = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let count = u32::try_from(n).map_err(|_| GpuError::Failed("batch too large".into()))?;
        let init = |label: &str, contents: &[u8], usage| {
            // wgpu rejects empty bindings: pad to one word.
            let contents = if contents.is_empty() { &[0u8; 16][..] } else { contents };
            d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        let params = init("tess params", &bytes_u32(&[count, 0, 0, 0]), wgpu::BufferUsages::UNIFORM);
        let surfaces = init("tess surfaces", &bytes_u32(&packed.surfaces), wgpu::BufferUsages::STORAGE);
        let data = init("tess data", &bytes_f32(&packed.data), wgpu::BufferUsages::STORAGE);
        let verts = init("tess vertices", &bytes_u32(vertices), wgpu::BufferUsages::STORAGE);
        let size = (n.max(1) * 32) as u64;
        let out = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tess out"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tess read"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tess"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: surfaces.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: data.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: verts.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: out.as_entire_binding() },
            ],
        });
        let mut enc = d.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("tess") });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("tess"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(count.div_ceil(WORKGROUP).max(1), 1, 1);
        }
        enc.copy_buffer_to_buffer(&out, 0, &read, 0, size);
        self.queue.submit([enc.finish()]);
        let slice = read.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        d.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Failed(e.to_string()))?;
        rx.recv().map_err(|e| GpuError::Failed(e.to_string()))?.map_err(|e| GpuError::Failed(e.to_string()))?;
        let bytes = slice.get_mapped_range().map_err(|e| GpuError::Failed(e.to_string()))?;
        let values = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        drop(bytes);
        read.unmap();
        Ok(values)
    }
}

/// Words per vertex: u, v (f32 bits), surface, flags, then a vec4 position.
const VERTEX_WORDS: usize = 8;
/// Words per surface header (matches `Surface` in the shader).
const SURFACE_WORDS: usize = 12;

/// A plan as the shader reads it.
struct Packed {
    surfaces: Vec<u32>,
    data: Vec<f32>,
    vertices: Vec<u32>,
    count: usize,
    /// Exact positions of fixed vertices (index in the evaluated order), for the blocking
    /// [`TessGpu::evaluate`] to restore after the f32 evaluation (native only).
    #[cfg(not(target_arch = "wasm32"))]
    fixed: Vec<(usize, Vec3)>,
}

fn word(x: usize) -> Result<u32> {
    u32::try_from(x).map_err(|_| GpuError::Failed("plan too large".into()))
}

fn push3(data: &mut Vec<f32>, v: Vec3) {
    data.extend([v.x as f32, v.y as f32, v.z as f32]);
}

fn push_curve(data: &mut Vec<f32>, c: &EvalCurve) -> Result<(u32, u32)> {
    let knots = word(data.len())?;
    data.extend(c.knots.iter().map(|k| *k as f32));
    let ctrl = word(data.len())?;
    data.extend(c.ctrl.iter().flat_map(|h| h.map(|x| x as f32)));
    Ok((knots, ctrl))
}

/// Several plans as one: their surfaces one after another, each face's surface index offset by
/// the surfaces of the plans before it.
fn pack(plans: &[&TessPlan]) -> Result<Packed> {
    let mut surfaces = Vec::with_capacity(plans.iter().map(|p| p.surfaces.len()).sum::<usize>() * SURFACE_WORDS);
    let mut data: Vec<f32> = Vec::new();
    for s in plans.iter().flat_map(|p| &p.surfaces) {
        let mut head = [0u32; SURFACE_WORDS];
        match s {
            EvalSurface::Plane { origin, du, dv } => {
                head[0] = 0;
                head[8] = word(data.len())?;
                push3(&mut data, *origin);
                push3(&mut data, *du);
                push3(&mut data, *dv);
            }
            EvalSurface::Nurbs { degree, knots, count, ctrl, closed } => {
                head[0] = 1;
                head[11] = u32::from(closed[0]) | (u32::from(closed[1]) << 1);
                head[1] = word(degree[0])?;
                head[2] = word(degree[1])?;
                head[3] = word(count[0])?;
                head[4] = word(count[1])?;
                head[5] = word(data.len())?;
                data.extend(knots[0].iter().map(|k| *k as f32));
                head[6] = word(data.len())?;
                data.extend(knots[1].iter().map(|k| *k as f32));
                head[7] = word(data.len())?;
                data.extend(ctrl.iter().flat_map(|h| h.map(|x| x as f32)));
            }
            EvalSurface::Revolved { profile, origin, axis, transform, swap } => {
                head[0] = 2;
                head[1] = word(profile.degree)?;
                head[3] = word(profile.ctrl.len())?;
                let (k, c) = push_curve(&mut data, profile)?;
                head[5] = k;
                head[7] = c;
                head[8] = word(data.len())?;
                push3(&mut data, *origin);
                push3(&mut data, *axis);
                data.extend(transform.iter().flatten().map(|x| *x as f32));
                head[9] = u32::from(*swap);
                let m = transform;
                let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                    + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
                head[10] = u32::from(det < 0.0);
            }
        }
        surfaces.extend(head);
    }
    let mut vertices = Vec::with_capacity(plans.iter().map(|p| p.vertex_count()).sum::<usize>() * VERTEX_WORDS);
    #[cfg(not(target_arch = "wasm32"))]
    let mut fixed = Vec::new();
    let mut count = 0usize;
    let mut base = 0u32;
    for plan in plans {
        for f in &plan.faces {
            let Some(surface) = f.surface else {
                return Err(GpuError::Unsupported("a face without an evaluable surface".into()));
            };
            if surface as usize >= plan.surfaces.len() {
                return Err(GpuError::Failed("surface index out of range".into()));
            }
            let surface = base + surface;
            for (k, uv) in f.uv.iter().enumerate() {
                let pinned = f.fixed.get(k).copied();
                let flags = u32::from(f.flip) | if pinned.is_some() { 2 } else { 0 };
                let p = pinned.unwrap_or(Vec3::ZERO);
                vertices.extend([
                    (uv[0] as f32).to_bits(),
                    (uv[1] as f32).to_bits(),
                    surface,
                    flags,
                    (p.x as f32).to_bits(),
                    (p.y as f32).to_bits(),
                    (p.z as f32).to_bits(),
                    0,
                ]);
                if let Some(p) = pinned {
                    #[cfg(not(target_arch = "wasm32"))]
                    fixed.push((count, p));
                    #[cfg(target_arch = "wasm32")]
                    let _ = p;
                }
                count += 1;
            }
        }
        base += word(plan.surfaces.len())?;
    }
    Ok(Packed {
        surfaces,
        data,
        vertices,
        count,
        #[cfg(not(target_arch = "wasm32"))]
        fixed,
    })
}

#[cfg(test)]
mod tests;
