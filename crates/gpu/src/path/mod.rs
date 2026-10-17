//! Native progressive product rendering. Software BVH traversal needs no ray-query
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
mod textures;
const SHADER: &str = concat!(include_str!("path.wgsl"), "\n", include_str!("shadows.wgsl"), "\n", include_str!("textures.wgsl"));
const MAX_TRIANGLES: u64 = 1_048_576;
const PARAMETER_BYTES: usize = 448;
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
    let (adapter, device, queue) = open_headless()?;
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
    let (device, queue) = open_device(adapter)?;
    log::info!("gpu-path viewport adapter: {} ({:?})", info.name, info.device_type);
    ImageHost::with_executor(Arc::new(Executor::new(device, queue)?))
}

// Path resources need five storage buffers (geometry, BVH, accumulation, HDRI,
// textures). Request that limit only on this independent path device: raster and
// tessellation keep their existing four-buffer downlevel requirement.
const PATH_STORAGE_BUFFERS: u32 = 5;
fn device_limits(supported: wgpu::Limits) -> Result<wgpu::Limits> {
    if supported.max_storage_buffers_per_shader_stage < PATH_STORAGE_BUFFERS {
        return Err(gpu("gpu-path requires five storage buffers per shader stage"));
    }
    let mut limits = wgpu::Limits::downlevel_defaults().using_resolution(supported);
    limits.max_storage_buffers_per_shader_stage = PATH_STORAGE_BUFFERS;
    Ok(limits)
}
fn open_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue)> {
    let required_limits = device_limits(adapter.limits())?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("solidcraft progressive path"),
        required_features: crate::depth_features(adapter),
        required_limits,
        ..Default::default()
    }))
    .map_err(gpu)
}
fn open_headless() -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let policy = crate::select::Policy::from_env().map_err(gpu)?;
    let (adapter, _) = crate::select::pick(policy).ok_or_else(|| gpu(format!("no gpu-path adapter satisfies {}", policy.name())))?;
    let (device, queue) = open_device(&adapter)?;
    Ok((adapter, device, queue))
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
        device_limits(device.limits())?;
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| log::error!("gpu-path: {e}")));
        let scopes = scopes(&device);
        let shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gpu-path"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let entries: Vec<_> = (0..6)
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
            features: vec![RenderFeature::Pbr, RenderFeature::Progressive, RenderFeature::Refraction, RenderFeature::Hdri],
        }],
        limits: RenderLimits::default(),
        scratch: ScratchLayout { sample_scale: 1, sample_bytes_per_pixel: 16, output_bytes_per_pixel: 16, encoding_bytes_per_pixel: 16 },
    })
}

