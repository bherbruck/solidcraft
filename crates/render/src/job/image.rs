//! Incremental product rasterization. Every phase yields between bounded work units.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use solidcraft_geom::Vec3;

use super::{CancelToken, ErrorCode, ImageRequest, JobError, Phase, Progress, Result};
use crate::{FaceAppearance, Mat4, ProductScene, RenderFormat, RenderMode, RenderOperation, RenderRequest};

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;

pub const MAX_TRIANGLES: u64 = 262_144;
pub const MAX_GEOMETRY_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_JOB_BYTES: u64 = 384 * 1024 * 1024;
pub const MAX_PIXEL_TESTS: u64 = 128_000_000;
const MAX_STEP: usize = 16_384;
const SS: usize = 2;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Admission {
    pub triangles: u64,
    pub face_appearances: u64,
    pub geometry_bytes: u64,
    pub pixel_scratch_bytes: u64,
    pub preparation_bytes: u64,
    pub estimated_total_bytes: u64,
    pub encoded_limit: u64,
}

fn limit(message: &str) -> JobError {
    JobError::new(ErrorCode::LimitExceeded, message)
}
fn invalid(message: &str) -> JobError {
    JobError::new(ErrorCode::InvalidScene, message)
}
fn add_bytes(sum: &mut u64, count: usize, size: usize) -> Result<()> {
    let bytes = (count as u64).checked_mul(size as u64).ok_or_else(|| limit("geometry size overflow"))?;
    *sum = sum.checked_add(bytes).ok_or_else(|| limit("geometry size overflow"))?;
    if *sum > MAX_GEOMETRY_BYTES {
        return Err(limit("retained product geometry exceeds 128 MiB"));
    }
    Ok(())
}

/// Conservative admission before allocating pixels or projected-work lists. Source Arcs are
/// counted even when another owner already holds them. Allocator and codec internals are estimates.
/// Includes the CPU raster work estimate and its 384 MiB per-job check.
pub fn admit(scene: &ProductScene, request: &ImageRequest) -> Result<Admission> {
    admit_raster(admit_source(scene, request, MAX_TRIANGLES)?)
}

/// Common camera, dimension and source admission for registered executors, without any CPU
/// raster estimate: the returned admission counts retained source geometry only
/// (`pixel_scratch_bytes` and `preparation_bytes` are zero). Executors budget their own
/// preparation and pixel storage, apply their own job limit, and validate triangle contents
/// before uploading anything to a device.
pub fn admit_geometry(scene: &ProductScene, request: &ImageRequest, max_triangles: u64) -> Result<Admission> {
    Ok(admit_source(scene, request, max_triangles)?.admission)
}

/// A validated source with its geometry accounting, and the builtin raster scratch estimate
/// for the requested dimensions (used only by the CPU estimate).
struct Source {
    admission: Admission,
    bodies: u64,
    raster_scratch_bytes: u64,
}

