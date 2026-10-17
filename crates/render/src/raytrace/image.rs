//! Opt-in opaque direct image integration; immutable world geometry, one host reservation.
use super::anisotropic_faces::{self, PlanarTarget};
use super::bvh::TraceCursor;
use super::lighting::{DirectionalLight, Lighting, StudioPreset, map_to_display};
use super::prepared::{PreparedPrimitive, PreparedScene};
use super::reflection;
use super::{BuildLimits, Bvh, BvhBuilder, MAX_WORK, Ray, TraceError, Triangle, WorkBudget};
use crate::job::{self, Admission, CancelToken, ErrorCode, ImageRequest, JobError, Phase, Progress, RaytraceOptions, TaskPoll};
use crate::{
    ProductScene,
    surface::{OpaqueSurface, srgb8_to_linear},
};
use solidcraft_geom::Vec3;

#[cfg(test)]
#[path = "environment_image_tests.rs"]
mod environment_image_tests;

#[cfg(test)]
#[path = "environment_reflection_tests.rs"]
mod environment_reflection_tests;

#[cfg(test)]
#[path = "anisotropic_reflection_image_tests.rs"]
mod anisotropic_reflection_tests;

fn error(e: TraceError) -> JobError {
    let code = match e {
        TraceError::Cancelled => ErrorCode::Cancelled,
        TraceError::MemoryLimit | TraceError::WorkLimit => ErrorCode::LimitExceeded,
        TraceError::AllocationFailed => ErrorCode::AllocationFailed,
        TraceError::InvalidInput(_) => ErrorCode::InvalidScene,
        _ => ErrorCode::Internal,
    };
    JobError::new(code, e.to_string())
}
fn invalid(message: &'static str) -> JobError {
    JobError::new(ErrorCode::InvalidScene, message)
}
fn limits() -> BuildLimits {
    BuildLimits { max_triangles: 262_144, max_memory_bytes: 384 * 1024 * 1024, max_work: MAX_WORK, ..Default::default() }
}
fn lighting(options: &RaytraceOptions) -> job::Result<Lighting> {
    options.validate()?;
    if let Some(preset) = options.preset {
        let preset = match preset {
            job::LightingPreset::Neutral => StudioPreset::Neutral,
            job::LightingPreset::WarmCool => StudioPreset::WarmCool,
            job::LightingPreset::KeyRim => StudioPreset::KeyRim,
        };
        return Lighting::preset(preset).map_err(error);
    }
    match &options.lights {
        None => Ok(Lighting::studio_default()),
        Some(inputs) => {
            let mut lights = Vec::new();
            lights.try_reserve_exact(inputs.len()).map_err(|_| error(TraceError::AllocationFailed))?;
            for l in inputs {
                lights.push(
                    DirectionalLight::new(
                        Vec3::new(l.direction[0], l.direction[1], l.direction[2]),
                        Vec3::new(l.color[0], l.color[1], l.color[2]),
                        l.intensity,
                    )
                    .map_err(error)?,
                );
            }
            Lighting::new(lights).map_err(error)
        }
    }
}
type Look = PreparedPrimitive;

pub fn admit(scene: &ProductScene, request: &ImageRequest, options: &RaytraceOptions) -> job::Result<Admission> {
    options.validate()?;
    if let Some(original) = &request.raytrace {
        original.validate()?;
    }
    if request.raytrace.as_ref().unwrap_or(&RaytraceOptions::default()) != options {
        return Err(JobError::new(ErrorCode::InvalidRequest, "raytrace options disagree with image request"));
    }
    if request.integrator != job::Integrator::Raytrace {
        return Err(JobError::new(ErrorCode::InvalidRequest, "raytrace task requires raytrace integrator"));
    }
    if request.transparent_background {
        return Err(invalid("raytrace supports opaque backgrounds only"));
    }
    let mut a = job::image::admit_base(scene, request)?;
    let r = options.reflections.as_ref().filter(|r| !r.bodies.is_empty()).map_or(1, |r| r.samples);
    u64::from(request.width)
        .checked_mul(u64::from(request.height))
        .and_then(|n| n.checked_mul(u64::from(options.primary_samples)))
        .and_then(|n| n.checked_mul(u64::from(r)))
        .ok_or_else(|| invalid("spatial attempt count overflow"))?;
    for body in &scene.meshes {
        if body.opacity != 1.0 || body.faces.iter().any(|f| f.opacity != 1.0) {
            return Err(invalid("raytrace requires opaque body and face appearances"));
        }
    }
    if let Some(reflections) = &options.reflections {
        for override_ in &reflections.bodies {
            let count = scene.meshes.iter().filter(|body| body.name == override_.body).count();
            if count != 1 {
                return Err(JobError::new(ErrorCode::InvalidRequest, "reflection body must resolve to exactly one captured body"));
            }
        }
    }
    anisotropic_faces::prepare(scene, options, || Ok(()))?;
    let n = usize::try_from(a.triangles).map_err(|_| error(TraceError::MemoryLimit))?;
    let extra = BvhBuilder::required_bytes(n, n, limits())
        .map_err(error)?
        .checked_add(a.triangles.checked_mul(std::mem::size_of::<Look>() as u64).ok_or_else(|| error(TraceError::MemoryLimit))?)
        .and_then(|v| {
            v.checked_add(8192 + if options.reflections.is_some() || options.anisotropic_faces.iter().any(Option::is_some) { 32768 } else { 0 })
        })
        .and_then(|v| {
            v.checked_add(options.area_lights.as_ref().map_or(0, Vec::capacity) as u64 * std::mem::size_of::<job::RectangleLightOptions>() as u64)
        })
        .and_then(|v| {
            v.checked_add(options.punctual_lights.as_ref().map_or(0, Vec::capacity) as u64 * std::mem::size_of::<job::PunctualLightOptions>() as u64)
        })
        .ok_or_else(|| error(TraceError::MemoryLimit))?;
    // Keep the conservative raster pixel/codec estimate; add every new owned buffer.
    a.preparation_bytes = a.preparation_bytes.checked_add(extra).ok_or_else(|| error(TraceError::MemoryLimit))?;
    a.estimated_total_bytes = a.estimated_total_bytes.checked_add(extra).ok_or_else(|| error(TraceError::MemoryLimit))?;
    if a.estimated_total_bytes > 384 * 1024 * 1024 {
        return Err(error(TraceError::MemoryLimit));
    }
    Ok(a)
}

