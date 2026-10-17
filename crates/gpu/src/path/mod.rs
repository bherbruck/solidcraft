//! Native progressive opaque product rendering. Software BVH traversal needs no ray-query
//! features. Every worker quantum submits at most 256K paths, then checks cancellation.
use solidcraft_geom::Vec3;
use solidcraft_render::{
    Availability, ExecutorDescriptor, ExecutorId, ProductScene, RenderFeature, RenderFormat, RenderLimits, RenderMode, RenderOperation,
    RenderProfile, ScratchLayout,
    job::{
        Admission, CancelToken, ErrorCode, ImageExecutor, ImageHost, ImageRequest, ImageWork, JobError, JobOptions, Phase, PngTask, Progress, Result,
        TaskPoll, admit_geometry,
    },
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use wgpu::util::DeviceExt;

mod scene;
#[cfg(test)]
mod tests;
const SHADER: &str = include_str!("path.wgsl");
const MAX_TRIANGLES: u64 = 1_048_576;
const TILE: u32 = 262_144;
const TIMEOUT: Duration = Duration::from_secs(30);

fn gpu(e: impl std::fmt::Display) -> JobError {
    JobError::new(ErrorCode::HostUnavailable, e.to_string())
}

fn scopes(device: &wgpu::Device) -> [wgpu::ErrorScopeGuard; 3] {
    [wgpu::ErrorFilter::Validation, wgpu::ErrorFilter::OutOfMemory, wgpu::ErrorFilter::Internal].map(|f| device.push_error_scope(f))
}
fn checked(scopes: [wgpu::ErrorScopeGuard; 3]) -> Result<()> {
    let mut error = None;
    for scope in scopes.into_iter().rev() {
        if let Some(e) = pollster::block_on(scope.pop()) {
            error = Some(e);
        }
    }
    error.map_or(Ok(()), |e| Err(gpu(e)))
}

/// Open and validate a native device before advertising the executor. Failure leaves the
/// caller's CPU host usable. Respects SOLIDCRAFT_GPU, including the software adapter policy.
pub fn host() -> Result<ImageHost> {
    let (adapter, device, queue) = crate::open_headless().map_err(gpu)?;
    log::info!("gpu-path adapter: {} ({:?})", adapter.get_info().name, adapter.get_info().device_type);
    ImageHost::with_executor(Arc::new(Executor::new(device, queue)?))
}

/// Open an independent job device on the viewport's selected adapter. In particular,
/// software fallback and safe mode must not rediscover a failing hardware driver.
/// An explicit incompatible SOLIDCRAFT_GPU policy leaves this executor unavailable.
pub fn host_on_adapter(adapter: &wgpu::Adapter) -> Result<ImageHost> {
    let policy = crate::select::Policy::from_env().map_err(gpu)?;
    let info = adapter.get_info();
    if !policy.admits(crate::select::tier_of(info.device_type)) {
        return Err(gpu(format!("viewport adapter does not satisfy SOLIDCRAFT_GPU={}", policy.name())));
    }
    let (device, queue) = crate::open_device(adapter).map_err(gpu)?;
    log::info!("gpu-path viewport adapter: {} ({:?})", info.name, info.device_type);
    ImageHost::with_executor(Arc::new(Executor::new(device, queue)?))
}

struct Context {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}
struct Executor {
    context: Arc<Context>,
}
impl Executor {
    fn new(device: wgpu::Device, queue: wgpu::Queue) -> Result<Self> {
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| log::error!("gpu-path: {e}")));
        let scopes = scopes(&device);
        let shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gpu-path"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let entries: Vec<_> = (0..4)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 0 { wgpu::BufferBindingType::Uniform } else { wgpu::BufferBindingType::Storage { read_only: binding != 3 } },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("gpu-path"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu-path"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gpu-path"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        checked(scopes)?;
        Ok(Self { context: Arc::new(Context { device, queue, pipeline, layout }) })
    }
}