fn admit_source(scene: &ProductScene, request: &ImageRequest, max_triangles: u64) -> Result<Source> {
    if !request.camera.is_valid() {
        return Err(JobError::new(ErrorCode::InvalidRequest, "invalid image camera"));
    }
    if scene.meshes.len() > 16_384 {
        return Err(limit("too many product bodies"));
    }
    let resolved = crate::resolve_request(
        &[crate::builtin_backend()],
        &RenderRequest {
            backend: None,
            mode: RenderMode::Preview,
            operation: RenderOperation::Image,
            width: u64::from(request.width),
            height: u64::from(request.height),
            format: Some(RenderFormat::Png),
            features: Vec::new(),
            policy: Default::default(),
        },
    )
    .map_err(|e| JobError::new(ErrorCode::InvalidRequest, e.to_string()))?;
    let (mut triangles, mut faces, mut geometry, mut polylines) = (0u64, 0u64, 0u64, 0u64);
    add_bytes(&mut geometry, scene.meshes.capacity(), std::mem::size_of::<crate::ProductMesh>())?;
    for body in &scene.meshes {
        let m = &body.mesh;
        triangles = triangles.checked_add(m.triangles.len() as u64).ok_or_else(|| limit("triangle count overflow"))?;
        faces = faces.checked_add(body.faces.len() as u64).ok_or_else(|| limit("appearance count overflow"))?;
        polylines = polylines.checked_add(m.edges.len() as u64 + m.edge_faces.len() as u64).ok_or_else(|| limit("edge count overflow"))?;
        if triangles > max_triangles || faces > max_triangles || polylines > max_triangles {
            return Err(limit("product image exceeds its triangle/appearance/polyline admission cap"));
        }
        add_bytes(&mut geometry, m.positions.capacity(), std::mem::size_of::<Vec3>())?;
        add_bytes(&mut geometry, m.normals.capacity(), std::mem::size_of::<Vec3>())?;
        add_bytes(&mut geometry, m.triangles.capacity(), std::mem::size_of::<[u32; 3]>())?;
        add_bytes(&mut geometry, m.tri_face.capacity(), std::mem::size_of::<u32>())?;
        add_bytes(&mut geometry, body.faces.capacity(), std::mem::size_of::<FaceAppearance>())?;
        add_bytes(&mut geometry, body.name.capacity(), 1)?;
        add_bytes(&mut geometry, m.edges.capacity(), std::mem::size_of::<Vec<Vec3>>())?;
        add_bytes(&mut geometry, m.edge_faces.capacity(), std::mem::size_of::<Vec<u32>>())?;
        for edge in &m.edges {
            add_bytes(&mut geometry, edge.capacity(), std::mem::size_of::<Vec3>())?;
        }
        for edge in &m.edge_faces {
            add_bytes(&mut geometry, edge.capacity(), std::mem::size_of::<u32>())?;
        }
        for count in [m.seams.capacity(), m.smooth_edges.capacity(), m.sharp_edges.capacity()] {
            add_bytes(&mut geometry, count, 1)?;
        }
        if body.surface.is_some() && body.opacity != 1.0 {
            return Err(invalid("opaque PBR requires opacity 1"));
        }
        if !body.opacity.is_finite() || !(0.0..=1.0).contains(&body.opacity) {
            return Err(invalid("invalid body opacity"));
        }
    }
    let encoded_limit = u64::from(request.width) * u64::from(request.height) * 8 + 65_536;
    Ok(Source {
        admission: Admission {
            triangles,
            face_appearances: faces,
            geometry_bytes: geometry,
            pixel_scratch_bytes: 0,
            preparation_bytes: 0,
            estimated_total_bytes: geometry,
            encoded_limit,
        },
        bodies: scene.meshes.len() as u64,
        raster_scratch_bytes: resolved.estimated_scratch_bytes,
    })
}

/// The CPU raster scratch/preparation estimate and its 384 MiB per-job check.
fn admit_raster(source: Source) -> Result<Admission> {
    let mut a = source.admission;
    // Three bounded 16-byte lists, a conservative map-node allowance, and codec overhead.
    let preparation = a
        .triangles
        .checked_mul(48)
        .and_then(|n| a.face_appearances.checked_mul(160).and_then(|f| n.checked_add(f)))
        .and_then(|n| n.checked_add(source.bodies * 128 + 262_144))
        .ok_or_else(|| limit("work list size overflow"))?;
    let total = a
        .geometry_bytes
        .checked_add(preparation)
        .and_then(|n| n.checked_add(source.raster_scratch_bytes))
        .ok_or_else(|| limit("image memory overflow"))?;
    if total > MAX_JOB_BYTES {
        return Err(limit("product job exceeds its 384 MiB admission estimate"));
    }
    a.pixel_scratch_bytes = source.raster_scratch_bytes;
    a.preparation_bytes = preparation;
    a.estimated_total_bytes = total;
    Ok(a)
}

fn reserve<T>(count: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(count).map_err(|_| JobError::new(ErrorCode::AllocationFailed, "could not reserve render work memory"))?;
    Ok(v)
}

#[derive(Clone, Copy, Default)]
struct Draw {
    body: u32,
    triangle: u32,
    depth: f64,
}

impl Draw {
    fn compare(&self, other: &Self) -> std::cmp::Ordering {
        self.depth.total_cmp(&other.depth).then_with(|| (self.body, self.triangle).cmp(&(other.body, other.triangle)))
    }
}

struct Triangle {
    points: [[f64; 3]; 3],
    world: [Vec3; 3],
    normals: [Vec3; 3],
    inv_w: [f64; 3],
    colour: [f64; 3],
    opacity: f64,
    surface: Option<crate::surface::OpaqueSurface>,
    centre_depth: f64,
    area: f64,
    bounds: [usize; 4],
    x: usize,
    y: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Looks,
    Prepare,
    Sort,
    Clear,
    Opaque,
    Glass,
    Downsample,
    Encode,
    Done,
}