struct PendingPixel {
    punctual_index: usize,
    punctual_shadow: bool,
    punctual_ready: bool,
    punctual_value: Vec3,
    area_light: usize,
    area_index: u32,
    area_shadow: bool,
    area_ready: bool,
    area_value: Vec3,

    /// Proof from the primary generator, revoked for any selected reflector.
    /// A jittered/time-varying primary generator MUST supply false.
    repeatable_primary: bool,
    environment_sample: u32,
    environment_sum: Vec3,
    environment_cursor: Option<TraceCursor>,
    environment_value: Vec3,
    ray: Ray,
    cursor: TraceCursor,
    hit: Option<super::Hit>,
    visibility: [bool; 8],
    light: usize,
    bounce: u8,
    throughput: Vec3,
    radiance: Vec3,
}

pub struct ImageTask {
    anisotropic_targets: [Option<PlanarTarget>; 2],
    environment: Option<crate::lighting::environment_input::EnvironmentState>,
    scene: ProductScene,
    request: ImageRequest,
    options: RaytraceOptions,
    cancel: CancelToken,
    admission: Admission,
    lights: Lighting,
    work: WorkBudget,
    clip: (f64, f64),
    triangles: Vec<Triangle>,
    looks: Vec<Look>,
    body: usize,
    triangle: usize,
    face_cursor: usize,
    chosen: Option<crate::FaceAppearance>,
    builder: Option<BvhBuilder>,
    bvh: Option<Bvh>,
    build_used: u64,
    pixels: Vec<u8>,
    pixel: usize,
    sample: u8,
    spatial: u8,
    primary_coverage: u16,
    spatial_radiance: Vec3,
    finish_spatial: bool,
    emit_spatial_pixel: bool,
    pixel_radiance: Vec3,
    pending: Option<PendingPixel>,
    encoder: Option<job::encoding::PngEncoding>,
    done: bool,
}
// Same host slot: a future feature build must prove the actual target layout.
const _: [(); 1] = [(); (std::mem::size_of::<ImageTask>() <= 4096) as usize];

