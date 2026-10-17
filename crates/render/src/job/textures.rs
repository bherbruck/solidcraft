//! Immutable request-only planar opaque textures. Codecs run on the image worker.
use super::{CancelToken, ErrorCode, JobError, Result};
use crate::{
    ProductScene,
    surface::{srgb8_to_linear, unit_direction},
    texture_input::{MapRole, TextureSource},
    texture_sampling::TextureView,
};
use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

pub const MAX_TEXTURE_BINDINGS: usize = 16;
pub const MAX_TEXTURE_PIXELS_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_TEXTURE_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_PLANAR_REPEAT: f64 = 128.0;

fn invalid(message: impl Into<String>) -> JobError {
    JobError::new(ErrorCode::InvalidRequest, message)
}
fn v(a: [f64; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

/// `u` and `v` span one full repeat in world-space millimetres. Image v increases
/// downward; normal-map green points along the caller's positive v direction.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanarFrame {
    pub origin: [f64; 3],
    pub u: [f64; 3],
    pub v: [f64; 3],
}
impl PlanarFrame {
    pub fn validate(self) -> Result<()> {
        let u = v(self.u);
        let vv = v(self.v);
        let ul = u.len();
        let vl = vv.len();
        if self.origin.into_iter().any(|c| !c.is_finite() || c.abs() > 1e12)
            || !u.is_finite()
            || !vv.is_finite()
            || !(0.001..=1e6).contains(&ul)
            || !(0.001..=1e6).contains(&vl)
            || u.dot(vv).abs() > 1e-6 * ul * vl
        {
            return Err(invalid("planar texture frame requires finite origin and orthogonal u/v repeat vectors of 0.001..1000000 mm"));
        }
        Ok(())
    }
    pub fn uv(self, point: Vec3) -> Result<[f64; 2]> {
        self.validate()?;
        let d = point - v(self.origin);
        let u = v(self.u);
        let vv = v(self.v);
        let uv = [d.dot(u) / u.len2(), d.dot(vv) / vv.len2()];
        if uv.iter().any(|c| !c.is_finite() || c.abs() > MAX_PLANAR_REPEAT) {
            return Err(invalid("planar UV exceeds the GPU precision bound of 128 repeats"));
        }
        Ok(uv)
    }
    pub fn mapped_normal(self, normal: Vec3, geometric: Vec3, rgb: [f64; 3], strength: f64) -> Result<Vec3> {
        self.validate()?;
        if !strength.is_finite() || !(0.0..=1.0).contains(&strength) || rgb.iter().any(|c| !c.is_finite() || !(0.0..=1.0).contains(c)) {
            return Err(invalid("invalid tangent normal input"));
        }
        let n = unit_direction(normal).map_err(|e| invalid(e.to_string()))?;
        let g = unit_direction(geometric).map_err(|e| invalid(e.to_string()))?;
        let u = unit_direction(v(self.u)).map_err(|e| invalid(e.to_string()))?;
        let tangent = u - n * n.dot(u);
        if tangent.len2() < 1e-12 {
            return Ok(n);
        }
        let tangent = unit_direction(tangent).map_err(|e| invalid(e.to_string()))?;
        let handedness = if n.dot(v(self.u).cross(v(self.v))) < 0.0 { -1.0 } else { 1.0 };
        let local = Vec3::new((2.0 * rgb[0] - 1.0) * strength, (2.0 * rgb[1] - 1.0) * strength, 2.0 * rgb[2] - 1.0);
        if local.len2() < 1e-12 || local.z <= 0.0 {
            return Ok(n);
        }
        let mapped =
            unit_direction(tangent * local.x + n.cross(tangent) * (handedness * local.y) + n * local.z).map_err(|e| invalid(e.to_string()))?;
        Ok(if mapped.dot(g) <= 0.0 { n } else { mapped })
    }
}

#[derive(Clone)]
pub struct TextureMap {
    source: TextureSource,
    retained_bytes: usize,
}
impl std::fmt::Debug for TextureMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextureMap").field("info", self.source.info()).finish()
    }
}
impl TextureMap {
    pub fn new(bytes: Vec<u8>, role: MapRole) -> Result<Self> {
        if !matches!(role, MapRole::BaseColor | MapRole::NormalOpenGl) {
            return Err(invalid("only sRGB base color and OpenGL tangent normal maps are supported"));
        }
        let retained_bytes = bytes.capacity();
        let source = TextureSource::new(bytes, role).map_err(|e| invalid(e.to_string()))?;
        if source.info().width > 1024 || source.info().height > 1024 {
            return Err(invalid("runtime texture maps are limited to 1024x1024"));
        }
        Ok(Self { source, retained_bytes })
    }
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
    pub fn dimensions(&self) -> [u32; 2] {
        [self.source.info().width, self.source.info().height]
    }
    pub fn role(&self) -> MapRole {
        self.source.info().role
    }
    pub fn decode(&self, cancel: &CancelToken) -> Result<Vec<f32>> {
        cancel.check()?;
        let decoded = self.source.decode(|| cancel.is_cancelled()).map_err(|e| {
            if cancel.is_cancelled() { JobError::new(ErrorCode::Cancelled, "texture decoding cancelled") } else { invalid(e.to_string()) }
        })?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(decoded.rgba().len()).map_err(|_| JobError::new(ErrorCode::AllocationFailed, "texture pixel allocation failed"))?;
        for (index, pixel) in decoded.rgba().as_chunks::<4>().0.iter().enumerate() {
            if index % 1024 == 0 {
                cancel.check()?;
            }
            for (channel, value) in pixel.iter().enumerate() {
                let linear = if self.role() == MapRole::BaseColor && channel < 3 { srgb8_to_linear(*value) } else { f64::from(*value) / 255.0 };
                rgba.push(linear as f32);
            }
        }
        Ok(rgba)
    }
}