struct BufferWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
    limit: usize,
    cancel: CancelToken,
}

impl Write for BufferWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::other("render cancelled"));
        }
        let mut out = self.bytes.lock().map_err(|_| std::io::Error::other("PNG output lock poisoned"))?;
        if out.len().checked_add(bytes.len()).is_none_or(|n| n > self.limit) {
            return Err(std::io::Error::other("PNG output exceeds its byte budget"));
        }
        out.try_reserve_exact(bytes.len()).map_err(|_| std::io::Error::other("PNG allocation failed"))?;
        out.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.cancel.is_cancelled() { Err(std::io::Error::other("render cancelled")) } else { Ok(()) }
    }
}

pub enum TaskPoll {
    Pending(Progress),
    Complete(Vec<u8>),
}

/// CPU product renderer with cooperative preparation, sorting, raster and PNG encoding.
/// Transparency sorts triangle centres and blends in linear space; it is not refraction.
pub struct ImageTask {
    scene: ProductScene,
    request: ImageRequest,
    admission: Admission,
    cancel: CancelToken,
    stage: Stage,
    vp: Mat4,
    back: Vec3,
    sw: usize,
    sh: usize,
    looks: Vec<BTreeMap<u32, FaceAppearance>>,
    body: usize,
    item: usize,
    prepare_done: u64,
    opaque: Vec<Draw>,
    glass: Vec<Draw>,
    scratch: Vec<Draw>,
    sort_width: usize,
    sort_block: usize,
    sort_left: usize,
    sort_right: usize,
    sort_mid: usize,
    sort_end: usize,
    sort_dst: usize,
    pixels: Vec<[u8; 4]>,
    depth: Vec<f32>,
    pixel_tests: u64,
    pixel_done: u64,
    draw: usize,
    triangle: Option<Triangle>,
    output: Vec<u8>,
    encoded: Arc<Mutex<Vec<u8>>>,
    encoder: Option<png::StreamWriter<'static, BufferWriter>>,
    row: usize,
}

impl ImageTask {
    pub fn new(scene: ProductScene, request: ImageRequest, cancel: CancelToken) -> Result<Self> {
        cancel.check()?;
        let admission = admit(&scene, &request)?;
        let (sw, sh) = (request.width as usize * SS, request.height as usize * SS);
        let radius = scene.bounds.diagonal().max(10.0) + scene.bounds.center().dist(request.camera.target);
        if !radius.is_finite() {
            return Err(invalid("invalid product bounds"));
        }
        let vp = request.camera.view_proj(sw as f64 / sh as f64, radius);
        let back = request.camera.back();
        let looks = reserve(scene.meshes.len())?;
        let (opaque, glass, scratch) =
            (reserve(admission.triangles as usize)?, reserve(admission.triangles as usize)?, reserve(admission.triangles as usize)?);
        let (pixels, depth, output) = (reserve(sw * sh)?, reserve(sw * sh)?, reserve(request.width as usize * request.height as usize * 4)?);
        Ok(Self {
            scene,
            request,
            admission,
            cancel,
            stage: Stage::Looks,
            vp,
            back,
            sw,
            sh,
            looks,
            body: 0,
            item: 0,
            prepare_done: 0,
            opaque,
            glass,
            scratch,
            sort_width: 1,
            sort_block: 0,
            sort_left: 0,
            sort_right: 0,
            sort_mid: 0,
            sort_end: 0,
            sort_dst: 0,
            pixels,
            depth,
            pixel_tests: 0,
            pixel_done: 0,
            draw: 0,
            triangle: None,
            output,
            encoded: Arc::new(Mutex::new(Vec::new())),
            encoder: None,
            row: 0,
        })
    }

    pub fn progress(&self) -> Progress {
        match self.stage {
            Stage::Looks | Stage::Prepare | Stage::Sort => Progress {
                phase: Phase::Preparing,
                done: self.prepare_done,
                total: self.admission.face_appearances + self.admission.triangles + self.scene.meshes.len() as u64 + self.glass.len() as u64 * 19,
            },
            Stage::Clear | Stage::Opaque | Stage::Glass | Stage::Downsample => Progress {
                phase: Phase::Running,
                done: self.pixel_done,
                total: self.pixel_tests + (self.sw * self.sh) as u64 + u64::from(self.request.width) * u64::from(self.request.height),
            },
            Stage::Encode => Progress { phase: Phase::Encoding, done: self.row as u64, total: u64::from(self.request.height) },
            Stage::Done => Progress { phase: Phase::Completed, done: 1, total: 1 },
        }
    }

