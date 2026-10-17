//! The move/rotate triad shared by Move/Copy and occurrence moves: X/Y/Z arrows, XY/YZ/XZ plane
//! squares, X/Y/Z rotation rings and a centre ball for a free drag in the view plane. It is a
//! constant size on screen. A drag reports its change since the drag began (distances snap to
//! round steps at the current zoom, angles to 5°; hold Alt or Ctrl for a free drag), so the caller
//! adds it to the values it had when the drag started.

use std::cell::RefCell;

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, vec2};
use solidcraft_engine::geom::Vec3;

use crate::theme::Tokens;
use crate::viewport::{Proj, projected_polygon};

/// A part of the triad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    /// Arrow along X, Y or Z (0, 1, 2).
    Axis(usize),
    /// Square in the plane across that axis (Plane(2) is XY).
    Plane(usize),
    /// Ring turning about that axis.
    Ring(usize),
    /// Centre ball: free drag in the view plane.
    Ball,
}

/// What a drag has done so far.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    Translate(Vec3),
    /// About X, Y or Z through the centre, in radians.
    Rotate {
        axis: usize,
        angle: f64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub handle: Handle,
    pub change: Change,
    /// The first frame of the drag (take a snapshot of the values now).
    pub started: bool,
    /// The drag ended this frame.
    pub done: bool,
}

/// Where the triad is and which handles it shows.
#[derive(Clone, Copy, Debug)]
pub struct Triad {
    pub center: Vec3,
    pub translate: bool,
    pub rotate: bool,
}

#[derive(Clone, Copy, Debug)]
struct State {
    handle: Handle,
    center: Vec3,
    /// Grab point on the plane (planes, ball) or parameter along the axis.
    grab: Vec3,
    t0: f64,
    /// Ring: last angle seen and the angle turned so far.
    last: f64,
    turned: f64,
}

const DIRS: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];
/// Arrow length on screen (px).
pub const LEN_PX: f64 = 110.0;

thread_local! {
    /// The last triad drawn: centre and arrow length in mm (for scenarios and tests).
    static LAST: RefCell<Option<(Vec3, f64)>> = const { RefCell::new(None) };
}

/// A world point on a handle of the last triad drawn: "x"/"y"/"z" on an arrow, "xy"/"yz"/"xz"
/// in a square, "rx"/"ry"/"rz" on a ring, "ball". `by` moves it, `turn` (degrees) turns a ring
/// point about its axis.
pub fn handle_point(name: &str, by: Vec3, turn: f64) -> Option<Vec3> {
    let (c, l) = LAST.with(|x| *x.borrow())?;
    let axis = |s: &str| match s {
        "x" => Some(0),
        "y" => Some(1),
        "z" => Some(2),
        _ => None,
    };
    let p = match name {
        "ball" => c,
        "xy" => c + (Vec3::X + Vec3::Y) * (0.35 * l),
        "yz" => c + (Vec3::Y + Vec3::Z) * (0.35 * l),
        "xz" => c + (Vec3::X + Vec3::Z) * (0.35 * l),
        r if r.starts_with('r') => {
            let i = axis(&r[1..])?;
            let (e1, e2) = ring_basis(i);
            let th = turn.to_radians();
            return Some(c + (e1 * th.cos() + e2 * th.sin()) * (RING * l) + by);
        }
        a => c + DIRS[axis(a)?] * (0.75 * l),
    };
    Some(p + by)
}

/// Ring radius as a share of the arrow length.
const RING: f64 = 0.62;

/// In-plane directions of the ring about an axis (right-handed: e1 × e2 = axis).
fn ring_basis(i: usize) -> (Vec3, Vec3) {
    match i {
        0 => (Vec3::Y, Vec3::Z),
        1 => (Vec3::Z, Vec3::X),
        _ => (Vec3::X, Vec3::Y),
    }
}

/// Distance from `p` to the screen segment a–b.
fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