#[derive(Clone, Debug)]
pub struct TextureBinding {
    pub body_index: usize,
    pub body_name: String,
    pub frame: PlanarFrame,
    pub normal_strength: f64,
    pub base_color: Option<TextureMap>,
    pub normal: Option<TextureMap>,
}
#[derive(Clone, Debug)]
pub struct RuntimeTextures {
    bindings: Vec<TextureBinding>,
    source_bytes: u64,
    pixel_bytes: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct TextureSummary {
    pub bodies: usize,
    pub encoded_bytes: u64,
    pub linear_pixel_bytes: u64,
}
impl RuntimeTextures {
    pub fn new(bindings: Vec<TextureBinding>) -> Result<Self> {
        if bindings.is_empty() || bindings.len() > MAX_TEXTURE_BINDINGS {
            return Err(invalid("runtime textures require 1..16 distinct body bindings"));
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut source_bytes = 0;
        let mut pixel_bytes = 0;
        for binding in &bindings {
            binding.frame.validate()?;
            if binding.body_index >= 16384
                || binding.body_name.is_empty()
                || binding.body_name.len() > 1024
                || binding.body_name.chars().any(char::is_control)
                || !seen.insert(binding.body_index)
                || !binding.normal_strength.is_finite()
                || !(0.0..=1.0).contains(&binding.normal_strength)
                || (binding.base_color.is_none() && binding.normal.is_none())
            {
                return Err(invalid("invalid or duplicate runtime texture body binding"));
            }
            if binding.base_color.as_ref().is_some_and(|m| m.role() != MapRole::BaseColor)
                || binding.normal.as_ref().is_some_and(|m| m.role() != MapRole::NormalOpenGl)
            {
                return Err(invalid("texture role does not match its binding"));
            }
            for map in [&binding.base_color, &binding.normal].into_iter().flatten() {
                source_bytes += map.retained_bytes as u64;
                pixel_bytes += map.source.info().rgba_bytes * 4;
            }
        }
        if source_bytes > MAX_TEXTURE_SOURCE_BYTES || pixel_bytes > MAX_TEXTURE_PIXELS_BYTES {
            return Err(invalid("runtime textures exceed 16 MiB encoded or 32 MiB linear pixel budget"));
        }
        Ok(Self { bindings, source_bytes, pixel_bytes })
    }
    pub fn bindings(&self) -> &[TextureBinding] {
        &self.bindings
    }
    pub fn summary(&self) -> TextureSummary {
        TextureSummary { bodies: self.bindings.len(), encoded_bytes: self.source_bytes, linear_pixel_bytes: self.pixel_bytes }
    }
    /// Source snapshots, codec scratch, decoded bytes, staging and device buffer,
    /// plus lookup records. Conservatively includes overlapping preparation.
    pub fn working_bytes(&self) -> u64 {
        self.source_bytes + self.pixel_bytes * 4 + 64 * 1024 * 1024 + 1024 * 1024
    }
    pub fn validate_scene(&self, scene: &ProductScene) -> Result<()> {
        for binding in &self.bindings {
            let body = scene
                .meshes
                .get(binding.body_index)
                .filter(|body| body.name == binding.body_name)
                .ok_or_else(|| invalid("texture ordinal and exact body name must match render.scene"))?;
            if body.opacity != 1.0 || body.transmission.is_some() || body.faces.iter().any(|f| f.opacity != 1.0 || f.transmission.is_some()) {
                return Err(invalid("texture maps currently support opaque bodies only; glass and alpha maps are unsupported"));
            }
            let bounds = body.mesh.bounds();
            for x in [bounds.min.x, bounds.max.x] {
                for y in [bounds.min.y, bounds.max.y] {
                    for z in [bounds.min.z, bounds.max.z] {
                        binding.frame.uv(Vec3::new(x, y, z))?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Independent f64 bilinear reference over the worker's normalized pixels.
pub fn sample_map(rgba: &[f32], dimensions: [u32; 2], uv: [f64; 2]) -> Result<[f64; 3]> {
    TextureView::new(rgba, dimensions).and_then(|v| v.sample_repeat_opaque(uv)).map_err(|e| invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    fn png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes).write_image(pixels, width, height, image::ExtendedColorType::Rgba8).unwrap();
        bytes
    }
    fn frame() -> PlanarFrame {
        PlanarFrame { origin: [0.; 3], u: [2., 0., 0.], v: [0., 4., 0.] }
    }
    fn binding() -> TextureBinding {
        TextureBinding {
            body_index: 0,
            body_name: "opaque".into(),
            frame: frame(),
            normal_strength: 1.,
            base_color: Some(TextureMap::new(png(1, 1, &[128, 64, 255, 0]), MapRole::BaseColor).unwrap()),
            normal: None,
        }
    }
    #[test]
    fn explicit_color_semantics_and_repeat_filter_ignore_alpha() {
        let color = TextureMap::new(png(2, 1, &[128, 64, 255, 0, 255, 0, 128, 255]), MapRole::BaseColor).unwrap();
        let data = TextureMap::new(png(1, 1, &[128, 64, 255, 0]), MapRole::NormalOpenGl).unwrap();
        let c = color.decode(&CancelToken::default()).unwrap();
        let n = data.decode(&CancelToken::default()).unwrap();
        assert!((f64::from(c[0]) - srgb8_to_linear(128)).abs() < 1e-7);
        assert!((n[0] - 128. / 255.).abs() < 1e-7);
        let a = sample_map(&c, [2, 1], [0.25, 0.5]).unwrap();
        let b = sample_map(&c, [2, 1], [-0.75, 100.5]).unwrap();
        assert_eq!(a, b);
        assert!((a[0] - srgb8_to_linear(128)).abs() < 1e-7);
        let seam = sample_map(&c, [2, 1], [0., 0.]).unwrap();
        assert!((seam[0] - (1. + a[0]) / 2.).abs() < 1e-7);
        let cancel = CancelToken::default();
        cancel.cancel();
        assert_eq!(color.decode(&cancel).unwrap_err().code, ErrorCode::Cancelled);
    }
    #[test]
    fn tangent_frame_preserves_handedness_and_falls_back_safely() {
        let f = frame();
        assert_eq!(f.uv(Vec3::new(-1., 2., 900.)).unwrap(), [-0.5, 0.5]);
        let normal = f.mapped_normal(Vec3::Z, Vec3::Z, [0.5, 1., 1.], 1.).unwrap();
        assert!((normal - Vec3::new(0., 1., 1.).normalized().unwrap()).len() < 1e-12);
        let mirrored = PlanarFrame { v: [0., -4., 0.], ..f }.mapped_normal(Vec3::Z, Vec3::Z, [0.5, 1., 1.], 1.).unwrap();
        assert!((mirrored - Vec3::new(0., -1., 1.).normalized().unwrap()).len() < 1e-12);
        for rgb in [[0.5; 3], [0., 0., 0.], [1., 1., 0.5]] {
            assert_eq!(f.mapped_normal(Vec3::Z, Vec3::Z, rgb, 1.).unwrap(), Vec3::Z);
        }
        assert_eq!(f.mapped_normal(Vec3::X, Vec3::X, [1.; 3], 1.).unwrap(), Vec3::X);
        assert!(PlanarFrame { u: [0.; 3], ..f }.validate().is_err());
        assert!(PlanarFrame { v: [1., 4., 0.], ..f }.validate().is_err());
        assert!(f.uv(Vec3::new(258., 0., 0.)).is_err());
    }
    #[test]
    fn bounded_resources_reject_duplicate_roles_and_encoded_capacity() {
        let b = binding();
        let t = RuntimeTextures::new(vec![b.clone()]).unwrap();
        assert_eq!(t.summary().linear_pixel_bytes, 16);
        assert!(RuntimeTextures::new(vec![b.clone(), b]).is_err());
        let mut wrong = binding();
        wrong.normal = wrong.base_color.take();
        assert!(RuntimeTextures::new(vec![wrong]).is_err());
        let mut bytes = png(1, 1, &[0; 4]);
        bytes.reserve_exact(16 * 1024 * 1024);
        assert!(TextureMap::new(bytes, MapRole::BaseColor).is_err());
        assert!(TextureMap::new(png(1, 1, &[0; 4]), MapRole::Roughness).is_err());
    }
}