    /// At most 16384 simple units; encoding additionally yields after at most four rows.
    pub fn step(&mut self, units: usize) -> Result<TaskPoll> {
        let mut rows = 0;
        for _ in 0..units.clamp(1, MAX_STEP) {
            self.cancel.check()?;
            match self.stage {
                Stage::Looks => self.look_step()?,
                Stage::Prepare => self.prepare_step()?,
                Stage::Sort => self.sort_step()?,
                Stage::Clear => self.clear_step(),
                Stage::Opaque | Stage::Glass => self.draw_step()?,
                Stage::Downsample => self.downsample_step()?,
                Stage::Encode => {
                    if let Some(png) = self.encode_step()? {
                        return Ok(TaskPoll::Complete(png));
                    }
                    rows += 1;
                    if rows >= 4 {
                        break;
                    }
                }
                Stage::Done => return Err(JobError::new(ErrorCode::NotCompleted, "render result already consumed")),
            }
        }
        Ok(TaskPoll::Pending(self.progress()))
    }

    fn look_step(&mut self) -> Result<()> {
        let Some(body) = self.scene.meshes.get(self.body) else {
            self.body = 0;
            self.item = 0;
            self.stage = Stage::Prepare;
            return Ok(());
        };
        if self.looks.len() == self.body {
            self.looks.push(BTreeMap::new());
        }
        if let Some(face) = body.faces.get(self.item) {
            if !face.opacity.is_finite() || !(0.0..=1.0).contains(&face.opacity) {
                return Err(invalid("invalid face opacity"));
            }
            let looks = self.looks.get_mut(self.body).ok_or_else(|| invalid("missing body appearances"))?;
            // Match the document policy: the last assignment of a face wins.
            looks.insert(face.face, face.clone());
            self.item += 1;
        } else {
            self.body += 1;
            self.item = 0;
        }
        self.prepare_done += 1;
        Ok(())
    }

    fn project(&self, draw: Draw) -> Result<Option<Triangle>> {
        let body = self.scene.meshes.get(draw.body as usize).ok_or_else(|| invalid("missing product body"))?;
        let mesh = &body.mesh;
        let indices = mesh.triangles.get(draw.triangle as usize).ok_or_else(|| invalid("missing product triangle"))?;
        let vertices = indices.map(|i| mesh.positions.get(i as usize).copied());
        let normals = indices.map(|i| mesh.normals.get(i as usize).copied());
        let ([Some(a), Some(b), Some(c)], [Some(na), Some(nb), Some(nc)]) = (vertices, normals) else {
            return Err(invalid("invalid triangle index or missing normal"));
        };
        let world = [a, b, c];
        let normals = [na, nb, nc];
        if world.iter().chain(normals.iter()).any(|v| !v.is_finite()) {
            return Err(invalid("non-finite product triangle"));
        }
        let look = self.looks.get(draw.body as usize).and_then(|looks| mesh.tri_face.get(draw.triangle as usize).and_then(|face| looks.get(face)));
        let (colour, opacity, surface) = look.map_or((body.color, body.opacity, body.surface), |look| (look.color, look.opacity, look.surface));
        if surface.is_some() && opacity != 1.0 {
            return Err(invalid("opaque PBR requires opacity 1"));
        }
        let normals = if surface.is_some() {
            [crate::surface::unit_direction(na), crate::surface::unit_direction(nb), crate::surface::unit_direction(nc)]
                .map(|n| n.map_err(|e| invalid(&e.to_string())))
        } else {
            [Ok(normals[0]), Ok(normals[1]), Ok(normals[2])]
        };
        let [na, nb, nc] = normals;
        let normals = [na?, nb?, nc?];
        if opacity <= 0.0 {
            return Ok(None);
        }
        let clip = world.map(|p| self.vp.apply(p));
        if clip.iter().flatten().any(|v| !v.is_finite()) {
            return Err(invalid("non-finite product projection"));
        }
        if clip.iter().any(|p| p[3] <= 1e-9) {
            return Ok(None);
        }
        let inv_w = clip.map(|p| 1.0 / p[3]);
        let points = clip.map(|p| [(p[0] / p[3] + 1.0) * 0.5 * self.sw as f64, (1.0 - p[1] / p[3]) * 0.5 * self.sh as f64, p[2] / p[3]]);
        if points.iter().flatten().any(|v| !v.is_finite()) || points.iter().any(|p| !(p[2] as f32).is_finite()) {
            return Err(invalid("product projection exceeds pixel precision"));
        }
        let [a, b, c] = points;
        let (x0, x1) = (a[0].min(b[0]).min(c[0]).floor().max(0.0), a[0].max(b[0]).max(c[0]).ceil().min(self.sw as f64 - 1.0));
        let (y0, y1) = (a[1].min(b[1]).min(c[1]).floor().max(0.0), a[1].max(b[1]).max(c[1]).ceil().min(self.sh as f64 - 1.0));
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if !(x0 <= x1 && y0 <= y1) || area.abs() < 1e-12 {
            return Ok(None);
        }
        if !area.is_finite() {
            return Err(invalid("invalid projected area"));
        }
        let centre_depth = (world[0] / 3.0 + world[1] / 3.0 + world[2] / 3.0 - self.request.camera.target).dot(self.back);
        if !centre_depth.is_finite() {
            return Err(invalid("invalid triangle depth"));
        }
        Ok(Some(Triangle {
            points,
            world,
            normals,
            inv_w,
            colour: colour.map(decode),
            opacity,
            surface,
            centre_depth,
            area,
            bounds: [x0 as usize, x1 as usize, y0 as usize, y1 as usize],
            x: x0 as usize,
            y: y0 as usize,
        }))
    }

