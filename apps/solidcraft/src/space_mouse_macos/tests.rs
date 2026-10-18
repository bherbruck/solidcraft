use super::*;
use solidcraft_engine::render::UpAxis;

fn close(a: Camera, b: Camera) {
    assert!(a.target.dist(b.target) < 1e-5, "{:?} != {:?}", a.target, b.target);
    assert!((a.distance - b.distance).abs() < 1e-7);
    for (a, b) in [a.basis().0, a.basis().1, a.basis().2].into_iter().zip([b.basis().0, b.basis().1, b.basis().2]) {
        assert!(a.dist(b) < 1e-5, "{a:?} != {b:?}");
    }
}

fn state() -> State {
    let mut state = State::new(egui::Context::default());
    state.enabled = true;
    state
}

#[test]
fn an_unavailable_native_driver_does_not_schedule_idle_ui_wakeups() {
    let ctx = egui::Context::default();
    let mut app = SolveApp::new(solidcraft_engine::Session::default(), solidcraft_ui_egui::Services::default());
    let mut mouse = SpaceMouse::start(&ctx);
    // Model a completed probe with no native connection. Do not register a real device reader
    // in a unit test; its event-driven wakeups belong to the existing fallback backend.
    mouse.initialized = true;
    for _ in 0..4 {
        let output = ctx.run_ui(egui::RawInput::default(), |ui| mouse.tick(&mut app, ui.ctx()));
        if ctx.cumulative_frame_nr() > 2 {
            assert!(output.viewport_output[&egui::ViewportId::ROOT].repaint_delay > Duration::from_secs(2));
        }
    }
    assert!(!app.viewport.space_mouse_driver_settings);
}

#[test]
fn driver_camera_roundtrips_roll_projection_and_both_up_axes() {
    for up in [UpAxis::Y, UpAxis::Z] {
        for fov in [0.0, 0.8] {
            for pitch in [-1.1, 0.0, 1.1, std::f64::consts::FRAC_PI_2] {
                let cam = Camera { target: Vec3::new(12.0, -23.0, 34.0), yaw: -0.7, pitch, roll: 0.9, distance: 175.0, fov, up };
                let result = camera_from_native(cam, affine(cam), extents(cam, 1.8), cam.distance).unwrap();
                close(cam, result);
            }
        }
    }
}

#[test]
fn orthographic_zoom_preserves_target_and_pan_survives_zoom() {
    let cam = Camera { target: Vec3::new(12.0, 34.0, 56.0), ..Camera::default() };
    let area = extents(Camera { distance: cam.distance * 0.5, ..cam }, 1.5);
    let zoomed = camera_from_native(cam, affine(cam), area, cam.distance).unwrap();
    assert!(zoomed.target.dist(cam.target) < 1e-9);
    assert_eq!(zoomed.distance, cam.distance * 0.5);
    let mut translated = affine(cam);
    translated[3][0] += 10.0;
    let panned = camera_from_native(zoomed, translated, area, cam.distance).unwrap();
    assert!(panned.target.dist(cam.target + Vec3::X * 10.0) < 1e-9);
}

#[test]
fn malformed_driver_cameras_never_replace_a_valid_camera() {
    let cam = Camera::default();
    let area = extents(cam, 1.0);
    for bad in [f64::NAN, f64::INFINITY, 1e20] {
        let mut matrix = affine(cam);
        matrix[3][0] = bad;
        assert!(camera_from_native(cam, matrix, area, cam.distance).is_none());
    }
    let mut scaled = affine(cam);
    scaled[0][0] *= 2.0;
    assert!(camera_from_native(cam, scaled, area, cam.distance).is_none());
    let mut state = state();
    let old = state.area;
    assert!(!state.set("view.extents", Value::Box([[0.0; 3]; 2])));
    assert!(!state.set("view.extents", Value::Box([[-1e10; 3], [1e10; 3]])));
    assert_eq!(state.area, old);
    assert!(state.pending.is_none());
}

#[test]
fn transaction_publishes_one_complete_pan_and_zoom() {
    let mut state = state();
    let cam = state.camera;
    assert!(state.set("transaction", Value::Long(1)));
    let mut matrix = affine(cam);
    matrix[3][0] += 10.0;
    assert!(state.set("view.affine", Value::Matrix(matrix)));
    assert!(state.set("view.extents", Value::Box(extents(Camera { distance: cam.distance * 0.5, ..cam }, 1.0))));
    assert!(state.pending.is_none());
    assert!(state.set("transaction", Value::Long(0)));
    let result = state.pending.unwrap();
    assert_eq!(result.distance, cam.distance * 0.5);
    assert!(result.target.dist(cam.target + Vec3::X * 10.0) < 1e-9);
}

#[test]
fn loss_of_focus_flushes_motion_camera_and_button_queue() {
    let mut state = state();
    state.pending = Some(state.camera);
    state.moving = true;
    state.commands.push("view.fit".into());
    let v = View { camera: state.camera, bounds: Aabb3::default(), selection: None, aspect: 2.0, rotatable: false, enabled: false };
    state.synchronize(&v);
    assert!(state.pending.is_none());
    assert!(!state.moving);
    assert!(state.commands.is_empty());
    assert!(!state.set("commands.activeCommand", Value::Text("view.fit".into())));
    assert!(!state.set("view.affine", Value::Matrix(affine(Camera::default()))));
    assert!(matches!(state.get("view.rotatable"), Some(Value::Bool(false))));
}

#[test]
fn enterprise_assignments_use_our_command_ids_and_ignore_releases() {
    let mut state = state();
    assert!(state.set("commands.activeCommand", Value::Text("view.fit".into())));
    assert!(state.set("commands.activeCommand", Value::Text(String::new())));
    assert!(state.set("commands.activeCommand", Value::Text("unknown.command".into())));
    assert_eq!(state.commands, ["view.fit"]);
    assert!(available_commands().iter().any(|c| c.0 == "solid.extrude"));
    for _ in 0..100 {
        state.set("commands.activeCommand", Value::Text("view.front".into()));
    }
    assert_eq!(state.commands.len(), 32);
}