impl ImageTask {
    pub fn new(scene: ProductScene, request: ImageRequest, cancel: CancelToken, options: RaytraceOptions) -> job::Result<Self> {
        Self::new_with_environment(scene, request, cancel, options, None)
    }
    pub fn new_with_environment(
        scene: ProductScene,
        request: ImageRequest,
        cancel: CancelToken,
        options: RaytraceOptions,
        input: Option<crate::lighting::environment_input::EnvironmentInput>,
    ) -> job::Result<Self> {
        cancel.check()?;
        let mut admission = admit(&scene, &request, &options)?;
        if input.is_some() {
            options.validate_environment_reflections(&scene)?;
        }
        let extra = input.as_ref().map_or(Ok(0), |e| e.required_bytes()).map_err(environment_error)?;
        admission.estimated_total_bytes = admission
            .estimated_total_bytes
            .checked_add(extra)
            .filter(|v| *v <= job::image::MAX_JOB_BYTES)
            .ok_or_else(|| error(TraceError::MemoryLimit))?;
        admission.preparation_bytes = admission.preparation_bytes.checked_add(extra).ok_or_else(|| error(TraceError::MemoryLimit))?;
        let environment = input.map(|e| e.begin()).transpose().map_err(environment_error)?;
        let lights = lighting(&options)?;
        let mut work = WorkBudget::new(MAX_WORK).map_err(error)?;
        let anisotropic_targets = anisotropic_faces::prepare(&scene, &options, || work.tick(&cancel).map_err(error))?;
        let radius =
            if scene.meshes.is_empty() { 10.0 } else { scene.bounds.diagonal().max(10.0) + scene.bounds.center().dist(request.camera.target) };
        if !radius.is_finite() {
            return Err(invalid("invalid product bounds"));
        }
        let clip = request.camera.clip(radius);
        let mut triangles = Vec::new();
        let mut looks = Vec::new();
        let mut pixels = Vec::new();
        triangles.try_reserve_exact(admission.triangles as usize).map_err(|_| error(TraceError::AllocationFailed))?;
        looks.try_reserve_exact(admission.triangles as usize).map_err(|_| error(TraceError::AllocationFailed))?;
        pixels.try_reserve_exact(request.width as usize * request.height as usize * 4).map_err(|_| error(TraceError::AllocationFailed))?;
        // Allocator capacities are checked, not assumed equal to requested capacities.
        let actual = BvhBuilder::required_bytes(admission.triangles as usize, triangles.capacity(), limits()).map_err(error)?;
        if actual != BvhBuilder::required_bytes(admission.triangles as usize, admission.triangles as usize, limits()).map_err(error)?
            || looks.capacity() != admission.triangles as usize
            || pixels.capacity() != request.width as usize * request.height as usize * 4
        {
            return Err(error(TraceError::MemoryLimit));
        }
        Ok(Self {
            anisotropic_targets,
            environment,
            scene,
            request,
            options,
            cancel,
            admission,
            lights,
            clip,
            work,
            triangles,
            looks,
            body: 0,
            triangle: 0,
            face_cursor: 0,
            chosen: None,
            builder: None,
            bvh: None,
            build_used: 0,
            pixels,
            pixel: 0,
            sample: 0,
            spatial: 0,
            primary_coverage: 0,
            spatial_radiance: Vec3::ZERO,
            finish_spatial: false,
            emit_spatial_pixel: false,
            pixel_radiance: Vec3::ZERO,
            pending: None,
            encoder: None,
            done: false,
        })
    }
    pub fn progress(&self) -> Progress {
        let (phase, done, total) = if self.done {
            (Phase::Completed, 1, 1)
        } else if let Some(e) = &self.encoder {
            (Phase::Encoding, e.row() as u64, self.request.height as u64)
        } else if self.bvh.is_some() {
            (Phase::Running, self.pixel as u64, self.request.width as u64 * self.request.height as u64)
        } else {
            (Phase::Preparing, self.triangles.len() as u64, self.admission.triangles)
        };
        Progress { phase, done, total }
    }
    fn prepare_one(&mut self) -> job::Result<()> {
        self.work.tick(&self.cancel).map_err(error)?;
        let Some(body) = self.scene.meshes.get(self.body) else {
            let triangles = std::mem::take(&mut self.triangles);
            let expected = BvhBuilder::required_bytes(triangles.len(), triangles.capacity(), limits()).map_err(error)?;
            let builder = BvhBuilder::new(triangles, limits(), &self.cancel).map_err(error)?;
            if builder.admitted_bytes() > expected {
                return Err(error(TraceError::MemoryLimit));
            }
            self.builder = Some(builder);
            return Ok(());
        };
        let Some(indices) = body.mesh.triangles.get(self.triangle) else {
            self.body += 1;
            self.triangle = 0;
            return Ok(());
        };
        let mut vertices = [Vec3::ZERO; 3];
        let mut normals = [Vec3::ZERO; 3];
        for ((p, n), i) in vertices.iter_mut().zip(normals.iter_mut()).zip(indices) {
            *p = *body.mesh.positions.get(*i as usize).ok_or_else(|| invalid("triangle vertex index"))?;
            *n = body.mesh.normals.get(*i as usize).copied().unwrap_or(Vec3::ZERO);
        }
        let face = body.mesh.tri_face.get(self.triangle).copied();
        let mut color = body.color;
        let mut surface = body.surface;
        if let Some(look) = body.faces.get(self.face_cursor) {
            self.face_cursor += 1;
            if Some(look.face) == face {
                self.chosen = Some(look.clone());
                self.face_cursor = body.faces.len();
            } else {
                return Ok(());
            }
        }
        if let Some(look) = self.chosen.take() {
            color = look.color;
            surface = look.surface;
        }
        self.face_cursor = 0;
        let surface = match surface {
            Some(s) => s,
            None => OpaqueSurface::new(Vec3::new(srgb8_to_linear(color[0]), srgb8_to_linear(color[1]), srgb8_to_linear(color[2])), 0.0, 0.7, 1.5)
                .map_err(|_| invalid("invalid scalar surface"))?,
        };
        self.triangles.push(Triangle::new(vertices).map_err(error)?);
        self.looks.push(Look { body_index: self.body, face, surface, normals, vertices });
        self.triangle += 1;
        Ok(())
    }
    fn sample_count(&self) -> u8 {
        self.options.reflections.as_ref().filter(|r| !r.bodies.is_empty()).map_or(1, |r| r.samples)
    }
    fn finish_sample(&mut self, radiance: Vec3) -> job::Result<()> {
        if self.options.primary_samples != 1 {
            return self.finish_spatial_sample(radiance);
        }
        // Only the current fixed-centre, nonselected primary path is identical
        // across reflection samples: directional inputs and the environment's
        // fixed u-grid do not depend on self.sample, and this path never bounces.
        // Secondary nonselected hits and selected null samples still require R.
        let repeats =
            if self.sample == 0 && self.pending.as_ref().is_some_and(|p| p.repeatable_primary && p.bounce == 0) { self.sample_count() } else { 1 };
        // Reuse the shading result, but retain the original addition order,
        // per-add bound and final divisor exactly. At most64 scalar additions,
        // no repeated BVH/BSDF/environment work or new allocation/work budget.
        for _ in 0..repeats {
            self.pixel_radiance += radiance;
            if !self.pixel_radiance.is_finite()
                || [self.pixel_radiance.x, self.pixel_radiance.y, self.pixel_radiance.z].into_iter().any(|v| v < 0.0 || v > 1e12)
            {
                return Err(invalid("reflection radiance exceeds finite transport bound"));
            }
            self.sample += 1;
        }
        self.pending = None;
        if self.sample == self.sample_count() {
            let display = map_to_display(self.pixel_radiance / self.sample as f64, self.options.exposure_ev).map_err(error)?;
            self.pixels.extend_from_slice(&[encode(display.x), encode(display.y), encode(display.z), 255]);
            self.pixel += 1;
            self.sample = 0;
            self.pixel_radiance = Vec3::ZERO;
        }
        Ok(())
    }
    fn finish_spatial_sample(&mut self, radiance: Vec3) -> job::Result<()> {
        // Same per-R-add finite bound and denominator, including zero/null events.
        self.pixel_radiance += radiance;
        if !bounded_radiance(self.pixel_radiance) {
            return Err(invalid("reflection radiance exceeds finite transport bound"));
        }
        self.sample = self.sample.checked_add(1).ok_or_else(|| invalid("reflection sample overflow"))?;
        if self.sample > self.sample_count() {
            return Err(invalid("reflection sample count exceeded"));
        }
        self.pending = None;
        self.finish_spatial = self.sample == self.sample_count();
        Ok(())
    }
    fn spatial_transition(&mut self) -> job::Result<bool> {
        if self.finish_spatial {
            self.work.tick(&self.cancel).map_err(error)?;
            self.spatial_radiance += self.pixel_radiance / f64::from(self.sample_count()) / f64::from(self.options.primary_samples);
            if !bounded_radiance(self.spatial_radiance) {
                return Err(invalid("spatial radiance exceeds finite transport bound"));
            }
            self.pixel_radiance = Vec3::ZERO;
            self.sample = 0;
            self.spatial = self.spatial.checked_add(1).ok_or_else(|| invalid("spatial sample overflow"))?;
            self.finish_spatial = false;
            self.emit_spatial_pixel = self.spatial == self.options.primary_samples;
            return Ok(true);
        }
        if self.emit_spatial_pixel {
            self.work.tick(&self.cancel).map_err(error)?;
            let attempts = u16::from(self.options.primary_samples)
                .checked_mul(u16::from(self.sample_count()))
                .filter(|n| *n <= 1024)
                .ok_or_else(|| invalid("spatial coverage bound"))?;
            let rgba = composite_spatial(self.spatial_radiance, self.primary_coverage, attempts, self.request.background, self.options.exposure_ev)?;
            self.cancel.check()?;
            self.pixels.extend_from_slice(&rgba);
            self.pixel += 1;
            self.spatial = 0;
            self.primary_coverage = 0;
            self.spatial_radiance = Vec3::ZERO;
            self.emit_spatial_pixel = false;
            return Ok(true);
        }
        Ok(false)
    }
    fn pixel_one(&mut self) -> job::Result<()> {
        if self.options.primary_samples != 1 && self.spatial_transition()? {
            return Ok(());
        }
        let bvh = self.bvh.as_ref().ok_or_else(|| invalid("missing image BVH"))?;
        if self.pending.is_none() {
            self.work.tick(&self.cancel).map_err(error)?;
            let width = self.request.width as usize;
            // This proof bit belongs to this fixed-centre generator, not to the
            // camera or request globally. Future jitter/AA must return false.
            let (ray, repeatable_primary) = if self.options.primary_samples == 1 {
                // Preserve the exact legacy centre path, including its reuse proof.
                (primary_ray(&self.request, (self.pixel % width) as f64 + 0.5, (self.pixel / width) as f64 + 0.5, self.clip).map_err(error)?, true)
            } else {
                let [dx, dy] = primary_offset(self.options.primary_samples, self.spatial)?;
                (primary_ray(&self.request, (self.pixel % width) as f64 + dx, (self.pixel / width) as f64 + dy, self.clip).map_err(error)?, false)
            };
            self.pending = Some(PendingPixel {
                punctual_index: 0,
                punctual_shadow: false,
                punctual_ready: false,
                punctual_value: Vec3::ZERO,
                area_light: 0,
                area_index: 0,
                area_shadow: false,
                area_ready: false,
                area_value: Vec3::ZERO,

                repeatable_primary,
                environment_sample: 0,
                environment_sum: Vec3::ZERO,
                environment_cursor: None,
                environment_value: Vec3::ZERO,
                ray,
                cursor: TraceCursor::new(ray, false),
                hit: None,
                visibility: [true; 8],
                light: 0,
                bounce: 0,
                throughput: Vec3::new(1.0, 1.0, 1.0),
                radiance: Vec3::ZERO,
            });
            return Ok(());
        }
        let scene = PreparedScene { bvh, primitives: &self.looks, source: &self.scene };
        let pending = self.pending.as_mut().ok_or_else(|| invalid("missing pixel state"))?;
        if pending.hit.is_none() {
            let Some(hit) = pending.cursor.step(bvh, &mut self.work, &self.cancel).map_err(error)? else {
                return Ok(());
            };
            let Some(hit) = hit else {
                if pending.bounce == 0 {
                    if self.options.primary_samples != 1 {
                        // A miss is one of S*R attempts, never the completed pixel.
                        return self.finish_sample(Vec3::ZERO);
                    }
                    let [r, g, b] = self.request.background;
                    self.pixels.extend_from_slice(&[r, g, b, 255]);
                    self.pixel += 1;
                    self.sample = 0;
                    self.pixel_radiance = Vec3::ZERO;
                    self.pending = None;
                    return Ok(());
                }
                if let Some(environment) = &self.environment {
                    let emitted =
                        environment.view().map_err(environment_error)?.radiance_linear_srgb(pending.ray.direction()).map_err(environment_error)?;
                    pending.radiance += multiply(pending.throughput, emitted);
                }
                let radiance = pending.radiance;
                return self.finish_sample(radiance);
            };
            if pending.bounce == 0 && self.options.primary_samples != 1 {
                self.primary_coverage =
                    self.primary_coverage.checked_add(1).filter(|n| *n <= 1024).ok_or_else(|| invalid("spatial coverage overflow"))?;
            }
            pending.hit = Some(hit);
            pending.light = 0;
            let shared = scene.resolve_hit(pending.ray, hit).map_err(error)?;
            let mode =
                self.options.reflections.as_ref().and_then(|r| r.bodies.iter().find(|entry| entry.body == shared.body_name).map(|entry| entry.mode));
            pending.repeatable_primary &= pending.bounce == 0 && mode.is_none();
            if self.options.shadows && !self.lights.lights().is_empty() && mode != Some(job::ReflectionMode::Mirror) {
                let light = self.lights.lights().first().ok_or_else(|| invalid("missing first light"))?;
                pending.cursor = TraceCursor::new(shared.spawn(light.direction()).map_err(error)?, true);
                return Ok(());
            }
            pending.light = self.lights.lights().len();
            return Ok(());
        } else if pending.light < self.lights.lights().len() {
            let Some(shadow) = pending.cursor.step(bvh, &mut self.work, &self.cancel).map_err(error)? else {
                return Ok(());
            };
            *pending.visibility.get_mut(pending.light).ok_or_else(|| invalid("light visibility index"))? = shadow.is_none();
            pending.light += 1;
            if let Some(light) = self.lights.lights().get(pending.light) {
                let shared = scene.resolve_hit(pending.ray, pending.hit.ok_or_else(|| invalid("missing shadow hit"))?).map_err(error)?;
                pending.cursor = TraceCursor::new(shared.spawn(light.direction()).map_err(error)?, true);
                return Ok(());
            }
            return Ok(());
        }
        if pending.punctual_index < self.options.punctual_lights.as_ref().map_or(0, Vec::len) {
            let shared = scene.resolve_hit(pending.ray, pending.hit.ok_or_else(|| invalid("punctual hit missing"))?).map_err(error)?;
            let mode =
                self.options.reflections.as_ref().and_then(|r| r.bodies.iter().find(|entry| entry.body == shared.body_name).map(|entry| entry.mode));
            let face = self.looks.get(shared.primitive_index).ok_or_else(|| invalid("punctual face missing"))?.face;
            let target = anisotropic_faces::target_index(&self.options, shared.body_index, face)
                .map(|i| self.anisotropic_targets.get(i).copied().flatten().ok_or_else(|| invalid("punctual anisotropic target missing")))
                .transpose()?;
            punctual_one(pending, &self.options, bvh, (shared, target, mode), &mut self.work, &self.cancel)?;
            return Ok(());
        }
        if pending.area_light < self.options.area_lights.as_ref().map_or(0, Vec::len) {
            let shared = scene.resolve_hit(pending.ray, pending.hit.ok_or_else(|| invalid("area hit missing"))?).map_err(error)?;
            let mode =
                self.options.reflections.as_ref().and_then(|r| r.bodies.iter().find(|entry| entry.body == shared.body_name).map(|entry| entry.mode));
            let face = self.looks.get(shared.primitive_index).ok_or_else(|| invalid("area face missing"))?.face;
            let target = anisotropic_faces::target_index(&self.options, shared.body_index, face)
                .map(|i| self.anisotropic_targets.get(i).copied().flatten().ok_or_else(|| invalid("area anisotropic target missing")))
                .transpose()?;
            area_one(pending, &self.options, bvh, (shared, target, mode), &mut self.work, &self.cancel, self.pixel as u64)?;
            return Ok(());
        }
        if let Some(cursor) = &mut pending.environment_cursor {
            let Some(blocker) = cursor.step(bvh, &mut self.work, &self.cancel).map_err(error)? else {
                return Ok(());
            };
            if blocker.is_none() {
                pending.environment_sum += pending.environment_value;
            }
            pending.environment_cursor = None;
            pending.environment_sample += 1;
            return Ok(());
        }
        if pending.light == self.lights.lights().len() {
            self.work.tick(&self.cancel).map_err(error)?;
            let shared = scene.resolve_hit(pending.ray, pending.hit.ok_or_else(|| invalid("missing shading hit"))?).map_err(error)?;
            let mode =
                self.options.reflections.as_ref().and_then(|r| r.bodies.iter().find(|entry| entry.body == shared.body_name).map(|entry| entry.mode));
            let view = -pending.ray.direction();
            let geometric = shared.oriented_geometric_normal();
            // Disjoint estimators: selected reflectors use BSDF transport to the environment;
            // only nonselected opaque hits use environment next-event estimates.
            if mode.is_none()
                && let Some(environment) = &self.environment
            {
                if pending.environment_sample < environment.samples {
                    let side = match environment.samples {
                        1 => 1,
                        4 => 2,
                        16 => 4,
                        _ => 8,
                    };
                    let i = pending.environment_sample;
                    let u = if side == 1 {
                        [0.25, 0.375]
                    } else {
                        [(f64::from(i / side) + 0.5) / f64::from(side), (f64::from(i % side) + 0.5) / f64::from(side)]
                    };
                    let sample = environment
                        .view()
                        .map_err(environment_error)?
                        .direct_sample_linear_srgb(u, shared.surface, shared.shading_normal, geometric, view)
                        .map_err(environment_error)?;
                    pending.environment_value = sample.contribution_linear_srgb;
                    if self.options.shadows && sample.contribution_linear_srgb.len2() > 0.0 {
                        pending.environment_cursor = Some(TraceCursor::new(shared.spawn(sample.direction).map_err(error)?, true));
                    } else {
                        pending.environment_sum += pending.environment_value;
                        pending.environment_sample += 1;
                    }
                    return Ok(());
                }
                pending.radiance += multiply(pending.throughput, pending.environment_sum / f64::from(environment.samples));
            }
            let face = self.looks.get(shared.primitive_index).ok_or_else(|| invalid("missing captured face"))?.face;
            let target_index = anisotropic_faces::target_index(&self.options, shared.body_index, face);
            if mode != Some(job::ReflectionMode::Mirror) {
                let visible = |index: usize, light: Vec3| {
                    Ok(geometric.dot(light) > 0.0 && *pending.visibility.get(index).ok_or(TraceError::Internal("light visibility"))?)
                };
                let direct = if let Some(index) = target_index {
                    self.anisotropic_targets
                        .get(index)
                        .copied()
                        .flatten()
                        .ok_or_else(|| invalid("missing certified anisotropic face"))?
                        .shade(&self.lights, shared.surface, geometric, view, visible)
                        .map_err(error)?
                } else {
                    self.lights.shade_linear(shared.surface, shared.shading_normal, view, visible).map_err(error)?
                };
                pending.radiance += multiply(pending.throughput, direct);
            }
            let max_bounces = self.options.reflections.as_ref().map_or(0, |r| r.max_bounces);
            if let Some(mode) = mode.filter(|_| pending.bounce < max_bounces) {
                let samples = self.options.reflections.as_ref().map_or(1, |r| u64::from(r.samples));
                let u = if self.options.primary_samples == 1 {
                    reflection::variates(self.pixel as u64, self.sample as u64, pending.bounce as u64, samples).map_err(error)?
                } else {
                    let key = spatial_key(self.pixel as u64, self.spatial, self.options.primary_samples)?;
                    reflection::variates(key, self.sample as u64, pending.bounce as u64, samples).map_err(error)?
                };
                let sampled = match (mode, target_index) {
                    (job::ReflectionMode::Mirror, _) => reflection::mirror(shared.surface, shared.shading_normal, view),
                    (job::ReflectionMode::RoughSpecular, Some(index)) => self
                        .anisotropic_targets
                        .get(index)
                        .copied()
                        .flatten()
                        .ok_or_else(|| invalid("missing anisotropic reflection target"))?
                        .sample(shared.surface, geometric, view, u),
                    (job::ReflectionMode::RoughSpecular, None) => reflection::rough_specular(shared.surface, shared.shading_normal, view, u),
                }
                .map_err(error)?;
                if let Some(sample) = sampled.filter(|s| s.direction.dot(geometric) > 0.0) {
                    pending.throughput = multiply(pending.throughput, sample.weight);
                    if !pending.throughput.is_finite() {
                        return Err(invalid("nonfinite reflection throughput"));
                    }
                    if pending.throughput.len2() > 0.0 {
                        pending.ray = shared.spawn(sample.direction).map_err(error)?;
                        pending.cursor = TraceCursor::new(pending.ray, false);
                        pending.hit = None;
                        pending.light = 0;
                        pending.visibility = [true; 8];
                        pending.punctual_index = 0;
                        pending.punctual_shadow = false;
                        pending.punctual_ready = false;
                        pending.punctual_value = Vec3::ZERO;
                        pending.area_light = 0;
                        pending.area_index = 0;
                        pending.area_shadow = false;
                        pending.area_ready = false;
                        pending.area_value = Vec3::ZERO;

                        pending.environment_sample = 0;
                        pending.environment_sum = Vec3::ZERO;
                        pending.environment_cursor = None;
                        pending.environment_value = Vec3::ZERO;
                        pending.bounce += 1;
                        return Ok(());
                    }
                }
            }
            let radiance = pending.radiance;
            return self.finish_sample(radiance);
        }
        Ok(())
    }
    pub fn step(&mut self, units: usize) -> job::Result<TaskPoll> {
        self.cancel.check()?;
        if units == 0 || units > 16384 || self.done {
            return Err(JobError::new(ErrorCode::InvalidRequest, "invalid raytrace step or completed task"));
        }
        for _ in 0..units.min(4) {
            if let Some(encoder) = &mut self.encoder {
                if let Some(png) = encoder.step(&self.pixels)? {
                    self.cancel.check()?;
                    self.done = true;
                    self.anisotropic_targets = [None, None];
                    self.options.anisotropic_faces = [None, None];
                    self.environment = None;
                    self.scene.meshes = Vec::new();
                    self.looks = Vec::new();
                    self.bvh = None;
                    self.builder = None;
                    self.triangles = Vec::new();
                    self.pixels = Vec::new();
                    self.pending = None;
                    self.request.raytrace = None;
                    self.options.lights = None;
                    self.options.punctual_lights = None;
                    self.options.area_lights = None;
                    self.options.reflections = None;
                    self.lights = Lighting::new(Vec::new()).map_err(error)?;
                    self.encoder = None;
                    return Ok(TaskPoll::Complete(png));
                }
            } else if self.environment.as_ref().is_some_and(|e| !e.ready()) {
                self.work.tick(&self.cancel).map_err(error)?;
                self.environment
                    .as_mut()
                    .ok_or_else(|| invalid("missing environment preparation"))?
                    .prepare_one(|| self.cancel.is_cancelled())
                    .map_err(environment_error)?;
            } else if self.bvh.is_some() {
                if self.pixel == self.request.width as usize * self.request.height as usize {
                    self.encoder = Some(job::encoding::PngEncoding::new(
                        self.request.width,
                        self.request.height,
                        self.admission.encoded_limit,
                        self.cancel.clone(),
                    )?);
                } else {
                    self.pixel_one()?;
                }
            } else if let Some(builder) = &mut self.builder {
                let progress = builder.step(1, &self.cancel).map_err(error)?;
                for _ in self.build_used..progress.work_used {
                    self.work.tick(&self.cancel).map_err(error)?;
                }
                self.build_used = progress.work_used;
                if progress.complete {
                    self.bvh = Some(self.builder.take().ok_or_else(|| invalid("missing builder"))?.finish().map_err(error)?);
                }
            } else {
                self.prepare_one()?;
            }
        }
        Ok(TaskPoll::Pending(self.progress()))
    }
}
fn bounded_radiance(v: Vec3) -> bool {
    v.is_finite() && [v.x, v.y, v.z].into_iter().all(|c| (0.0..=1e12).contains(&c))
}
fn primary_offset(samples: u8, ordinal: u8) -> job::Result<[f64; 2]> {
    let side = match samples {
        1 => 1,
        4 => 2,
        16 => 4,
        _ => return Err(invalid("invalid spatial sample count")),
    };
    if ordinal >= samples {
        return Err(invalid("invalid spatial sample ordinal"));
    }
    Ok([(f64::from(ordinal % side) + 0.5) / f64::from(side), (f64::from(ordinal / side) + 0.5) / f64::from(side)])
}
fn spatial_key(pixel: u64, ordinal: u8, samples: u8) -> job::Result<u64> {
    primary_offset(samples, ordinal)?;
    pixel
        .checked_mul(u64::from(samples))
        .and_then(|n| n.checked_add(u64::from(ordinal)))
        .map(|n| n ^ 0x4141_5052_494d_4152)
        .ok_or_else(|| invalid("spatial seed overflow"))
}
fn composite_spatial(premultiplied: Vec3, covered: u16, attempts: u16, background: [u8; 3], exposure: f64) -> job::Result<[u8; 4]> {
    if attempts == 0 || attempts > 1024 || covered > attempts || !bounded_radiance(premultiplied) {
        return Err(invalid("invalid spatial coverage/radiance"));
    }
    if covered == 0 {
        if premultiplied != Vec3::ZERO {
            return Err(invalid("uncovered spatial radiance"));
        }
        return Ok([background[0], background[1], background[2], 255]);
    }
    let coverage = f64::from(covered) / f64::from(attempts);
    let geometry = map_to_display(premultiplied / coverage, exposure).map_err(error)?;
    let display = if covered == attempts {
        geometry
    } else {
        let bg = Vec3::new(srgb8_to_linear(background[0]), srgb8_to_linear(background[1]), srgb8_to_linear(background[2]));
        geometry * coverage + bg * (1.0 - coverage)
    };
    Ok([encode(display.x), encode(display.y), encode(display.z), 255])
}