    fn prepare_step(&mut self) -> Result<()> {
        let Some(body) = self.scene.meshes.get(self.body) else {
            self.stage = if self.glass.len() > 1 { Stage::Sort } else { Stage::Clear };
            self.sort_block = 0;
            self.init_merge();
            return Ok(());
        };
        if self.item >= body.mesh.triangles.len() {
            self.body += 1;
            self.item = 0;
            return Ok(());
        }
        let mut draw = Draw { body: self.body as u32, triangle: self.item as u32, depth: 0.0 };
        if let Some(triangle) = self.project(draw)? {
            let [x0, x1, y0, y1] = triangle.bounds;
            let tests = ((x1 - x0 + 1) as u64).checked_mul((y1 - y0 + 1) as u64).ok_or_else(|| limit("pixel work overflow"))?;
            self.pixel_tests = self.pixel_tests.checked_add(tests).ok_or_else(|| limit("pixel work overflow"))?;
            if self.pixel_tests > MAX_PIXEL_TESTS {
                return Err(limit("projected product exceeds 128000000 pixel tests"));
            }
            draw.depth = triangle.centre_depth;
            if triangle.opacity >= 1.0 {
                self.opaque.push(draw);
            } else {
                self.glass.push(draw);
                self.scratch.push(draw);
            }
        }
        self.item += 1;
        self.prepare_done += 1;
        Ok(())
    }

    fn init_merge(&mut self) {
        let n = self.glass.len();
        self.sort_mid = (self.sort_block + self.sort_width).min(n);
        self.sort_end = (self.sort_block + 2 * self.sort_width).min(n);
        self.sort_left = self.sort_block;
        self.sort_right = self.sort_mid;
        self.sort_dst = self.sort_block;
    }

    fn sort_step(&mut self) -> Result<()> {
        if self.sort_dst >= self.sort_end {
            self.sort_block += 2 * self.sort_width;
            if self.sort_block >= self.glass.len() {
                std::mem::swap(&mut self.glass, &mut self.scratch);
                self.sort_width *= 2;
                self.sort_block = 0;
                if self.sort_width >= self.glass.len() {
                    self.stage = Stage::Clear;
                    return Ok(());
                }
            }
            self.init_merge();
        }
        let left = self.glass.get(self.sort_left).filter(|_| self.sort_left < self.sort_mid);
        let right = self.glass.get(self.sort_right).filter(|_| self.sort_right < self.sort_end);
        let take_left = match (left, right) {
            (Some(a), Some(b)) => a.compare(b).is_le(),
            (Some(_), None) => true,
            _ => false,
        };
        let source = if take_left {
            let i = self.sort_left;
            self.sort_left += 1;
            i
        } else {
            let i = self.sort_right;
            self.sort_right += 1;
            i
        };
        let draw = self.glass.get(source).copied().ok_or_else(|| invalid("invalid transparent sort source"))?;
        *self.scratch.get_mut(self.sort_dst).ok_or_else(|| invalid("invalid transparent sort destination"))? = draw;
        self.sort_dst += 1;
        self.prepare_done += 1;
        Ok(())
    }