fn admission(scene: &ProductScene, request: &ImageRequest, options: JobOptions) -> Result<Admission> {
    options.validate()?;
    if let Some(environment) = &request.environment {
        environment.validate()?;
    }
    let camera = request.camera;
    if options.lens.is_some_and(|lens| lens.aperture_radius_mm > 0.0)
        && (camera.distance < 1e-6 || !(1e-6..=1e6).contains(&(camera.half_height() / camera.distance)) || camera.fov == 0.0)
    {
        return Err(JobError::new(
            ErrorCode::InvalidRequest,
            "thin-lens depth of field requires a bounded perspective camera (distance >= 0.000001 mm and tan(fov/2) in 0.000001..1000000)",
        ));
    }
    if !camera.fov.is_finite()
        || !(0.0..std::f64::consts::PI).contains(&camera.fov)
        || !camera.half_height().is_finite()
        || camera.half_height() > 1e12
        || camera.distance > 1e9
    {
        return Err(JobError::new(ErrorCode::InvalidRequest, "gpu-path requires a bounded finite camera"));
    }
    let mut admission = admit_geometry(scene, request, MAX_TRIANGLES)?;
    if scene
        .meshes
        .iter()
        .any(|m| (m.opacity != 1.0 && m.transmission.is_none()) || m.faces.iter().any(|f| f.opacity != 1.0 && f.transmission.is_none()))
    {
        return Err(JobError::new(ErrorCode::InvalidScene, "gpu-path requires explicit transmission for nonopaque preview appearances"));
    }
    // Peak preparation: f64 triangles + nodes + serialized upload. After preparation:
    // serialized + device geometry + accumulation/readback/RGBA/PNG. These phases don't overlap.
    let pixels = u64::from(request.width) * u64::from(request.height);
    admission.pixel_scratch_bytes = pixels * 64;
    let vertex_bytes = scene.meshes.iter().map(|m| m.mesh.positions.len() as u64 * 32).sum::<u64>();
    let triangle_preparation =
        (admission.triangles * 400_u64.max(scene::planar_preparation_bytes_per_triangle() as u64)).max(scene::medium_preparation_bytes(scene));
    admission.preparation_bytes = (triangle_preparation + vertex_bytes * 2 + admission.face_appearances * 8)
        .max(admission.triangles * 232 + vertex_bytes * 2 + pixels * 32)
        .max(admission.triangles * 116 + vertex_bytes + admission.pixel_scratch_bytes)
        + 1_048_576;
    admission.preparation_bytes += request.environment.as_ref().map_or(0, |source| source.admitted_bytes());
    if let Some(textures) = &request.textures {
        textures.validate_scene(scene)?;
        admission.preparation_bytes += textures.working_bytes();
    }
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
        let geometry_bytes = 16 + a.triangles.max(1) * 84 + scene.meshes.iter().map(|m| m.mesh.positions.len() as u64 * 32).sum::<u64>();
        let texture_bytes = request
            .textures
            .as_ref()
            .map_or(16, |t| t.summary().linear_pixel_bytes + 4 * (5 + scene.meshes.len() as u64 + t.bindings().len() as u64 * 16));
        let environment_bytes =
            request.environment.as_ref().map_or(48, |source| 16 + u64::from(source.info().width) * u64::from(source.info().height) * 32);
        if texture_bytes > limit
            || environment_bytes > limit
            || geometry_bytes > limit
            || u64::from(request.width) * u64::from(request.height) * 16 > limit
        {
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
    environment_bottom_only: bool,
    studio: f32,
    #[cfg(test)]
    metrics: (f64, u64, f64),
}
impl Task {
    fn new(context: Arc<Context>, scene: ProductScene, mut request: ImageRequest, options: JobOptions, cancel: CancelToken) -> Result<Self> {
        cancel.check()?;
        let start = Instant::now();
        if let Some(resources) = &request.textures {
            resources.validate_scene(&scene)?;
        }
        let texture_upload = textures::flatten(request.textures.as_deref(), scene.meshes.len(), request.camera.target, &cancel)?;
        request.textures = None; // GPU upload owns decoded pixels; release captured source bytes.
        let upload = scene::flatten(&scene, request.camera.target, &cancel)?;
        let bvh_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut upload_bytes = (upload.geometry.len() + upload.nodes.len() + texture_upload.len()) as u64;
        let uploading = Instant::now();
        drop(scene);
        let device = &context.device;
        let scopes = [wgpu::ErrorFilter::Validation, wgpu::ErrorFilter::OutOfMemory, wgpu::ErrorFilter::Internal].map(|f| device.push_error_scope(f));
        let make =
            |label, bytes: &[u8], usage| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage });
        let mut environment_bytes = Vec::new();
        if let Some(source) = &request.environment {
            let prepared = source.prepare(&cancel)?;
            environment_bytes = scene::reserved(16 + prepared.texels().len() * 32)?;
            let [width, height] = prepared.dimensions();
            for x in [width, height, width * height, 0] {
                environment_bytes.extend_from_slice(&x.to_le_bytes());
            }
            for (index, texel) in prepared.texels().iter().enumerate() {
                if index % 1024 == 0 {
                    cancel.check()?;
                }
                for x in texel.rgb.into_iter().chain([texel.pdf, texel.threshold]) {
                    environment_bytes.extend_from_slice(&x.to_le_bytes());
                }
                environment_bytes.extend_from_slice(&texel.alias.to_le_bytes());
                environment_bytes.extend_from_slice(&[0; 8]);
            }
        } else {
            environment_bytes.resize(48, 0);
        }
        upload_bytes += environment_bytes.len() as u64;
        let environment = make("path environment", &environment_bytes, wgpu::BufferUsages::STORAGE);
        drop(environment_bytes);
        let triangles = make("path triangles", &upload.geometry, wgpu::BufferUsages::STORAGE);
        let nodes = make("path BVH", &upload.nodes, wgpu::BufferUsages::STORAGE);
        let texture_buffer = make("path opaque textures", &texture_upload, wgpu::BufferUsages::STORAGE);
        let uniform = make("path parameters", &[0; PARAMETER_BYTES], wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
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
        let mut entries: Vec<_> = [&uniform, &triangles, &nodes, &accumulation, &environment]
            .iter()
            .enumerate()
            .map(|(binding, buffer)| wgpu::BindGroupEntry { binding: binding as u32, resource: buffer.as_entire_binding() })
            .collect();
        entries.push(wgpu::BindGroupEntry { binding: 5, resource: texture_buffer.as_entire_binding() });
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
            environment_bottom_only: false,
            studio: 1.0,
            #[cfg(test)]
            metrics: (bvh_ms, upload_bytes, upload_ms),
        })
    }
    fn parameters(&self, count: u32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PARAMETER_BYTES);
        for x in [
            self.request.width,
            self.request.height,
            self.sample,
            self.offset,
            self.options.seed,
            self.options.max_depth,
            count,
            u32::from(self.environment_bottom_only),
        ] {
            bytes.extend_from_slice(&x.to_le_bytes());
        }
        let camera = self.request.camera;
        let (right, up, back) = camera.basis();
        let perspective = camera.fov > 0.0;
        let eye = back * (camera.distance * if perspective { 1.0 } else { 4.0 });
        scene::vector(&mut bytes, eye, if perspective { 1.0 } else { 0.0 });
        scene::vector(
            &mut bytes,
            right * camera.half_height() * (f64::from(self.request.width) / f64::from(self.request.height)),
            self.options.lens.map_or(0.0, |l| l.aperture_radius_mm as f32),
        );
        scene::vector(&mut bytes, up * camera.half_height(), self.options.lens.map_or(0.0, |l| l.focus_distance_mm as f32));
        scene::vector(&mut bytes, back, camera.distance as f32);
        scene::vector(&mut bytes, Vec3::new(f64::from(self.epsilon), f64::from(self.environment), f64::from(self.studio)), 0.0);
        let lighting = self.options.lighting.unwrap_or_default();
        scene::vector(
            &mut bytes,
            Vec3::new(lighting.environment[0], lighting.environment[1], lighting.environment[2]),
            if self.options.lighting.is_some() || self.request.environment.is_some() { 1.0 } else { 0.0 },
        );
        scene::vector(&mut bytes, Vec3::ZERO, lighting.area_lights.iter().count() as f32);
        for light in lighting.area_lights.iter() {
            let vector = |[x, y, z]: [f64; 3]| Vec3::new(x, y, z);
            scene::vector(&mut bytes, vector(light.center) - camera.target, light.area() as f32);
            scene::vector(&mut bytes, vector(light.u), 0.0);
            scene::vector(&mut bytes, vector(light.v), 0.0);
            scene::vector(&mut bytes, vector(light.radiance), 0.0);
        }
        bytes.resize(400, 0);
        if let Some(source) = &self.request.environment {
            let info = source.info();
            scene::vector(&mut bytes, Vec3::new(info.rotation_rad.cos(), info.rotation_rad.sin(), info.intensity), 1.0);
        } else {
            scene::vector(&mut bytes, Vec3::ZERO, 0.0);
        }
        if let Some(sun) = lighting.sun {
            scene::vector(&mut bytes, sun.direction(), sun.angular_radius_rad.cos() as f32);
            let [r, g, b] = sun.radiance;
            scene::vector(&mut bytes, Vec3::new(r, g, b), (1.0 / sun.solid_angle()) as f32);
        } else {
            bytes.resize(PARAMETER_BYTES, 0);
        }

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
        for (index, pixel) in data.as_chunks::<16>().0.iter().enumerate() {
            if f32::from_le_bytes([pixel[12], pixel[13], pixel[14], pixel[15]]) < 0.0 {
                let diagnostic = [0, 4, 8, 12].map(|i| f32::from_le_bytes([pixel[i], pixel[i + 1], pixel[i + 2], pixel[i + 3]]));
                return Err(JobError::new(
                    ErrorCode::InvalidScene,
                    format!(
                        "inconsistent glass medium path at pixel {index}, bounce {}, sample {}, code {}: camera must start outside volumes; overlapping solids or more than 16 nested media are unsupported",
                        diagnostic[0], diagnostic[1], -diagnostic[3]
                    ),
                ));
            }
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
                let mapped = self.options.tone_map.map(c);
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
