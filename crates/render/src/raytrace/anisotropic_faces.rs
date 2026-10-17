//! Bounded explicit planar-face adapter for anisotropic direct lighting only.
//! Captured ordinal/name and mesh face tag must match. Strict plane equality is
//! conservative: rotated or imported roundoff may refuse instead of guessing.

use solidcraft_geom::Vec3;

use crate::{
    ProductScene,
    anisotropic_surface::{AnisotropicSurface, VisualFrame},
    job::{self, AnisotropicOrientation, ErrorCode, JobError, RaytraceOptions},
    surface::OpaqueSurface,
};

use super::{TraceError, Triangle, lighting::Lighting};

const MAX_TARGET_BODY_TRIANGLES: usize = 4096;

#[derive(Clone, Copy)]
pub(super) struct PlanarTarget {
    frame: VisualFrame,
    roughness: [f64; 2],
}

fn invalid(message: &'static str) -> JobError {
    JobError::new(ErrorCode::InvalidScene, message)
}

/// Preflight is bounded to two 4096-triangle selected bodies. Task construction
/// repeats validation under its own cumulative work/cancellation callback.
pub(super) fn prepare(
    scene: &ProductScene,
    options: &RaytraceOptions,
    mut charge: impl FnMut() -> job::Result<()>,
) -> job::Result<[Option<PlanarTarget>; 2]> {
    let mut targets = [None; 2];
    for (target, input) in targets.iter_mut().zip(&options.anisotropic_faces) {
        let Some(input) = input else { continue };
        let body = scene.meshes.get(input.body_index).ok_or_else(|| invalid("anisotropic captured body ordinal"))?;
        if body.name != input.body_name {
            return Err(invalid("anisotropic captured body identity changed"));
        }
        if body.mesh.triangles.len() > MAX_TARGET_BODY_TRIANGLES || body.mesh.tri_face.len() != body.mesh.triangles.len() {
            return Err(invalid("anisotropic targets require complete face tags and at most 4096 triangles per body"));
        }
        let mut plane: Option<(Vec3, Vec3)> = None;
        for (indices, face) in body.mesh.triangles.iter().zip(&body.mesh.tri_face) {
            charge()?;
            if *face != input.face {
                continue;
            }
            let mut vertices = [Vec3::ZERO; 3];
            for (v, index) in vertices.iter_mut().zip(indices) {
                *v = *body.mesh.positions.get(*index as usize).ok_or_else(|| invalid("anisotropic face vertex index"))?;
            }
            let triangle = Triangle::new(vertices).map_err(|_| invalid("anisotropic face triangle"))?;
            let normal = triangle.geometric_normal().ok_or_else(|| invalid("anisotropic face must be nondegenerate"))?;
            let [anchor, _, _] = vertices;
            if let Some((origin, expected)) = plane {
                if normal != expected || vertices.into_iter().any(|p| (p - origin).dot(expected) != 0.0) {
                    return Err(invalid("anisotropic face is curved, inconsistently wound or not exactly planar"));
                }
            } else {
                plane = Some((anchor, normal));
            }
        }
        let (_, normal) = plane.ok_or_else(|| invalid("anisotropic captured face has no triangles"))?;
        let [x, y, z] = input.tangent;
        let frame = VisualFrame::new(normal, Vec3::new(x, y, z))
            .map_err(|_| invalid("anisotropic visual tangent is not perpendicular to the planar face"))?;
        let tangent = match input.orientation {
            AnisotropicOrientation::Zero => frame.tangent(),
            AnisotropicOrientation::QuarterTurn => frame.bitangent(),
        };
        *target = Some(PlanarTarget {
            frame: VisualFrame::new(normal, tangent).map_err(|_| invalid("anisotropic planar frame"))?,
            roughness: [input.roughness_x, input.roughness_y],
        });
    }
    Ok(targets)
}

