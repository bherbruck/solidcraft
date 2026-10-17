//! Borrowed prepared-scene contract shared by bounded transport consumers.
use super::{Bvh, Hit, MAX_DISTANCE_MM, Ray, Result, TraceError};
use crate::{ProductScene, surface::OpaqueSurface};
use solidcraft_geom::Vec3;

#[derive(Clone, Copy)]
pub struct PreparedPrimitive {
    pub body_index: usize,
    /// Authoritative captured mesh face tag; never inferred from a plane.
    pub face: Option<u32>,
    pub vertices: [Vec3; 3],
    pub normals: [Vec3; 3],
    pub surface: OpaqueSurface,
}
pub struct PreparedScene<'a> {
    pub bvh: &'a Bvh,
    pub primitives: &'a [PreparedPrimitive],
    pub source: &'a ProductScene,
}
#[derive(Clone, Copy)]
pub struct SharedHit<'a> {
    pub body_index: usize,
    pub body_name: &'a str,
    pub primitive_index: usize,
    pub distance_mm: f64,
    pub position: Vec3,
    /// Original captured winding; never orient this destructively for shading.
    pub geometric_normal: Vec3,
    pub front_face: bool,
    pub shading_normal: Vec3,
    pub surface: OpaqueSurface,
    pub vertices: [Vec3; 3],
    pub barycentric: [f64; 3],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryStatus {
    Unknown,
}
impl<'a> PreparedScene<'a> {
    pub fn boundary_status(&self, _body_index: usize) -> BoundaryStatus {
        BoundaryStatus::Unknown
    }
    pub fn resolve_hit(&self, ray: Ray, hit: Hit) -> Result<SharedHit<'a>> {
        let primitive = self.primitives.get(hit.primitive_index).ok_or(TraceError::Internal("prepared primitive"))?;
        let body = self.source.meshes.get(primitive.body_index).ok_or(TraceError::Internal("prepared body"))?;
        let interval = ray.interval();
        if !hit.geometry.distance.is_finite()
            || hit.geometry.distance < interval[0]
            || hit.geometry.distance > interval[1]
            || !hit.geometry.barycentric.into_iter().all(|w| w.is_finite() && (0.0..=1.0).contains(&w))
            || (hit.geometry.barycentric.into_iter().sum::<f64>() - 1.0).abs() > 1e-12
        {
            return Err(TraceError::InvalidInput("prepared hit interval or barycentrics"));
        }
        let geometric = super::Triangle::new(primitive.vertices)?.geometric_normal().ok_or(TraceError::InvalidInput("degenerate prepared hit"))?;
        let front_face = ray.direction().dot(geometric) < 0.0;
        let [a, b, c] = hit.geometry.barycentric;
        let [va, vb, vc] = primitive.vertices;
        let [na, nb, nc] = primitive.normals;
        let mut normal = super::unit(na * a + nb * b + nc * c).unwrap_or(geometric);
        if normal.dot(geometric) < 0.0 {
            normal = -normal;
        }
        if !front_face {
            normal = -normal;
        }
        Ok(SharedHit {
            body_index: primitive.body_index,
            body_name: &body.name,
            primitive_index: hit.primitive_index,
            distance_mm: hit.geometry.distance,
            position: va * a + vb * b + vc * c,
            geometric_normal: geometric,
            front_face,
            shading_normal: normal,
            surface: primitive.surface,
            vertices: primitive.vertices,
            barycentric: hit.geometry.barycentric,
        })
    }
}
impl SharedHit<'_> {
    pub fn oriented_geometric_normal(self) -> Vec3 {
        if self.front_face { self.geometric_normal } else { -self.geometric_normal }
    }
    /// Component-wise position error projected onto the original geometric normal.
    pub fn spawn(self, direction: Vec3) -> Result<Ray> {
        let mut n = self.geometric_normal;
        if n.dot(direction) < 0.0 {
            n = -n;
        }
        let [a, b, c] = self.barycentric;
        let [va, vb, vc] = self.vertices;
        let magnitude = |x: f64, y: f64, z: f64| x.abs() * a.abs() + y.abs() * b.abs() + z.abs() * c.abs();
        let e = Vec3::new(magnitude(va.x, vb.x, vc.x), magnitude(va.y, vb.y, vc.y), magnitude(va.z, vb.z, vc.z)) * (8.0 * f64::EPSILON);
        let shifted = self.position + n * (n.x.abs() * e.x + n.y.abs() * e.y + n.z.abs() * e.z);
        let outward = |v: f64, n: f64| {
            if n > 0.0 {
                v.next_up()
            } else if n < 0.0 {
                v.next_down()
            } else {
                v
            }
        };
        Ray::new(Vec3::new(outward(shifted.x, n.x), outward(shifted.y, n.y), outward(shifted.z, n.z)), direction, 0.0_f64.next_up(), MAX_DISTANCE_MM)
    }
}