fn descriptor() -> Result<ExecutorDescriptor> {
    Ok(ExecutorDescriptor {
        id: ExecutorId::new("gpu-path").map_err(gpu)?,
        availability: Availability::Available,
        profiles: vec![RenderProfile {
            mode: RenderMode::Preview,
            operation: RenderOperation::Image,
            formats: vec![RenderFormat::Png],
            features: vec![RenderFeature::Pbr, RenderFeature::Progressive],
        }],
        limits: RenderLimits::default(),
        scratch: ScratchLayout { sample_scale: 1, sample_bytes_per_pixel: 16, output_bytes_per_pixel: 16, encoding_bytes_per_pixel: 16 },
    })
}

fn admission(scene: &ProductScene, request: &ImageRequest, options: JobOptions) -> Result<Admission> {
    options.validate()?;
    let camera = request.camera;
    if !camera.fov.is_finite()
        || !(0.0..std::f64::consts::PI).contains(&camera.fov)
        || !camera.half_height().is_finite()
        || camera.half_height() > 1e12
        || camera.distance > 1e9
    {
        return Err(JobError::new(ErrorCode::InvalidRequest, "gpu-path requires a bounded finite camera"));
    }
    let mut admission = admit_geometry(scene, request, MAX_TRIANGLES)?;
    if scene.meshes.iter().any(|m| m.opacity != 1.0 || m.faces.iter().any(|f| f.opacity != 1.0)) {
        return Err(JobError::new(ErrorCode::InvalidScene, "gpu-path supports opaque surfaces only; refraction is not implemented"));
    }
    // Peak preparation: f64 triangles + nodes + serialized upload. After preparation:
    // serialized + device geometry + accumulation/readback/RGBA/PNG. These phases don't overlap.
    let pixels = u64::from(request.width) * u64::from(request.height);
    admission.pixel_scratch_bytes = pixels * 64;
    let vertex_bytes = scene.meshes.iter().map(|m| m.mesh.positions.len() as u64 * 32).sum::<u64>();
    admission.preparation_bytes = (admission.triangles * 400 + vertex_bytes * 2 + admission.face_appearances * 8)
        .max(admission.triangles * 160 + vertex_bytes * 2 + pixels * 32)
        .max(admission.triangles * 80 + vertex_bytes + admission.pixel_scratch_bytes)
        + 1_048_576;
    admission.estimated_total_bytes = admission.geometry_bytes + admission.preparation_bytes;
    if admission.estimated_total_bytes > 512 * 1024 * 1024 {
        return Err(JobError::new(ErrorCode::LimitExceeded, "gpu-path job exceeds its 512 MiB admission estimate"));
    }
    Ok(admission)
}

impl ImageExecutor for Executor {
    fn descriptor(&self) -> Result<ExecutorDescriptor> {
        descriptor()
    }
    fn admit(&self, scene: &ProductScene, request: &ImageRequest, options: JobOptions) -> Result<Admission> {
        let a = admission(scene, request, options)?;
        let limit = self.context.device.limits().max_storage_buffer_binding_size;
        let geometry_bytes = 16 + a.triangles.max(1) * 48 + scene.meshes.iter().map(|m| m.mesh.positions.len() as u64 * 32).sum::<u64>();
        if geometry_bytes > limit || u64::from(request.width) * u64::from(request.height) * 16 > limit {
            return Err(JobError::new(ErrorCode::LimitExceeded, "gpu-path storage buffer exceeds this device's limit"));
        }
        Ok(a)
    }
    fn begin(&self, scene: ProductScene, request: ImageRequest, options: JobOptions, cancel: CancelToken) -> Result<Box<dyn ImageWork>> {
        self.admit(&scene, &request, options)?;
        Ok(Box::new(Task::new(self.context.clone(), scene, request, options, cancel)?))
    }
}

