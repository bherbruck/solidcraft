//! macOS 3DxWare navigation. The installed Navigation Library calculates the camera, including
//! object/target/fly modes, axis settings, horizon lock and automatic/selection rotation centers.
//! Only `ffi` crosses the native boundary; callbacks exchange snapshots, never the app or document.

mod ffi;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use solidcraft_engine::geom::{Aabb3, Vec3};
use solidcraft_engine::render::{Camera, StandardView};
use solidcraft_ui_egui::SolveApp;
use solidcraft_ui_egui::space_mouse::{View, WheelGuard, navigate_camera_frame, view};

const DRIVER: &str = "/Library/Frameworks/3DconnexionNavlib.framework/3DconnexionNavlib";

/// Native row-vector camera-to-world matrix: right/up/back/eye rows, with translation in the
/// last row. Keep NlCreate's default layout; column_vectors would require transposing all values.
type Matrix = [[f64; 4]; 4];
type Bounds = [[f64; 3]; 2];

fn xyz(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

fn vec(v: [f64; 3]) -> Vec3 {
    Vec3::new(v[0], v[1], v[2])
}

fn affine(cam: Camera) -> Matrix {
    let (r, u, b) = cam.basis();
    let e = cam.eye();
    [[r.x, r.y, r.z, 0.0], [u.x, u.y, u.z, 0.0], [b.x, b.y, b.z, 0.0], [e.x, e.y, e.z, 1.0]]
}

fn bounds(b: Aabb3, cam: Camera) -> Bounds {
    if b.is_empty() {
        let half = Vec3::new(1.0, 1.0, 1.0);
        [xyz(cam.target - half), xyz(cam.target + half)]
    } else {
        [xyz(b.min), xyz(b.max)]
    }
}

fn extents(cam: Camera, aspect: f64) -> Bounds {
    let half = cam.half_height();
    let aspect = if aspect.is_finite() { aspect.clamp(0.01, 100.0) } else { 1.0 };
    [[-half * aspect, -half, -cam.distance * 10.0], [half * aspect, half, cam.distance * 10.0]]
}

/// Interpret a rigid camera pose while retaining independent orthographic scale. The native
/// library can zoom an orthographic view without changing its eye; SolidCraft encodes that scale
/// in `distance`, so the virtual eye depth is kept separately until pointer navigation takes over.
fn camera_from_native(base: Camera, matrix: Matrix, area: Bounds, eye_depth: f64) -> Option<Camera> {
    if !matrix.iter().flatten().chain(area.iter().flatten()).all(|v| v.is_finite() && v.abs() <= 1e12) {
        return None;
    }
    let column = |i: usize| Vec3::new(matrix[i][0], matrix[i][1], matrix[i][2]);
    let (r, u, b, eye) = (column(0), column(1), column(2), column(3));
    if [r, u, b].iter().any(|v| (v.len() - 1.0).abs() > 1e-4)
        || r.dot(u).abs() > 1e-4
        || r.cross(u).dist(b) > 1e-4
        || matrix[0][3].abs() > 1e-9
        || matrix[1][3].abs() > 1e-9
        || matrix[2][3].abs() > 1e-9
        || (matrix[3][3] - 1.0).abs() > 1e-9
    {
        return None;
    }
    let mut cam = base.looking_from(b);
    let (zero_r, zero_u, _) = cam.basis();
    cam.roll = r.dot(zero_u).atan2(r.dot(zero_r));
    let depth = if base.fov > 0.0 { (eye - base.target).dot(b).max(1e-3) } else { eye_depth };
    cam.target = eye - b * depth;
    cam.distance = if base.fov > 0.0 { depth } else { (area[1][1] - area[0][1]) / 0.8 };
    (cam.is_valid() && (1e-3..=1e7).contains(&cam.distance)).then_some(cam)
}

/// Owned values, independent of the native union and its pointers.
#[derive(Clone, Debug)]
enum Value {
    Bool(bool),
    Long(i64),
    Double(f64),
    Point([f64; 3]),
    Plane([f64; 4]),
    Frustum([f64; 6]),
    Box(Bounds),
    Matrix(Matrix),
    Text(String),
}

struct State {
    ctx: egui::Context,
    camera: Camera,
    matrix: Matrix,
    area: Bounds,
    eye_depth: f64,
    model: Bounds,
    selection: Option<Bounds>,
    rotatable: bool,
    enabled: bool,
    moving: bool,
    dirty: bool,
    transaction: bool,
    pending: Option<Camera>,
    last_applied: Camera,
    aspect: f64,
    commands: Vec<String>,
    pivot: [f64; 3],
}

impl State {
    fn new(ctx: egui::Context) -> Self {
        let camera = Camera::default();
        Self {
            ctx,
            camera,
            matrix: affine(camera),
            area: extents(camera, 1.0),
            eye_depth: camera.distance,
            model: bounds(Aabb3::default(), camera),
            selection: None,
            rotatable: true,
            enabled: false,
            moving: false,
            dirty: false,
            transaction: false,
            pending: None,
            last_applied: camera,
            aspect: 1.0,
            commands: Vec::new(),
            pivot: xyz(camera.target),
        }
    }

    fn synchronize(&mut self, v: &View) {
        // Keep the native pose through a motion; reset it when a pointer, view button, sketch
        // transition or another canvas changed the app camera, or navigation was suspended.
        if v.camera != self.last_applied || !v.enabled || v.rotatable != self.rotatable {
            self.camera = v.camera;
            self.last_applied = v.camera;
            self.matrix = affine(v.camera);
            self.area = extents(v.camera, v.aspect);
            self.eye_depth = v.camera.distance;
            self.pending = None;
            self.dirty = false;
        }
        self.model = bounds(v.bounds, v.camera);
        if v.aspect != self.aspect {
            let half_width = (self.area[1][1] - self.area[0][1]) * 0.5 * v.aspect.clamp(0.01, 100.0);
            self.area[0][0] = -half_width;
            self.area[1][0] = half_width;
            self.aspect = v.aspect;
        }
        self.selection = v.selection.map(|b| bounds(b, v.camera));
        self.rotatable = v.rotatable;
        self.enabled = v.enabled;
        if !v.enabled {
            self.moving = false;
            self.commands.clear();
        }
    }

    fn get(&self, name: &str) -> Option<Value> {
        Some(match name {
            "view.affine" => Value::Matrix(self.matrix),
            "view.extents" => Value::Box(self.area),
            "view.perspective" => Value::Bool(self.camera.fov > 0.0),
            "view.fov" => Value::Double(self.camera.fov),
            "view.frustum" => {
                let radius = vec(self.model[1]).dist(vec(self.model[0])) * 0.5;
                let (near, far) = self.camera.clip(radius);
                let top = near * (self.camera.fov * 0.5).tan();
                let right = top * self.aspect;
                Value::Frustum([-right, right, -top, top, near, far])
            }
            "view.rotatable" => Value::Bool(self.rotatable),
            "view.target" => Value::Point(xyz(self.camera.target)),
            "view.focusDistance" => Value::Double(self.camera.distance),
            "model.extents" => Value::Box(self.model),
            "selection.empty" => Value::Bool(self.selection.is_none()),
            "selection.extents" => Value::Box(self.selection?),
            "pivot.position" => Value::Point(self.pivot),
            "pivot.user" => Value::Bool(false),
            "views.front" => {
                let mut front = self.camera;
                front.set_view(StandardView::Front);
                let mut m = affine(front);
                m[3] = [0.0, 0.0, 0.0, 1.0];
                Value::Matrix(m)
            }
            "coordinateSystem" => {
                // World axes in the library's X-right/Y-up/Z-out coordinate system. Respect
                // the user's Y-up/Z-up choice, including Lock Horizon's reference direction.
                let x = self.camera.to_frame(Vec3::X);
                let y = self.camera.to_frame(Vec3::Y);
                let z = self.camera.to_frame(Vec3::Z);
                Value::Matrix([[x.x, x.z, -x.y, 0.0], [y.x, y.z, -y.y, 0.0], [z.x, z.z, -z.y, 0.0], [0.0, 0.0, 0.0, 1.0]])
            }
            "view.constructionPlane" if !self.rotatable => {
                let b = self.camera.back();
                Value::Plane([b.x, b.y, b.z, -b.dot(self.camera.target)])
            }
            _ => return None,
        })
    }

    fn set(&mut self, name: &str, value: Value) -> bool {
        if !self.enabled {
            return false;
        }
        match (name, value) {
            ("view.affine", Value::Matrix(m)) => {
                if camera_from_native(self.camera, m, self.area, self.eye_depth).is_none() {
                    log::trace!("3D mouse: rejected affine {m:?}");
                    return false;
                }
                self.matrix = m;
                self.dirty = true;
            }
            ("view.extents", Value::Box(b)) => {
                if b[1][0] <= b[0][0] || b[1][1] <= b[0][1] || camera_from_native(self.camera, self.matrix, b, self.eye_depth).is_none() {
                    return false;
                }
                self.area = b;
                self.dirty = true;
            }
            ("view.fov", Value::Double(f)) if f.is_finite() && (0.01..3.0).contains(&f) => {
                self.camera.fov = f;
                self.dirty = true;
            }
            ("view.target", Value::Point(p)) if vec(p).is_finite() => self.camera.target = vec(p),
            ("pivot.position", Value::Point(p)) if vec(p).is_finite() => self.pivot = p,
            ("pivot.visible", Value::Bool(_)) => {}
            ("motion", Value::Bool(moving)) => self.moving = moving,
            ("transaction", Value::Long(n)) => self.transaction = n != 0,
            ("commands.activeCommand", Value::Text(command)) => {
                // Releases are empty strings. Bound the queue if the UI is busy and accept only
                // commands we advertised, never arbitrary input or another app's command IDs.
                if !command.is_empty() && self.commands.len() < 32 && available_commands().iter().any(|c| c.0 == command) {
                    self.commands.push(command);
                }
            }
            _ => return false,
        }
        if self.dirty && !self.transaction {
            if let Some(cam) = camera_from_native(self.camera, self.matrix, self.area, self.eye_depth) {
                self.camera = cam;
                self.pending = Some(cam);
            }
            self.dirty = false;
        }
        self.ctx.request_repaint();
        true
    }
}

/// The same UI entry points as toolbar clicks. Use stable SolidCraft identifiers for 3DxWare's
/// assignments. Include navigation explicitly: view.navigate's JSON interface isn't a UI tool.
fn available_commands() -> Vec<(&'static str, &'static str)> {
    let mut commands = vec![
        ("view.fit", "Fit"),
        ("view.home", "Home"),
        ("view.front", "Front"),
        ("view.back", "Back"),
        ("view.left", "Left"),
        ("view.right", "Right"),
        ("view.top", "Top"),
        ("view.bottom", "Bottom"),
    ];
    commands.extend(solidcraft_engine::command_specs().into_iter().filter(|c| !c.tab.is_empty() && c.planned.is_none()).map(|c| (c.id, c.label)));
    commands
}

pub struct SpaceMouse {
    shared: Arc<Mutex<State>>,
    connection: Option<ffi::Connection>,
    initialized: bool,
    fallback: Option<super::space_mouse::SpaceMouse>,
    focused: bool,
    sketch_bank: Option<bool>,
    wheel: WheelGuard,
    started: Instant,
}

impl SpaceMouse {
    pub fn start(ctx: &egui::Context) -> Self {
        Self {
            shared: Arc::new(Mutex::new(State::new(ctx.clone()))),
            connection: None,
            initialized: false,
            fallback: None,
            focused: false,
            sketch_bank: None,
            wheel: WheelGuard::default(),
            started: Instant::now(),
        }
    }

    pub fn raw_input_hook(&mut self, enabled: bool, raw: &mut egui::RawInput) {
        if let Some(fallback) = &mut self.fallback {
            fallback.raw_input_hook(enabled, raw);
            return;
        }
        let moving = self.shared.lock().map(|s| s.moving && s.enabled).unwrap_or(false);
        if enabled {
            self.wheel.filter(moving, self.started.elapsed().as_secs_f64(), &mut raw.events);
        }
    }

    pub fn tick(&mut self, app: &mut SolveApp, ctx: &egui::Context) {
        app.viewport.space_mouse_driver_settings = self.connection.is_some();
        if let Some(fallback) = &mut self.fallback {
            if !app.file_picker_busy() {
                fallback.tick(app, ctx);
            }
            return;
        }
        let v = view(app, ctx);
        let (pending, commands, interrupted, up_axis) = match self.shared.lock() {
            Ok(mut state) => {
                let interrupted = state.moving && (v.camera != state.last_applied || v.rotatable != state.rotatable);
                let changed_up = state.camera.up != v.camera.up;
                state.synchronize(&v);
                let pending = state.pending.take();
                if let Some(cam) = pending {
                    state.last_applied = cam;
                }
                let up_axis = if changed_up {
                    match (state.get("coordinateSystem"), state.get("views.front")) {
                        (Some(Value::Matrix(coordinates)), Some(Value::Matrix(front))) => Some((coordinates, front)),
                        _ => None,
                    }
                } else {
                    None
                };
                (pending, std::mem::take(&mut state.commands), interrupted, up_axis)
            }
            Err(_) => return,
        };
        if let Some(cam) = pending {
            navigate_camera_frame(app, ctx, cam);
        }
        if v.enabled {
            for command in commands {
                if let Some(which) = command.strip_prefix("view.") {
                    app.animate_view(which);
                } else {
                    app.start(&command);
                }
            }
        }
        // Probe once at startup. Installing/replacing a driver takes effect on the next launch;
        // Macs without Navlib must not schedule periodic UI repaints or framework-path checks.
        if !self.initialized {
            self.initialized = true;
            if std::path::Path::new(DRIVER).exists() {
                match ffi::Connection::open(DRIVER, Arc::clone(&self.shared), &available_commands()) {
                    Ok(connection) => {
                        log::info!("3D mouse: connected to the macOS 3DxWare Navigation Library");
                        self.connection = Some(connection);
                        self.sketch_bank = None;
                        self.focused = !v.enabled;
                        app.viewport.space_mouse_driver_settings = true;
                    }
                    Err(error) => log::warn!("3D mouse: {error}"),
                }
            }
            if self.connection.is_none() {
                // Retain the driver-client/HID backend from #1815, but never register both
                // native clients at once or apply the same cap motion twice.
                self.fallback = Some(super::space_mouse::SpaceMouse::start(ctx));
            }
        }
        if let Some(connection) = &self.connection
            && self.focused != v.enabled
        {
            connection.focus(v.enabled);
            self.focused = v.enabled;
        }
        if let Some(connection) = &self.connection {
            let sketch = app.session.active_sketch.is_some();
            if self.sketch_bank != Some(sketch) {
                connection.command_set(sketch);
                self.sketch_bank = Some(sketch);
            }
            if interrupted {
                connection.interrupt();
            }
            if let Some((coordinates, front)) = up_axis {
                connection.up_axis(coordinates, front);
            }
        }
        if self.shared.lock().map(|s| s.moving && s.enabled).unwrap_or(false) {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

#[cfg(test)]
mod tests;
