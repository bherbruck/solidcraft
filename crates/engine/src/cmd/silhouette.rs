//! A tightly framed, view-aligned union of the projected body meshes.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use super::CommandSpec;
use crate::params::bad;
use crate::render::{Camera, StandardView};
use crate::{Result, Session};

const CMD: &str = "inspect.silhouette";
const MAX_PIXELS: usize = 4 * 1024 * 1024;

// For agents comparing the model with a picture; nothing to show a person, so no toolbar button.
pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new(CMD, "Silhouette", run).icon("project").noundo().params(
    "view?: front|back|left|right|top|bottom|iso (default front); or camera?: {target: [x,y,z], yaw, pitch, distance, fov?} (mm, radians; fov 0 means parallel); bodies?: [world body names] (default visible bodies, named hidden bodies included); rows?: mask height in pixels (default 240, clamped to 16..1024) → tightly framed run-length mask, box_mm, mm_per_px, right/up, projection, body and triangle counts",
)];

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let camera = camera(p)?;
    let rows = rows(p)?;
    let wanted = body_names(p)?;
    let state = s.world_state();
    if let Some(names) = &wanted {
        for name in names {
            if !state.bodies.iter().any(|b| b.name == *name) {
                return Err(bad(CMD, format!("unknown body name `{name}`")));
            }
        }
    }
    let bodies: Vec<_> = state
        .bodies
        .iter()
        .filter(|b| wanted.as_ref().map_or_else(|| s.body_visible(&b.name), |names| names.iter().any(|name| name == &b.name)))
        .collect();
    if bodies.is_empty() {
        return Err(bad(CMD, "nothing visible to outline"));
    }

    let (right, up, back) = camera.basis();
    let perspective = camera.fov > 0.0;
    let origin_u = camera.target.dot(right);
    let origin_v = camera.target.dot(up);
    let project = |point: Vec3| -> Result<[f64; 2]> {
        let delta = point - camera.target;
        let scale = if perspective {
            let depth = camera.distance - delta.dot(back);
            if !depth.is_finite() || depth <= 0.0 {
                return Err(bad(
                    CMD,
                    "part of the model is at or behind the camera: move the camera back, or name the bodies in front of it in `bodies`",
                ));
            }
            camera.distance / depth
        } else {
            1.0
        };
        let uv = [origin_u + delta.dot(right) * scale, origin_v + delta.dot(up) * scale];
        if uv.iter().any(|v| !v.is_finite()) {
            return Err(bad(CMD, "projected model coordinates are not finite"));
        }
        Ok(uv)
    };

    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut meshes = Vec::with_capacity(bodies.len());
    let mut triangle_count = 0usize;
    for body in &bodies {
        let mesh = body.mesh();
        triangle_count = triangle_count.checked_add(mesh.triangles.len()).ok_or_else(|| bad(CMD, "too many triangles"))?;
        let mut projected = Vec::with_capacity(mesh.positions.len());
        for &point in &mesh.positions {
            let uv = project(point)?;
            for axis in 0..2 {
                lo[axis] = lo[axis].min(uv[axis]);
                hi[axis] = hi[axis].max(uv[axis]);
            }
            projected.push(uv);
        }
        meshes.push((mesh, projected));
    }
    let u_span = hi[0] - lo[0];
    let v_span = hi[1] - lo[1];
    if !u_span.is_finite() || !v_span.is_finite() || (u_span <= 0.0 && v_span <= 0.0) {
        return Err(bad(CMD, "nothing visible to outline"));
    }
    let mm_per_px = (v_span / rows as f64).max(u_span / (4 * rows) as f64);
    if !mm_per_px.is_finite() || mm_per_px <= 0.0 {
        return Err(bad(CMD, "outline scale is invalid"));
    }
    let width = dimension(u_span, mm_per_px)?;
    let height = dimension(v_span, mm_per_px)?;
    let pixels = width.checked_mul(height).filter(|n| *n <= MAX_PIXELS).ok_or_else(|| bad(CMD, "outline exceeds the pixel limit"))?;
    if width == 0 || height == 0 {
        return Err(bad(CMD, "nothing visible to outline"));
    }

    let mut covered = vec![false; pixels];
    for (mesh, projected) in &meshes {
        for indices in &mesh.triangles {
            // Meshes can come from imported data; never trust triangle indices.
            let (Some(a), Some(b), Some(c)) =
                (projected.get(indices[0] as usize), projected.get(indices[1] as usize), projected.get(indices[2] as usize))
            else {
                continue;
            };
            let tri = [*a, *b, *c].map(|q| [(q[0] - lo[0]) / mm_per_px, (hi[1] - q[1]) / mm_per_px]);
            if tri.iter().flat_map(|p| p.iter()).any(|v| !v.is_finite()) {
                return Err(bad(CMD, "projected triangle coordinates are not finite"));
            }
            let [a, b, c] = tri;
            let area = edge(a, b, c);
            if area == 0.0 || !area.is_finite() {
                continue;
            }
            let x0 = a[0].min(b[0]).min(c[0]).floor().clamp(0.0, width as f64) as usize;
            let x1 = a[0].max(b[0]).max(c[0]).ceil().clamp(0.0, width as f64) as usize;
            let y0 = a[1].min(b[1]).min(c[1]).floor().clamp(0.0, height as f64) as usize;
            let y1 = a[1].max(b[1]).max(c[1]).ceil().clamp(0.0, height as f64) as usize;
            for y in y0..y1 {
                for x in x0..x1 {
                    let centre = [x as f64 + 0.5, y as f64 + 0.5];
                    let e = [edge(a, b, centre), edge(b, c, centre), edge(c, a, centre)];
                    if (e.iter().all(|v| *v >= -1e-9) || e.iter().all(|v| *v <= 1e-9))
                        && let Some(i) = y.checked_mul(width).and_then(|base| base.checked_add(x))
                        && let Some(pixel) = covered.get_mut(i)
                    {
                        *pixel = true;
                    }
                }
            }
        }
    }

    let mask: Vec<Vec<[usize; 2]>> = covered.chunks_exact(width).map(spans).collect();
    Ok(json!({
        "width": width,
        "height": height,
        "mm_per_px": round4(mm_per_px),
        "box_mm": [[round4(lo[0]), round4(lo[1])], [round4(hi[0]), round4(hi[1])]],
        "right": [clean(right.x), clean(right.y), clean(right.z)],
        "up": [clean(up.x), clean(up.y), clean(up.z)],
        "projection": if perspective { "perspective" } else { "parallel" },
        "bodies": bodies.len(),
        "triangles": triangle_count,
        "mask": mask,
    }))
}