struct Task {
    context: Arc<Context>,
    request: ImageRequest,
    options: JobOptions,
    cancel: CancelToken,
    uniform: wgpu::Buffer,
    accumulation: wgpu::Buffer,
    readback: wgpu::Buffer,
    bind: wgpu::BindGroup,
    sample: u32,
    offset: u32,
    epsilon: f32,
    png: Option<PngTask>,
    started: Instant,
    // Uniform environment is used only by the numerical furnace test, never advertised as HDRI.
    environment: f32,
    studio: f32,
    #[cfg(test)]
    metrics: (f64, u64, f64),
}
impl Task {
    fn new(context: Arc<Context>, scene: ProductScene, request: ImageRequest, options: JobOptions, cancel: CancelToken) -> Result<Self> {
        cancel.check()?;
        let start = Instant::now();
        let upload = scene::flatten(&scene, request.camera.target, &cancel)?;
        let bvh_ms = start.elapsed().as_secs_f64() * 1000.0;
        let upload_bytes = (upload.geometry.len() + upload.nodes.len()) as u64;
        let uploading = Instant::now();
        drop(scene);
        let device = &context.device;
        let scopes = [wgpu::ErrorFilter::Validation, wgpu::ErrorFilter::OutOfMemory, wgpu::ErrorFilter::Internal].map(|f| device.push_error_scope(f));
        let make =
            |label, bytes: &[u8], usage| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage });
        let triangles = make("path triangles", &upload.geometry, wgpu::BufferUsages::STORAGE);
        let nodes = make("path BVH", &upload.nodes, wgpu::BufferUsages::STORAGE);
        let uniform = make("path parameters", &[0; 112], wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let size = u64::from(request.width) * u64::from(request.height) * 16;
        let accumulation = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("path accumulation"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("path readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let entries: Vec<_> = [&uniform, &triangles, &nodes, &accumulation]
            .iter()
            .enumerate()
            .map(|(binding, buffer)| wgpu::BindGroupEntry { binding: binding as u32, resource: buffer.as_entire_binding() })
            .collect();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("gpu-path"), layout: &context.layout, entries: &entries });
        let mut error = None;
        for scope in scopes.into_iter().rev() {
            if let Some(e) = pollster::block_on(scope.pop()) {
                error = Some(e);
            }
        }
        if let Some(e) = error {
            return Err(gpu(e));
        }
        let submission = context.queue.submit([]);
        device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(TIMEOUT) }).map_err(gpu)?;
        let upload_ms = uploading.elapsed().as_secs_f64() * 1000.0;
        log::info!("gpu-path BVH {bvh_ms:.3} ms, upload {upload_bytes} bytes in {upload_ms:.3} ms");
        cancel.check()?;
        Ok(Self {
            context,
            request,
            options,
            cancel,
            uniform,
            accumulation,
            readback,
            bind,
            sample: 0,
            offset: 0,
            epsilon: upload.epsilon,
            png: None,
            started: Instant::now(),
            environment: 0.0,
            studio: 1.0,
            #[cfg(test)]
            metrics: (bvh_ms, upload_bytes, upload_ms),
        })
    }
    fn parameters(&self, count: u32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(112);
        for x in [self.request.width, self.request.height, self.sample, self.offset, self.options.seed, self.options.max_depth, count, 0] {
            bytes.extend_from_slice(&x.to_le_bytes());
        }
        let camera = self.request.camera;
        let (right, up, back) = camera.basis();
        let perspective = camera.fov > 0.0;
        let eye = back * (camera.distance * if perspective { 1.0 } else { 4.0 });
        scene::vector(&mut bytes, eye, if perspective { 1.0 } else { 0.0 });
        scene::vector(&mut bytes, right * camera.half_height() * (f64::from(self.request.width) / f64::from(self.request.height)), 0.0);
        scene::vector(&mut bytes, up * camera.half_height(), 0.0);
        scene::vector(&mut bytes, back, camera.distance as f32);
        scene::vector(&mut bytes, Vec3::new(f64::from(self.epsilon), f64::from(self.environment), f64::from(self.studio)), 0.0);
        bytes
    }
    fn dispatch(&mut self) -> Result<()> {
        self.cancel.check()?;
        let pixels = self.request.width * self.request.height;
        let count = (pixels - self.offset).min(TILE);
        let context = &self.context;
        let scopes = scopes(&context.device);
        context.queue.write_buffer(&self.uniform, 0, &self.parameters(count));
        let mut encoder = context.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("path tile") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("path tile"), timestamp_writes: None });
            pass.set_pipeline(&context.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
        let submission = context.queue.submit([encoder.finish()]);
        checked(scopes)?;
        context.device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(TIMEOUT) }).map_err(gpu)?;
        self.cancel.check()?;
        self.offset += count;
        if self.offset == pixels {
            self.sample += 1;
            self.offset = 0;
        }
        Ok(())
    }
    fn read_linear(&self) -> Result<Vec<[f32; 4]>> {
        self.cancel.check()?;
        let context = &self.context;
        let scopes = scopes(&context.device);
        let mut encoder = context.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("path readback") });
        encoder.copy_buffer_to_buffer(&self.accumulation, 0, &self.readback, 0, self.accumulation.size());
        let submission = context.queue.submit([encoder.finish()]);
        checked(scopes)?;
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        context.device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(TIMEOUT) }).map_err(gpu)?;
        rx.try_recv().map_err(gpu)?.map_err(gpu)?;
        let data = slice.get_mapped_range().map_err(gpu)?;
        let mut values = scene::reserved((self.request.width * self.request.height) as usize)?;
        for pixel in data.as_chunks::<16>().0 {
            let mut value = [0.0; 4];
            for (v, bytes) in value.iter_mut().zip(pixel.as_chunks::<4>().0) {
                *v = f32::from_le_bytes(*bytes) / self.sample as f32;
                if !v.is_finite() || *v < 0.0 {
                    return Err(JobError::new(ErrorCode::Internal, "nonfinite or negative path radiance"));
                }
            }
            values.push(value);
        }
        drop(data);
        self.readback.unmap();
        self.cancel.check()?;
        Ok(values)
    }
}
impl ImageWork for Task {
    fn step(&mut self, units: usize) -> Result<TaskPoll> {
        self.cancel.check()?;
        if let Some(png) = &mut self.png {
            return png.step(units);
        }
        if self.sample < self.options.samples {
            self.dispatch()?;
            return Ok(TaskPoll::Pending(Progress { phase: Phase::Running, done: u64::from(self.sample), total: u64::from(self.options.samples) }));
        }
        log::info!("gpu-path {} spp, {:.3} ms/spp", self.sample, self.started.elapsed().as_secs_f64() * 1000.0 / f64::from(self.sample));
        let linear = self.read_linear()?;
        let mut rgba = scene::reserved(linear.len() * 4)?;
        let exposure = 2.0f64.powf(self.options.exposure_ev);
        for (index, pixel) in linear.iter().enumerate() {
            if index % 4096 == 0 {
                self.cancel.check()?;
            }
            let alpha = f64::from(pixel[3]).clamp(0.0, 1.0);
            for (radiance, background) in pixel.iter().take(3).zip(self.request.background) {
                let c = if alpha > 0.0 { f64::from(*radiance) * exposure / alpha } else { 0.0 };
                let mapped = c / (1.0 + c);
                let value = if self.request.transparent_background {
                    mapped
                } else {
                    mapped * alpha + solidcraft_render::surface::srgb8_to_linear(background) * (1.0 - alpha)
                };
                let encoded = if value <= 0.0031308 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 };
                rgba.push((encoded.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            rgba.push(if self.request.transparent_background { (alpha * 255.0).round() as u8 } else { 255 });
        }
        self.png = Some(PngTask::new(&self.request, rgba, self.cancel.clone())?);
        Ok(TaskPoll::Pending(Progress { phase: Phase::Encoding, done: 0, total: u64::from(self.request.height) }))
    }
}