/// One sample preparation, one traversal operation OR one visible-sum operation.
/// Reuses the existing primary/directional cursor: no second task or heap state.
fn punctual_one(
    pending: &mut PendingPixel,
    options: &RaytraceOptions,
    bvh: &Bvh,
    context: (super::prepared::SharedHit<'_>, Option<PlanarTarget>, Option<job::ReflectionMode>),
    work: &mut WorkBudget,
    cancel: &CancelToken,
) -> job::Result<()> {
    let (shared, target, mode) = context;
    if pending.punctual_shadow {
        if let Some(blocker) = pending.cursor.step(bvh, work, cancel).map_err(error)? {
            if blocker.is_some() {
                pending.punctual_value = Vec3::ZERO;
            }
            pending.punctual_shadow = false;
            pending.punctual_ready = true;
        }
        return Ok(());
    }
    work.tick(cancel).map_err(error)?;
    if pending.punctual_ready {
        pending.radiance += multiply(pending.throughput, pending.punctual_value);
        super::lighting::validate_radiance(pending.radiance).map_err(error)?;
        pending.punctual_index += 1;
        pending.punctual_ready = false;
        return Ok(());
    }
    let inputs = options.punctual_lights.as_ref().ok_or_else(|| invalid("punctual options missing"))?;
    if mode == Some(job::ReflectionMode::Mirror) {
        pending.punctual_index = inputs.len();
        return Ok(());
    }
    let light = inputs.get(pending.punctual_index).copied().ok_or_else(|| invalid("punctual source index"))?.light().map_err(error)?;
    let sample = light.sample(shared.position).map_err(error)?;
    let geometric = shared.oriented_geometric_normal();
    let cosine = if target.is_some() { geometric } else { shared.shading_normal }.dot(sample.direction).max(0.0);
    let contribution = if geometric.dot(sample.direction) <= 0.0 || cosine == 0.0 || sample.incident_linear_srgb == Vec3::ZERO {
        Vec3::ZERO
    } else {
        let view = -pending.ray.direction();
        let f = if let Some(target) = target {
            target.direct_brdf(shared.surface, geometric, view, sample.direction).map_err(error)?
        } else {
            shared.surface.brdf(shared.shading_normal, view, sample.direction).map_err(|_| invalid("punctual opaque response"))?
        };
        multiply(f, sample.incident_linear_srgb) * cosine
    };
    super::lighting::validate_radiance(contribution).map_err(error)?;
    pending.punctual_value = contribution;
    if options.shadows && contribution != Vec3::ZERO {
        pending.cursor = TraceCursor::new(sample.shadow_ray(shared).map_err(error)?, true);
        pending.punctual_shadow = true;
    } else {
        pending.radiance += multiply(pending.throughput, contribution);
        super::lighting::validate_radiance(pending.radiance).map_err(error)?;
        pending.punctual_index += 1;
    }
    Ok(())
}

fn multiply(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x * b.x, a.y * b.y, a.z * b.z)
}
fn encode(c: f64) -> u8 {
    let s = if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
fn shadow_ray(ray: Ray, hit: super::Hit, light: Vec3, vertices: [Vec3; 3]) -> super::Result<Ray> {
    let mut n = hit.geometry.geometric_normal;
    if n.dot(light) < 0.0 {
        n = -n;
    }
    let [a, b, c] = hit.geometry.barycentric;
    let [va, vb, vc] = vertices;
    // Reconstruct from captured vertices, not a distant camera. Component-wise
    // rounding bounds avoid an unrelated large X translation displacing a Z plane.
    let p = va * a + vb * b + vc * c;
    let magnitude = |x: f64, y: f64, z: f64| x.abs() * a.abs() + y.abs() * b.abs() + z.abs() * c.abs();
    let e = Vec3::new(magnitude(va.x, vb.x, vc.x), magnitude(va.y, vb.y, vc.y), magnitude(va.z, vb.z, vc.z)) * (8.0 * f64::EPSILON);
    let distance = n.x.abs() * e.x + n.y.abs() * e.y + n.z.abs() * e.z;
    let shifted = p + n * distance;
    let outward = |v: f64, n: f64| {
        if n > 0.0 {
            v.next_up()
        } else if n < 0.0 {
            v.next_down()
        } else {
            v
        }
    };
    let _ = ray; // hit position comes from geometry, independent of camera distance.
    Ray::new(Vec3::new(outward(shifted.x, n.x), outward(shifted.y, n.y), outward(shifted.z, n.z)), light, 0.0_f64.next_up(), super::MAX_DISTANCE_MM)
}

fn primary_ray(request: &ImageRequest, x: f64, y: f64, clip: (f64, f64)) -> super::Result<Ray> {
    let camera = &request.camera;
    let (mut origin, direction) = camera.ray(x, y, request.width as f64, request.height as f64);
    if camera.fov == 0.0 {
        origin -= camera.back() * (camera.distance * 3.0);
    }
    let forward = -camera.back();
    let cosine = direction.dot(forward);
    if !cosine.is_finite() || cosine <= 0.0 {
        return Err(TraceError::InvalidInput("camera ray direction"));
    }
    let depth = (origin - camera.eye()).dot(forward);
    let start = (clip.0 - depth) / cosine;
    let length = (clip.1 - clip.0) / cosine;
    Ray::new(origin + direction * start, direction, 0.0, length)
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "deterministic_primary_tests.rs"]
mod deterministic_primary_tests;

#[cfg(test)]
#[path = "reflection_image_tests.rs"]
mod reflection_tests;

fn environment_error(e: crate::lighting::HdrError) -> JobError {
    use crate::lighting::HdrErrorCode;
    let code = match e.code {
        HdrErrorCode::Cancelled => ErrorCode::Cancelled,
        HdrErrorCode::LimitExceeded => ErrorCode::LimitExceeded,
        HdrErrorCode::AllocationFailed => ErrorCode::AllocationFailed,
        _ => ErrorCode::InvalidRequest,
    };
    JobError::new(code, e.to_string())
}

#[cfg(test)]
#[path = "primary_aa_tests.rs"]
mod primary_aa_tests;

// One bounded continuation operation, using the same cursor, work accumulator and task.
fn area_one(
    pending: &mut PendingPixel,
    options: &RaytraceOptions,
    bvh: &Bvh,
    context: (super::prepared::SharedHit<'_>, Option<PlanarTarget>, Option<job::ReflectionMode>),
    work: &mut WorkBudget,
    cancel: &CancelToken,
    pixel: u64,
) -> job::Result<()> {
    let (shared, target, mode) = context;
    if pending.area_shadow {
        if let Some(blocker) = pending.cursor.step(bvh, work, cancel).map_err(error)? {
            if blocker.is_some() {
                pending.area_value = Vec3::ZERO;
            }
            pending.area_shadow = false;
            pending.area_ready = true;
        }
        return Ok(());
    }
    work.tick(cancel).map_err(error)?;
    if pending.area_ready {
        pending.radiance += multiply(pending.throughput, pending.area_value);
        super::lighting::validate_radiance(pending.radiance).map_err(error)?;
        pending.area_index += 1;
        pending.area_ready = false;
        return Ok(());
    }
    let inputs = options.area_lights.as_ref().ok_or_else(|| invalid("area options missing"))?;
    if mode == Some(job::ReflectionMode::Mirror) {
        pending.area_light = inputs.len();
        return Ok(());
    }
    let count = options.area_samples.unwrap_or(16);
    if pending.area_index == count {
        pending.area_light += 1;
        pending.area_index = 0;
        return Ok(());
    }
    let input = inputs.get(pending.area_light).ok_or_else(|| invalid("area source index"))?;
    let vec = |[x, y, z]: [f64; 3]| Vec3::new(x, y, z);
    let light = super::area::RectangleLight::new(vec(input.center_mm), vec(input.edge_u_mm), vec(input.edge_v_mm), vec(input.color), input.intensity)
        .map_err(error)?;
    let Some(sample) =
        light.sample(shared.position, pixel, pending.area_light as u32, pending.area_index, count, options.seed.unwrap_or(0)).map_err(error)?
    else {
        pending.area_index += 1;
        return Ok(());
    };
    let geometric = shared.oriented_geometric_normal();
    let cosine = if target.is_some() { geometric } else { shared.shading_normal }.dot(sample.direction()).max(0.0);
    let contribution = if geometric.dot(sample.direction()) <= 0.0 || cosine == 0.0 {
        Vec3::ZERO
    } else {
        let view = -pending.ray.direction();
        let f = if let Some(target) = target {
            target.direct_brdf(shared.surface, geometric, view, sample.direction()).map_err(error)?
        } else {
            shared.surface.brdf(shared.shading_normal, view, sample.direction()).map_err(|_| invalid("area opaque response"))?
        };
        multiply(f, sample.incident()) * cosine
    };
    super::lighting::validate_radiance(contribution).map_err(error)?;
    pending.area_value = contribution;
    if options.shadows && contribution != Vec3::ZERO {
        let origin = shared.spawn(sample.direction()).map_err(error)?.origin();
        pending.cursor = TraceCursor::new(sample.shadow_ray(origin).map_err(error)?, true);
        pending.area_shadow = true;
    } else {
        pending.radiance += multiply(pending.throughput, contribution);
        super::lighting::validate_radiance(pending.radiance).map_err(error)?;
        pending.area_index += 1;
    }
    Ok(())
}

#[cfg(test)]
#[path = "punctual_image_tests.rs"]
mod punctual_image_tests;