fn camera(p: &Value) -> Result<Camera> {
    if p.get("view").is_some() && p.get("camera").is_some() {
        return Err(bad(CMD, "give `view` or `camera`, not both"));
    }
    let camera = if let Some(value) = p.get("camera") {
        let named: solidcraft_doc::NamedView = serde_json::from_value(value.clone()).map_err(|e| bad(CMD, format!("bad camera: {e}")))?;
        Camera { target: named.target, yaw: named.yaw, pitch: named.pitch, distance: named.distance, fov: named.fov, ..Default::default() }
    } else {
        let name = match p.get("view") {
            None => "front",
            Some(Value::String(name)) => name,
            _ => return Err(bad(CMD, "`view` must be a standard view name")),
        };
        let view = StandardView::parse(name).ok_or_else(|| bad(CMD, format!("unknown standard view `{name}`")))?;
        let (yaw, pitch) = view.angles();
        Camera { target: Vec3::ZERO, yaw, pitch, distance: 200.0, fov: 0.0, ..Default::default() }
    };
    if !camera.is_valid() || !camera.fov.is_finite() || camera.fov < 0.0 {
        return Err(bad(CMD, "the camera must be finite with a positive distance"));
    }
    Ok(camera)
}

fn body_names(p: &Value) -> Result<Option<Vec<String>>> {
    match p.get("bodies") {
        None => Ok(None),
        Some(Value::String(name)) => Ok(Some(vec![name.clone()])),
        Some(Value::Array(names)) if names.len() <= 10_000 => names
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(CMD, "`bodies` must contain names")))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        _ => Err(bad(CMD, "`bodies` must be a list of names")),
    }
}

fn rows(p: &Value) -> Result<usize> {
    match p.get("rows") {
        None => Ok(240),
        Some(v) => {
            let n = v.as_i64().or_else(|| v.as_u64().map(|n| n.min(i64::MAX as u64) as i64)).ok_or_else(|| bad(CMD, "`rows` must be an integer"))?;
            Ok(n.clamp(16, 1024) as usize)
        }
    }
}

fn dimension(span: f64, mm_per_px: f64) -> Result<usize> {
    let ratio = span / mm_per_px;
    if !ratio.is_finite() || ratio > MAX_PIXELS as f64 {
        return Err(bad(CMD, "outline exceeds the pixel limit"));
    }
    // Exact axis views pick up a few ulps from sin(π) and cos(π/2).
    let nearest = ratio.round();
    let stable = if (ratio - nearest).abs() < 1e-9 { nearest } else { ratio };
    Ok(stable.ceil() as usize)
}

fn edge(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

fn spans(row: &[bool]) -> Vec<[usize; 2]> {
    let mut out = Vec::new();
    let mut start = None;
    for (x, &on) in row.iter().enumerate() {
        if on && start.is_none() {
            start = Some(x);
        } else if !on && let Some(first) = start.take() {
            out.push([first, x]);
        }
    }
    if let Some(first) = start {
        out.push([first, row.len()]);
    }
    out
}

fn round4(value: f64) -> f64 {
    if value.abs() < 1e12 { (value * 10_000.0).round() / 10_000.0 } else { value }
}

fn clean(value: f64) -> f64 {
    if value.abs() < 1e-12 { 0.0 } else { value }
}