fn in_quad(p: Pos2, q: &[Pos2]) -> bool {
    if q.len() < 3 {
        return false;
    }
    let mut sign = 0.0f32;
    for i in 0..q.len() {
        let (a, b) = (q[i], q[(i + 1) % q.len()]);
        let c = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if c.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// Where the view ray through `pos` meets the plane through `o` with normal `n`.
fn on_plane(proj: &Proj, pos: Pos2, o: Vec3, n: Vec3) -> Option<Vec3> {
    let (ro, rd) = proj.ray(pos);
    let den = rd.dot(n);
    if den.abs() < 1e-9 {
        return None;
    }
    Some(ro + rd * ((o - ro).dot(n) / den))
}

/// Parameter along the line `o + d t` closest to the view ray through `pos`.
fn along(proj: &Proj, pos: Pos2, o: Vec3, d: Vec3) -> Option<f64> {
    let (ro, r) = proj.ray(pos);
    let w0 = o - ro;
    let (b, dd, e) = (d.dot(r), d.dot(w0), r.dot(w0));
    let den = 1.0 - b * b;
    if den.abs() < 1e-6 {
        return None;
    }
    Some((b * e - dd) / den)
}

/// The triad's shapes: (handle, screen polygon or polyline, is it a filled area).
struct Shapes {
    arrows: Vec<(usize, Pos2, Pos2)>,
    planes: Vec<(usize, Vec<Pos2>)>,
    rings: Vec<(usize, Vec<Pos2>)>,
    ball: Pos2,
}

fn shapes(proj: &Proj, t: &Triad, l: f64) -> Option<Shapes> {
    let c = t.center;
    let ball = proj.to_screen(c)?;
    let mut s = Shapes { arrows: Vec::new(), planes: Vec::new(), rings: Vec::new(), ball };
    if t.translate {
        for (i, d) in DIRS.iter().enumerate() {
            if let (Some(a), Some(b)) = (proj.to_screen(c + *d * (0.18 * l)), proj.to_screen(c + *d * l)) {
                s.arrows.push((i, a, b));
            }
        }
        for i in 0..3 {
            let (e1, e2) = ring_basis(i);
            let q: Vec<Pos2> = [(0.25, 0.25), (0.45, 0.25), (0.45, 0.45), (0.25, 0.45)]
                .iter()
                .filter_map(|(a, b)| proj.to_screen(c + e1 * (a * l) + e2 * (b * l)))
                .collect();
            if q.len() == 4 {
                s.planes.push((i, q));
            }
        }
    }
    if t.rotate {
        for i in 0..3 {
            let (e1, e2) = ring_basis(i);
            let pts: Vec<Pos2> = (0..=64)
                .map(|k| k as f64 / 64.0 * std::f64::consts::TAU)
                .filter_map(|th| proj.to_screen(c + (e1 * th.cos() + e2 * th.sin()) * (RING * l)))
                .collect();
            if pts.len() > 2 {
                s.rings.push((i, pts));
            }
        }
    }
    Some(s)
}

fn hit(s: &Shapes, p: Pos2) -> Option<Handle> {
    if s.ball.distance(p) < 9.0 {
        return Some(Handle::Ball);
    }
    if let Some((i, ..)) = s.arrows.iter().find(|(_, a, b)| seg_dist(p, *a, *b) < 6.0) {
        return Some(Handle::Axis(*i));
    }
    if let Some((i, _)) = s.planes.iter().find(|(_, q)| in_quad(p, q)) {
        return Some(Handle::Plane(*i));
    }
    s.rings.iter().find(|(_, pts)| pts.windows(2).any(|w| seg_dist(p, w[0], w[1]) < 5.0)).map(|(i, _)| Handle::Ring(*i))
}

/// Millimetres per screen pixel at a point.
fn mm_per_px(proj: &Proj, c: Vec3) -> Option<f64> {
    let (_, view) = proj.ray(proj.to_screen(c)?);
    let u = view.any_perp().normalized()?;
    let (a, b) = (proj.to_screen(c)?, proj.to_screen(c + u)?);
    let px = f64::from(a.distance(b));
    (px > 1e-9).then(|| 1.0 / px)
}

fn snap(v: f64, step: f64, free: bool) -> f64 {
    if free || step <= 0.0 { v } else { (v / step).round() * step }
}

/// Discard a suspended pointer gesture without reporting a final model change.
pub(crate) fn cancel(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|d| d.remove::<State>(id));
}

/// Hit-test before registering the dynamic egui drag widget or changing the camera.
pub(crate) fn contains_pointer(proj: &Proj, t: &Triad, p: Pos2) -> bool {
    mm_per_px(proj, t.center).and_then(|scale| shapes(proj, t, scale * LEN_PX)).is_some_and(|sh| hit(&sh, p).is_some())
}

/// Draw the triad and run a drag on it. `step` is the distance snap step.
pub fn show(ui: &mut egui::Ui, painter: &egui::Painter, proj: &Proj, id: egui::Id, t: &Triad, step: f64) -> Option<Drag> {
    let tk = Tokens::get();
    let l = mm_per_px(proj, t.center)? * LEN_PX;
    LAST.with(|x| *x.borrow_mut() = Some((t.center, l)));
    let sh = shapes(proj, t, l)?;
    // Handles by name for scenarios ("move_x", "move_rz", "move_ball"…).
    for n in ["x", "y", "z", "xy", "yz", "xz", "rx", "ry", "rz", "ball"] {
        if let Some(q) = handle_point(n, Vec3::ZERO, 60.0).and_then(|p| proj.to_screen(p)) {
            crate::scenario::publish_handle(&format!("move_{n}"), q);
        }
    }
    let state: Option<State> = ui.data(|d| d.get_temp(id));
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let hovered = if state.is_none() { pointer.filter(|_| ui.rect_contains_pointer(proj.rect)).and_then(|p| hit(&sh, p)) } else { None };
    let active = state.map(|s| s.handle).or(hovered);
    let hot = |h: Handle| active == Some(h);
    let col = |h: Handle, i: usize| if hot(h) { tk.accent } else { tk.gizmo[i.min(2)] };
    for (i, pts) in &sh.rings {
        painter.add(Shape::line(pts.clone(), Stroke::new(if hot(Handle::Ring(*i)) { 3.0 } else { 1.8 }, col(Handle::Ring(*i), *i))));
    }
    for (i, q) in &sh.planes {
        let c = col(Handle::Plane(*i), *i);
        painter.extend(projected_polygon(q, c.gamma_multiply(if hot(Handle::Plane(*i)) { 0.7 } else { 0.35 }), Stroke::new(1.0, c)));
    }
    for (i, a, b) in &sh.arrows {
        arrow(painter, *a, *b, col(Handle::Axis(*i), *i), hot(Handle::Axis(*i)));
    }
    let bc = if hot(Handle::Ball) { tk.accent } else { Color32::from_gray(235) };
    painter.circle(sh.ball, 6.0, bc, Stroke::new(1.5, Color32::from_gray(60)));
    let p = pointer?;
    active?;
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    let r = ui.interact(Rect::from_center_size(p, vec2(24.0, 24.0)), id.with("hit"), Sense::drag());
    // Free drag is driven by the per-frame thread-local set from prefs::manipulator_free_held.
    let free = crate::tool_modifiers::manipulator_free();
    if state.is_some() || r.dragged() || r.drag_started() {
        crate::tool_modifiers::set_manipulator_dragging(true);
    }
    let view = proj.ray(sh.ball).1;
    let plane_normal = |h: Handle| match h {
        Handle::Plane(i) | Handle::Ring(i) => DIRS[i],
        _ => view,
    };
    let mut started = false;
    let mut st = match state {
        Some(s) => s,
        None => {
            if !r.drag_started() {
                return None;
            }
            let h = hovered?;
            let from = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            let c = t.center;
            let mut s = State { handle: h, center: c, grab: c, t0: 0.0, last: 0.0, turned: 0.0 };
            match h {
                Handle::Axis(i) => s.t0 = along(proj, from, c, DIRS[i])?,
                Handle::Ring(i) => {
                    let q = on_plane(proj, from, c, DIRS[i])? - c;
                    let (e1, e2) = ring_basis(i);
                    s.last = q.dot(e2).atan2(q.dot(e1));
                }
                _ => s.grab = on_plane(proj, from, c, plane_normal(h))?,
            }
            started = true;
            s
        }
    };
    let change = match st.handle {
        Handle::Axis(i) => {
            let v = along(proj, p, st.center, DIRS[i]).map(|v| v - st.t0).unwrap_or(0.0);
            Change::Translate(DIRS[i] * snap(v, step, free))
        }
        Handle::Ring(i) => {
            if let Some(q) = on_plane(proj, p, st.center, DIRS[i]).map(|q| q - st.center) {
                let (e1, e2) = ring_basis(i);
                let th = q.dot(e2).atan2(q.dot(e1));
                let mut d = th - st.last;
                while d > std::f64::consts::PI {
                    d -= std::f64::consts::TAU;
                }
                while d < -std::f64::consts::PI {
                    d += std::f64::consts::TAU;
                }
                st.turned += d;
                st.last = th;
            }
            Change::Rotate { axis: i, angle: snap(st.turned, 5f64.to_radians(), free) }
        }
        h => {
            let d = on_plane(proj, p, st.center, plane_normal(h)).map(|q| q - st.grab).unwrap_or(Vec3::ZERO);
            Change::Translate(Vec3::new(snap(d.x, step, free), snap(d.y, step, free), snap(d.z, step, free)))
        }
    };
    let done = r.drag_stopped() || !r.dragged() && !started;
    if done {
        ui.data_mut(|d| d.remove::<State>(id));
    } else {
        ui.data_mut(|d| d.insert_temp(id, st));
    }
    Some(Drag { handle: st.handle, change, started, done })
}

fn arrow(painter: &egui::Painter, a: Pos2, b: Pos2, col: Color32, hot: bool) {
    let v = b - a;
    let len = v.length();
    if len < 1.0 {
        return;
    }
    let u = v / len;
    let n = vec2(-u.y, u.x);
    painter.line_segment([a, b], Stroke::new(if hot { 3.0 } else { 2.0 }, col));
    let head = 12.0f32.min(len * 0.5);
    painter.add(Shape::convex_polygon(vec![b + u * 2.0, b - u * head + n * head * 0.4, b - u * head - n * head * 0.4], col, Stroke::NONE));
}

/// A 3×3 rotation (rows) about a unit axis.
pub fn rotation(axis: Vec3, angle: f64) -> [[f64; 3]; 3] {
    let (c, s) = (angle.cos(), angle.sin());
    let t = 1.0 - c;
    let (x, y, z) = (axis.x, axis.y, axis.z);
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

pub fn mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

pub fn apply(m: &[[f64; 3]; 3], p: Vec3) -> Vec3 {
    Vec3::new(
        m[0][0] * p.x + m[0][1] * p.y + m[0][2] * p.z,
        m[1][0] * p.x + m[1][1] * p.y + m[1][2] * p.z,
        m[2][0] * p.x + m[2][1] * p.y + m[2][2] * p.z,
    )
}

/// The axis and angle of a rotation (angle 0 for none).
pub fn axis_angle(m: &[[f64; 3]; 3]) -> (Vec3, f64) {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let angle = ((tr - 1.0) / 2.0).clamp(-1.0, 1.0).acos();
    if angle < 1e-12 {
        return (Vec3::Z, 0.0);
    }
    let v = Vec3::new(m[2][1] - m[1][2], m[0][2] - m[2][0], m[1][0] - m[0][1]);
    if let Some(a) = v.normalized().filter(|_| v.len() > 1e-6) {
        return (a, angle);
    }
    // Half a turn: the axis from the diagonal.
    let d = [((m[0][0] + 1.0) / 2.0).max(0.0).sqrt(), ((m[1][1] + 1.0) / 2.0).max(0.0).sqrt(), ((m[2][2] + 1.0) / 2.0).max(0.0).sqrt()];
    let (x, mut y, mut z) = (d[0], d[1], d[2]);
    if m[0][1] + m[1][0] < 0.0 && x > 1e-6 {
        y = -y;
    }
    if m[0][2] + m[2][0] < 0.0 && x > 1e-6 {
        z = -z;
    }
    if x <= 1e-6 && m[1][2] + m[2][1] < 0.0 {
        z = -z;
    }
    (Vec3::new(x, y, z).normalized().unwrap_or(Vec3::Z), angle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_squares_stay_within_the_triad() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let t = Triad { center: Vec3::ZERO, translate: true, rotate: false };
        // The squares reach 0.45 √2 of the arrow length from the centre, plus stroke and feathering.
        let limit = (0.45 * 2f64.sqrt() * LEN_PX) as f32 + 3.0;
        let mut worst = 0.0f32;
        for dir in crate::viewport::orbit_directions() {
            let cam = solidcraft_engine::render::Camera::default().looking_from(dir);
            let proj = Proj { cam, vp: cam.view_proj(4.0 / 3.0, 100.0), rect };
            let l = mm_per_px(&proj, t.center).unwrap() * LEN_PX;
            let sh = shapes(&proj, &t, l).unwrap();
            for (_, q) in &sh.planes {
                for p in crate::viewport::drawn_points(projected_polygon(q, Color32::GRAY, Stroke::new(1.0, Color32::BLACK))) {
                    worst = worst.max(p.distance(sh.ball));
                }
            }
        }
        assert!(worst <= limit, "a vertex {worst} px from the centre (limit {limit})");
    }

    #[test]
    fn rotations_compose_to_one_axis_angle() {
        let r = mul(&rotation(Vec3::Z, 0.7), &rotation(Vec3::X, 0.4));
        let (a, ang) = axis_angle(&r);
        let back = rotation(a, ang);
        for i in 0..3 {
            for j in 0..3 {
                assert!((back[i][j] - r[i][j]).abs() < 1e-9);
            }
        }
        let h = rotation(Vec3::Y, std::f64::consts::PI);
        let (a, ang) = axis_angle(&h);
        assert!((ang - std::f64::consts::PI).abs() < 1e-9 && (a.y.abs() - 1.0).abs() < 1e-9);
        assert!((apply(&rotation(Vec3::Z, std::f64::consts::FRAC_PI_2), Vec3::X) - Vec3::Y).len() < 1e-12);
    }
}