pub(super) fn target_index(options: &RaytraceOptions, body_index: usize, face: Option<u32>) -> Option<usize> {
    options.anisotropic_faces.iter().position(|input| input.as_ref().is_some_and(|input| input.body_index == body_index && Some(input.face) == face))
}

impl PlanarTarget {
    pub(super) fn direct_brdf(self, surface: OpaqueSurface, geometric: Vec3, view: Vec3, light: Vec3) -> super::Result<Vec3> {
        let frame = VisualFrame::new(geometric, self.frame.tangent()).map_err(|_| TraceError::InvalidInput("anisotropic punctual frame"))?;
        let [rx, ry] = self.roughness;
        AnisotropicSurface::new(surface.base_color(), surface.metallic(), rx, ry, surface.dielectric_ior())
            .and_then(|surface| surface.brdf(frame, geometric, view, light))
            .map_err(|_| TraceError::InvalidInput("anisotropic punctual response"))
    }

    /// Real #1658 path continuation: NDF null events return None unchanged.
    /// Equal axes use its existing isotropic sampler and deterministic azimuth,
    /// with the target's explicit scalar roughness. No conditional PDF is invented.
    pub(super) fn sample(
        self,
        surface: OpaqueSurface,
        geometric: Vec3,
        view: Vec3,
        u: [f64; 2],
    ) -> super::Result<Option<super::reflection::ReflectionSample>> {
        let [rx, ry] = self.roughness;
        let frame = VisualFrame::new(geometric, self.frame.tangent()).map_err(|_| TraceError::InvalidInput("anisotropic reflection frame"))?;
        if rx == ry {
            let surface = OpaqueSurface::new(surface.base_color(), surface.metallic(), rx, surface.dielectric_ior())
                .map_err(|_| TraceError::InvalidInput("isotropic reflection surface"))?;
            return super::reflection::rough_specular(surface, geometric, view, u);
        }
        let surface = AnisotropicSurface::new(surface.base_color(), surface.metallic(), rx, ry, surface.dielectric_ior())
            .map_err(|_| TraceError::InvalidInput("anisotropic reflected surface"))?;
        surface
            .sample_reflection(frame, geometric, view, u)
            .map(|sample| sample.map(|s| super::reflection::ReflectionSample { direction: s.direction, weight: s.weight, pdf: s.pdf }))
            .map_err(|_| TraceError::InvalidInput("anisotropic reflection sample"))
    }

    pub(super) fn shade(
        self,
        lights: &Lighting,
        surface: OpaqueSurface,
        geometric: Vec3,
        view: Vec3,
        mut visible: impl FnMut(usize, Vec3) -> super::Result<bool>,
    ) -> super::Result<Vec3> {
        let frame = VisualFrame::new(geometric, self.frame.tangent()).map_err(|_| TraceError::InvalidInput("anisotropic resolved planar frame"))?;
        let [rx, ry] = self.roughness;
        let surface = AnisotropicSurface::new(surface.base_color(), surface.metallic(), rx, ry, surface.dielectric_ior())
            .map_err(|_| TraceError::InvalidInput("anisotropic captured surface"))?;
        let mut sum = Vec3::ZERO;
        for (index, light) in lights.lights().iter().enumerate() {
            let cosine = geometric.dot(light.direction()).max(0.0);
            if cosine == 0.0 || !visible(index, light.direction())? {
                continue;
            }
            let f = surface.brdf(frame, geometric, view, light.direction()).map_err(|_| TraceError::InvalidInput("anisotropic direct response"))?;
            let li = light.color() * (light.intensity() * cosine);
            sum += Vec3::new(f.x * li.x, f.y * li.y, f.z * li.z);
            if !sum.is_finite() || [sum.x, sum.y, sum.z].into_iter().any(|v| !(0.0..=1e12).contains(&v)) {
                return Err(TraceError::InvalidInput("anisotropic direct radiance bound"));
            }
        }
        Ok(sum)
    }
}

#[cfg(test)]
#[path = "anisotropic_faces_tests.rs"]
mod tests;