    fn clear_step(&mut self) {
        if self.pixels.len() >= self.sw * self.sh {
            self.stage = Stage::Opaque;
            self.draw = 0;
            return;
        }
        let [r, g, b] = self.request.background;
        self.pixels.push(if self.request.transparent_background { [0; 4] } else { [r, g, b, 255] });
        self.depth.push(f32::INFINITY);
        self.pixel_done += 1;
    }

    fn draw_step(&mut self) -> Result<()> {
        if self.triangle.is_none() {
            let list = if self.stage == Stage::Opaque { &self.opaque } else { &self.glass };
            let Some(draw) = list.get(self.draw).copied() else {
                self.stage = if self.stage == Stage::Opaque { Stage::Glass } else { Stage::Downsample };
                self.draw = 0;
                return Ok(());
            };
            self.triangle = self.project(draw)?;
            if self.triangle.is_none() {
                self.draw += 1;
            }
            return Ok(());
        }
        let triangle = self.triangle.as_mut().ok_or_else(|| invalid("missing raster triangle"))?;
        let eye = (self.request.camera.fov > 0.0).then(|| self.request.camera.eye());
        raster_pixel(triangle, self.sw, self.back, eye, &mut self.pixels, &mut self.depth)?;
        triangle.x += 1;
        if triangle.x > triangle.bounds[1] {
            triangle.x = triangle.bounds[0];
            triangle.y += 1;
        }
        if triangle.y > triangle.bounds[3] {
            self.triangle = None;
            self.draw += 1;
        }
        self.pixel_done += 1;
        Ok(())
    }

    fn downsample_step(&mut self) -> Result<()> {
        let width = self.request.width as usize;
        let index = self.output.len() / 4;
        if index >= width * self.request.height as usize {
            let writer = BufferWriter { bytes: self.encoded.clone(), limit: self.admission.encoded_limit as usize, cancel: self.cancel.clone() };
            let mut encoder = png::Encoder::new(writer, self.request.width, self.request.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
            self.encoder = Some(
                encoder.write_header().and_then(|w| w.into_stream_writer()).map_err(|e| JobError::new(ErrorCode::EncodingFailed, e.to_string()))?,
            );
            self.stage = Stage::Encode;
            // Pixels and prepared geometry are no longer needed during PNG compression.
            self.pixels = Vec::new();
            self.depth = Vec::new();
            self.opaque = Vec::new();
            self.glass = Vec::new();
            self.scratch = Vec::new();
            self.looks = Vec::new();
            self.scene.meshes.clear();
            return Ok(());
        }
        let (x, y) = (index % width, index / width);
        let (mut colour, mut alpha) = ([0.0; 3], 0.0);
        for dy in 0..SS {
            for dx in 0..SS {
                let p = self.pixels.get((y * SS + dy) * self.sw + x * SS + dx).ok_or_else(|| invalid("missing downsample pixel"))?;
                let a = f64::from(p[3]) / 255.0;
                for (sum, channel) in colour.iter_mut().zip(p.iter().take(3)) {
                    *sum += decode(*channel) * a;
                }
                alpha += a;
            }
        }
        let rgb = colour.map(|v| if alpha > 1e-12 { encode(v / alpha) } else { 0 });
        self.output.extend_from_slice(&[rgb[0], rgb[1], rgb[2], (alpha * 255.0 / (SS * SS) as f64).round() as u8]);
        self.pixel_done += 1;
        Ok(())
    }

    fn encode_step(&mut self) -> Result<Option<Vec<u8>>> {
        if self.row < self.request.height as usize {
            let length = self.request.width as usize * 4;
            let start = self.row * length;
            let row = self.output.get(start..start + length).ok_or_else(|| invalid("missing PNG row"))?;
            self.encoder
                .as_mut()
                .ok_or_else(|| invalid("missing PNG writer"))?
                .write_all(row)
                .map_err(|e| JobError::new(ErrorCode::EncodingFailed, e.to_string()))?;
            self.row += 1;
            return Ok(None);
        }
        self.encoder
            .take()
            .ok_or_else(|| invalid("missing PNG writer"))?
            .finish()
            .map_err(|e| JobError::new(ErrorCode::EncodingFailed, e.to_string()))?;
        self.cancel.check()?;
        let mut bytes = self.encoded.lock().map_err(|_| JobError::new(ErrorCode::Internal, "PNG output lock poisoned"))?;
        let png = std::mem::take(&mut *bytes);
        if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(JobError::new(ErrorCode::EncodingFailed, "invalid PNG output"));
        }
        self.output = Vec::new();
        self.stage = Stage::Done;
        Ok(Some(png))
    }
}

