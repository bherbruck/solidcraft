//! Original one-sided rectangular direct emitters in world millimetres.
//! Sampling estimates the area integral; it is not an exact solid-angle integral.
//! No emitter geometry, indirect bounce or calibrated photometric units are implied.

use solidcraft_geom::Vec3;

use super::lighting::MAX_LINEAR_RADIANCE;
use super::{MAX_COORDINATE_MM, MAX_DISTANCE_MM, Ray, Result, TraceError};
use crate::surface::{OpaqueSurface, unit_direction};

pub const MAX_AREA_LIGHTS: usize = 4;
pub const MAX_AREA_SAMPLES: u32 = 64;
pub const MAX_EDGE_MM: f64 = 1.0e6;
pub const MIN_EDGE_MM: f64 = 1.0e-6;

fn invalid(reason: &'static str) -> TraceError {
    TraceError::InvalidInput(reason)
}

fn bounded_point(p: Vec3) -> bool {
    p.is_finite() && [p.x, p.y, p.z].into_iter().all(|c| c.abs() <= MAX_COORDINATE_MM)
}

pub fn valid_sample_count(samples: u32) -> bool {
    matches!(samples, 1 | 4 | 16 | 64)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectangleLight {
    center: Vec3,
    edge_u: Vec3,
    edge_v: Vec3,
    normal: Vec3,
    area: f64,
    emitted: Vec3,
}

impl RectangleLight {
    /// Edges are full side vectors, not half extents. `U cross V` is the emitting side.
    /// Linear colour times intensity is relative emitted radiance, never a surface temperature.
    pub fn new(center: Vec3, edge_u: Vec3, edge_v: Vec3, color: Vec3, intensity: f64) -> Result<Self> {
        if !bounded_point(center) || !edge_u.is_finite() || !edge_v.is_finite() {
            return Err(invalid("area light requires finite bounded center and edges"));
        }
        let u = edge_u.len();
        let v = edge_v.len();
        if !u.is_finite() || !v.is_finite() || !(MIN_EDGE_MM..=MAX_EDGE_MM).contains(&u) || !(MIN_EDGE_MM..=MAX_EDGE_MM).contains(&v) {
            return Err(invalid("area light side lengths must be within 1e-6..1e6 mm"));
        }
        if (edge_u.dot(edge_v) / (u * v)).abs() > 1e-10 {
            return Err(invalid("rectangular light edges must be perpendicular"));
        }
        if !color.is_finite()
            || ![color.x, color.y, color.z].into_iter().all(|c| (0.0..=1.0).contains(&c))
            || !intensity.is_finite()
            || !(0.0..=64.0).contains(&intensity)
        {
            return Err(invalid("area light requires linear color in [0,1] and intensity in [0,64]"));
        }
        let cross = edge_u.cross(edge_v);
        let area = cross.len();
        let normal = unit_direction(cross).map_err(|_| invalid("area light has degenerate emitting normal"))?;
        for a in [-0.5, 0.5] {
            for b in [-0.5, 0.5] {
                if !bounded_point(center + edge_u * a + edge_v * b) {
                    return Err(invalid("area light corners exceed world coordinate bounds"));
                }
            }
        }
        Ok(Self { center, edge_u, edge_v, normal, area, emitted: color * intensity })
    }

    pub fn area_mm2(self) -> f64 {
        self.area
    }

    pub fn normal(self) -> Vec3 {
        self.normal
    }

    /// One deterministic, uniformly stratified area sample. `samples=1` samples the centre.
    /// The consumer owns cancellation and charges each sample to its cumulative work budget.
    pub fn sample(self, point: Vec3, pixel: u64, light: u32, index: u32, samples: u32, seed: u32) -> Result<Option<AreaSample>> {
        if !bounded_point(point) || !valid_sample_count(samples) || index >= samples || light >= MAX_AREA_LIGHTS as u32 {
            return Err(invalid("invalid bounded area sample request"));
        }
        if self.emitted == Vec3::ZERO {
            return Ok(None);
        }
        let (u, v) = sample_uv(pixel, light, index, samples, seed);
        let position = self.center + self.edge_u * (u - 0.5) + self.edge_v * (v - 0.5);
        let delta = position - point;
        let distance = delta.len();
        if !distance.is_finite() || distance <= 0.0 || distance > MAX_DISTANCE_MM {
            return Err(invalid("area sample distance must be finite, positive and within ray bounds"));
        }
        let direction = delta / distance;
        let cosine = self.normal.dot(-direction).max(0.0);
        if cosine == 0.0 {
            return Ok(None);
        }
        // mm² / mm² cancels; 1/pdf(area)=area and the finite-sample average=1/N.
        let weight = self.area * cosine / (distance * distance) / f64::from(samples);
        let incident = self.emitted * weight;
        if !incident.is_finite() || ![incident.x, incident.y, incident.z].into_iter().all(|v| (0.0..=MAX_LINEAR_RADIANCE).contains(&v)) {
            return Err(invalid("area sample exceeds finite linear radiance bounds"));
        }
        Ok(Some(AreaSample { position, direction, distance, incident }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaSample {
    position: Vec3,
    direction: Vec3,
    distance: f64,
    incident: Vec3,
}

impl AreaSample {
    pub fn position(self) -> Vec3 {
        self.position
    }
    pub fn direction(self) -> Vec3 {
        self.direction
    }
    pub fn distance_mm(self) -> f64 {
        self.distance
    }
    pub fn incident(self) -> Vec3 {
        self.incident
    }

    /// Re-aim from the robustly offset hit origin to the finite emitter sample.
    /// Geometry beyond that point must not cast a shadow from this light sample.
    pub fn shadow_ray(self, offset_origin: Vec3) -> Result<Ray> {
        let delta = self.position - offset_origin;
        let distance = delta.len();
        let forward = delta.dot(self.direction);
        if !forward.is_finite() || forward <= 0.0 || !distance.is_finite() || distance <= 0.0 || distance > MAX_DISTANCE_MM {
            return Err(invalid("invalid finite area shadow interval"));
        }
        Ray::new(offset_origin, delta, f64::from_bits(1), distance.next_down())
    }

    /// This sample's visible direct contribution, before exposure or display transfer.
    pub fn shade(self, surface: OpaqueSurface, normal: Vec3, view: Vec3) -> Result<Vec3> {
        let normal = unit_direction(normal).map_err(|_| invalid("invalid area shading normal"))?;
        let brdf = surface.brdf(normal, view, self.direction).map_err(|_| invalid("invalid area shading directions"))?;
        let cosine = normal.dot(self.direction).max(0.0);
        let value = Vec3::new(brdf.x * self.incident.x, brdf.y * self.incident.y, brdf.z * self.incident.z) * cosine;
        if !value.is_finite() || ![value.x, value.y, value.z].into_iter().all(|c| (0.0..=MAX_LINEAR_RADIANCE).contains(&c)) {
            return Err(invalid("area contribution exceeds finite linear radiance bounds"));
        }
        Ok(value)
    }
}

fn sample_uv(pixel: u64, light: u32, index: u32, samples: u32, seed: u32) -> (f64, f64) {
    if samples == 1 {
        return (0.5, 0.5);
    }
    let side = match samples {
        4 => 2,
        16 => 4,
        _ => 8,
    };
    // Original repeatable integer recurrence. It chooses a bounded subcell offset;
    // it is a sampling pattern, not a cryptographic RNG or an unbiasedness certificate.
    let mut state = pixel.wrapping_add(u64::from(seed) * 65537).wrapping_add(u64::from(light) * 104729).wrapping_add(u64::from(index) * 8191);
    let mut fraction = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((state >> 32) as u32 as f64 + 0.5) / 4294967296.0
    };
    ((f64::from(index % side) + fraction()) / f64::from(side), (f64::from(index / side) + fraction()) / f64::from(side))
}

#[cfg(test)]
#[path = "area_tests.rs"]
mod tests;