fn raster_pixel(t: &Triangle, width: usize, back: Vec3, eye: Option<Vec3>, pixels: &mut [[u8; 4]], depth: &mut [f32]) -> Result<()> {
    let [a, b, c] = t.points;
    let (x, y) = (t.x as f64 + 0.5, t.y as f64 + 0.5);
    let w0 = ((b[0] - x) * (c[1] - y) - (b[1] - y) * (c[0] - x)) / t.area;
    let w1 = ((c[0] - x) * (a[1] - y) - (c[1] - y) * (a[0] - x)) / t.area;
    let w2 = 1.0 - w0 - w1;
    // A shared triangle edge belongs to exactly one raster triangle, including glass.
    for (w, mut p, mut q) in [(w0, b, c), (w1, c, a), (w2, a, b)] {
        if w < -1e-9 {
            return Ok(());
        }
        if w.abs() <= 1e-9 {
            if t.area < 0.0 {
                std::mem::swap(&mut p, &mut q);
            }
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            if !(dy < 0.0 || (dy == 0.0 && dx > 0.0)) {
                return Ok(());
            }
        }
    }
    let z = (w0 * a[2] + w1 * b[2] + w2 * c[2]) as f32;
    let index = t.y * width + t.x;
    let old_depth = depth.get_mut(index).ok_or_else(|| invalid("missing pixel depth"))?;
    if z > *old_depth {
        return Ok(());
    }
    let weights = [w0 * t.inv_w[0], w1 * t.inv_w[1], w2 * t.inv_w[2]];
    let sum: f64 = weights.iter().sum();
    if !sum.is_finite() || sum <= 1e-12 {
        return Ok(());
    }
    let n = t.normals.iter().zip(weights).fold(Vec3::ZERO, |v, (n, w)| v + *n * (w / sum)).normalized().unwrap_or(Vec3::Z);
    let colour = if let Some(surface) = t.surface {
        let world = t.world.iter().zip(weights).fold(Vec3::ZERO, |p, (v, w)| p + *v * (w / sum));
        let view = eye.map_or(back, |eye| eye - world);
        let radiance = crate::studio::shade(surface, n, view).map_err(|e| invalid(&e.to_string()))?;
        let mapped = crate::studio::tone_map(radiance).map_err(|e| invalid(&e.to_string()))?;
        [mapped.x, mapped.y, mapped.z]
    } else {
        crate::raster::shade(n, back, t.colour)
    };
    if colour.iter().any(|v| !v.is_finite()) {
        return Err(invalid("non-finite shaded colour"));
    }
    let pixel = pixels.get_mut(index).ok_or_else(|| invalid("missing colour pixel"))?;
    if t.opacity >= 1.0 {
        *old_depth = z;
        *pixel = [encode(colour[0]), encode(colour[1]), encode(colour[2]), 255];
    } else {
        let old_alpha = f64::from(pixel[3]) / 255.0;
        let alpha = t.opacity + old_alpha * (1.0 - t.opacity);
        for (out, src) in pixel.iter_mut().take(3).zip(colour) {
            *out = encode((src * t.opacity + decode(*out) * old_alpha * (1.0 - t.opacity)) / alpha);
        }
        pixel[3] = (alpha * 255.0).round() as u8;
    }
    Ok(())
}

fn decode(v: u8) -> f64 {
    let s = f64::from(v) / 255.0;
    if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
}
fn encode(v: f64) -> u8 {
    let l = v.clamp(0.0, 1.0);
    let s = if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}
