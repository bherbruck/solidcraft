//! 3D mouse input, shared by the desktop and web apps. Each app reads the device its own way (the
//! desktop through HID, the web app through the browser's Gamepad API) and hands its six raw axes
//! to [`navigate_frame`] once per frame. This module maps them, applies the dead zone and drops
//! 3DxWare's emulated wheel; the page showing the camera moves it (`navigation::device`), so the
//! cap does what that page's camera may do.

use std::time::Duration;

use crate::SolveApp;

/// What a 3Dconnexion device reports at full deflection, in raw units, on every axis.
pub const FULL_SCALE: i16 = 350;

/// One reading of the six axes in the device's raw units: X right, Y towards the user, Z down,
/// and the rotations about those axes (right-handed).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Axes {
    pub translation: [i16; 3],
    pub rotation: [i16; 3],
}

impl Axes {
    /// Some axis is past the dead zone: the cap is being moved.
    pub fn moving(self) -> bool {
        self.translation.iter().chain(&self.rotation).any(|&v| normalized(v) != 0.0)
    }
}

/// Seconds after the cap last moved that wheel events are still taken for 3DxWare's: one can be
/// queued just before the cap stops and only reach the next frame.
pub const WHEEL_QUIET_S: f64 = 0.15;

/// 3DxWare turns the cap's motion into mouse-wheel turns for programs it has no profile for (its
/// keyboard and mouse emulation), browsers included (#1336, #1380). Both apps read the cap
/// themselves, so those are dropped: wheel events while the cap is moving and for
/// [`WHEEL_QUIET_S`] after. 3DxWare sends them only well past the dead zone (from about a third
/// of full deflection), never at rest, so a wheel turn with the cap at rest is the user's.
#[derive(Debug, Default)]
pub struct WheelGuard {
    moved_at: Option<f64>,
}

impl WheelGuard {
    /// Takes this frame's reading (`now` in seconds) and drops the frame's wheel events if they
    /// are 3DxWare's.
    pub fn filter(&mut self, moving: bool, now: f64, events: &mut Vec<egui::Event>) {
        if moving {
            self.moved_at = Some(now);
        }
        let recent = self.moved_at.is_some_and(|at| (0.0..=WHEEL_QUIET_S).contains(&(now - at)));
        if recent {
            events.retain(|e| !matches!(e, egui::Event::MouseWheel { .. } | egui::Event::Zoom(_)));
        }
    }
}

/// Where one physical axis goes: its logical axis, and how.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisRule {
    pub output: usize,
    pub enabled: bool,
    pub reversed: bool,
    pub scale: f64,
}

/// The physical-to-logical axis mapping 3DxWare applies (the desktop reads it from the driver's
/// profile; direct reads bypass the driver).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriverMapping {
    pub axes: [AxisRule; 6],
    pub overall_scale: f64,
}

impl Default for DriverMapping {
    fn default() -> Self {
        Self { axes: std::array::from_fn(|output| AxisRule { output, enabled: true, reversed: false, scale: 1.0 }), overall_scale: 1.0 }
    }
}

impl DriverMapping {
    /// What 3DxWare applies by default in object mode (#1172): the physical Ry and Rz swapped and
    /// reversed, so a tilt left/right rolls and a twist spins.
    pub fn object_mode() -> Self {
        let mut mapping = Self::default();
        mapping.axes[4] = AxisRule { output: 5, enabled: true, reversed: true, scale: 1.0 };
        mapping.axes[5] = AxisRule { output: 4, enabled: true, reversed: true, scale: 1.0 };
        mapping
    }

    /// The mapping with the user's per-axis choices folded in: `reverse` flips and `speed` scales
    /// each physical axis of the cap. A speed that is not a finite number counts as 1.
    pub fn with_user_axes(mut self, reverse: [bool; 6], speed: [f32; 6]) -> Self {
        for ((rule, reverse), speed) in self.axes.iter_mut().zip(reverse).zip(speed) {
            rule.reversed ^= reverse;
            rule.scale *= if speed.is_finite() { f64::from(speed).clamp(0.0, 3.0) } else { 1.0 };
        }
        self
    }

    pub fn apply(self, axes: Axes) -> Motion {
        let raw = [
            normalized(axes.translation[0]),
            normalized(axes.translation[1]),
            normalized(axes.translation[2]),
            normalized(axes.rotation[0]),
            normalized(axes.rotation[1]),
            normalized(axes.rotation[2]),
        ];
        let mut logical = [0.0; 6];
        for (input, rule) in self.axes.iter().enumerate() {
            if rule.enabled {
                let sign = if rule.reversed { -1.0 } else { 1.0 };
                if let Some(slot) = logical.get_mut(rule.output) {
                    *slot = raw[input] * sign * rule.scale * self.overall_scale;
                }
            }
        }
        Motion { translation: [logical[0], logical[1], logical[2]], rotation: [logical[3], logical[4], logical[5]] }
    }
}

/// The logical axes after the mapping, each in -1..1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    pub translation: [f64; 3],
    pub rotation: [f64; 3],
}

/// One frame of 3D mouse motion, waiting for the page that shows the camera: the logical axes,
/// and how many seconds of motion they stand for (the frame's time, scaled by the speed).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeviceMotion {
    pub motion: Motion,
    pub seconds: f64,
}

/// Hands one frame of 3D mouse input, `seconds` after the previous frame, to the page showing
/// the camera. Nothing happens with navigation switched off, on the start page or while a file
/// picker is open. Returns whether there was motion to hand over; it then asks for the next frame
/// itself.
pub fn navigate_frame(app: &mut SolveApp, ctx: &egui::Context, mapping: DriverMapping, axes: Axes, seconds: f64) -> bool {
    if !app.preferences.space_mouse_enabled || app.home.open || app.file_picker_busy() || axes == Axes::default() {
        return false;
    }
    let speed = f64::from(app.preferences.space_mouse_speed);
    if !speed.is_finite() {
        return false;
    }
    app.begin_external_navigation();
    // A long gap (the first frame after a pause) moves as far as a 50 ms one.
    let step = if seconds.is_finite() { seconds.min(0.05) } else { 0.0 };
    app.viewport.device_motion = Some(DeviceMotion {
        motion: mapping.with_user_axes(app.preferences.space_mouse_reverse, app.preferences.space_mouse_axis_speed).apply(axes),
        seconds: step * speed.clamp(0.1, 5.0),
    });
    ctx.request_repaint_after(Duration::from_millis(16));
    true
}

/// A small dead zone avoids drift at rest. A full cap deflection takes roughly one viewport
/// per second to pan and about two radians per second to turn, independent of frame rate.
fn normalized(value: i16) -> f64 {
    let v = f64::from(value);
    if v.abs() <= 20.0 { 0.0 } else { v.signum() * ((v.abs() - 20.0) / 330.0).min(1.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use solidcraft_engine::Session;
    use solidcraft_engine::render::Camera;

    /// The camera math the page applies the motion with (`navigation::device`): `sketch` keeps
    /// the orientation, as editing a sketch does.
    fn navigate(cam: &mut Camera, motion: Motion, seconds: f64, height: f64, sketch: bool, reverse_zoom: bool) {
        crate::navigation::device_camera(cam, motion, seconds, height, !sketch, reverse_zoom);
    }

    /// One frame: the 3D mouse hands its motion over, and the modelling viewport applies it.
    fn frame(app: &mut SolveApp, ctx: &egui::Context, mapping: DriverMapping, axes: Axes, seconds: f64) -> bool {
        let handed = navigate_frame(app, ctx, mapping, axes, seconds);
        let page = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        crate::navigation::device(app, page, crate::navigation::NavCaps::MODEL);
        handed
    }

    fn raw(axes: Axes) -> Motion {
        DriverMapping::default().apply(axes)
    }

    fn app() -> SolveApp {
        let mut app = SolveApp::new(Session::default(), Services::default());
        app.home.open = false;
        app
    }

    #[test]
    fn per_axis_choices_flip_and_scale_one_physical_axis() {
        let mapping = DriverMapping::default().with_user_axes([false, true, false, false, false, false], [1.0, 1.0, 1.0, 0.5, f32::NAN, 1.0]);
        let still = Motion::default();
        assert_eq!(mapping.apply(Axes { translation: [0, 350, 0], rotation: [0; 3] }).translation[1], -raw(PUSH).translation[1]);
        assert_eq!(
            mapping.apply(Axes { translation: [350, 0, 0], rotation: [0; 3] }).translation[0],
            raw(Axes { translation: [350, 0, 0], rotation: [0; 3] }).translation[0]
        );
        assert_eq!(mapping.apply(Axes { translation: [0; 3], rotation: [350, 0, 0] }).rotation[0], 0.5);
        assert_eq!(mapping.apply(Axes { translation: [0; 3], rotation: [0, 350, 0] }).rotation[1], 1.0);
        assert_eq!(mapping.apply(Axes::default()), still);
        let off = DriverMapping::default().with_user_axes([false; 6], [0.0; 6]);
        assert_eq!(off.apply(PUSH), still);
    }

    const PUSH: Axes = Axes { translation: [0, 350, 0], rotation: [0; 3] };

    fn wheel(dy: f32) -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, dy),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        }
    }

    fn other() -> egui::Event {
        egui::Event::PointerMoved(egui::pos2(1.0, 2.0))
    }

    #[test]
    fn six_axes_move_the_camera_and_sketches_keep_their_orientation() {
        let mut cam = Camera::default();
        let before = cam;
        navigate(&mut cam, raw(Axes { translation: [350, 350, 350], rotation: [350, 350, 350] }), 0.02, 600.0, false, false);
        assert_ne!(cam.target, before.target);
        assert!(cam.distance < before.distance);
        assert_ne!(cam.yaw, before.yaw);
        assert_ne!(cam.pitch, before.pitch);
        assert_ne!(cam.roll, before.roll);
        let orientation = (cam.yaw, cam.pitch, cam.roll);
        navigate(&mut cam, raw(Axes { translation: [350, 0, 0], rotation: [350, 350, 350] }), 0.02, 600.0, true, false);
        assert_eq!((cam.yaw, cam.pitch, cam.roll), orientation);
        assert!(cam.is_valid());
    }

    #[test]
    fn individual_axes_have_physical_spacemouse_meaning() {
        let before = Camera::default();

        let mut slide = before;
        navigate(&mut slide, raw(Axes { translation: [350, 0, 0], rotation: [0; 3] }), 0.02, 600.0, false, false);
        assert_ne!(slide.target, before.target);
        assert_eq!(slide.distance, before.distance);

        let mut push = before;
        navigate(&mut push, raw(Axes { translation: [0, 350, 0], rotation: [0; 3] }), 0.02, 600.0, false, false);
        assert_eq!(push.target, before.target);
        assert!(push.distance < before.distance);

        let mut lift = before;
        navigate(&mut lift, raw(Axes { translation: [0, 0, 350], rotation: [0; 3] }), 0.02, 600.0, false, false);
        assert_ne!(lift.target, before.target);
        assert_eq!(lift.distance, before.distance);

        let mut tilt = before;
        navigate(&mut tilt, raw(Axes { translation: [0; 3], rotation: [350, 0, 0] }), 0.02, 600.0, false, false);
        assert_eq!((tilt.yaw, tilt.roll), (before.yaw, before.roll));
        assert_ne!(tilt.pitch, before.pitch);

        let mut roll = before;
        navigate(&mut roll, raw(Axes { translation: [0; 3], rotation: [0, 350, 0] }), 0.02, 600.0, false, false);
        assert_eq!((roll.pitch, roll.roll), (before.pitch, before.roll));
        assert_ne!(roll.yaw, before.yaw);

        let mut twist = before;
        navigate(&mut twist, raw(Axes { translation: [0; 3], rotation: [0, 0, 350] }), 0.02, 600.0, false, false);
        assert_eq!((twist.yaw, twist.pitch), (before.yaw, before.pitch));
        assert_ne!(twist.roll, before.roll);
    }

    #[test]
    fn reverse_zoom_setting_applies_to_spacemouse() {
        let before = Camera::default();
        let motion = raw(Axes { translation: [0, 350, 0], rotation: [0; 3] });
        let mut normal = before;
        navigate(&mut normal, motion, 0.02, 600.0, false, false);
        let mut reversed = before;
        navigate(&mut reversed, motion, 0.02, 600.0, false, true);
        assert!(normal.distance < before.distance);
        assert!(reversed.distance > before.distance);
    }

    #[test]
    fn object_mode_tilts_roll_and_twists_spin() {
        let before = Camera::default();
        let mut tilt = before;
        navigate(&mut tilt, DriverMapping::object_mode().apply(Axes { translation: [0; 3], rotation: [0, 350, 0] }), 0.02, 600.0, false, false);
        assert_eq!((tilt.yaw, tilt.pitch), (before.yaw, before.pitch));
        assert_ne!(tilt.roll, before.roll);
        let mut twist = before;
        navigate(&mut twist, DriverMapping::object_mode().apply(Axes { translation: [0; 3], rotation: [0, 0, 350] }), 0.02, 600.0, false, false);
        assert_ne!(twist.yaw, before.yaw);
        assert_eq!((twist.pitch, twist.roll), (before.pitch, before.roll));
        // The physical tilt right (Ry−) rolls one way and the clockwise twist (Rz+) spins the
        // other, as 3DxWare's profile has them (#1172).
        let tilt_right = DriverMapping::object_mode().apply(Axes { translation: [0; 3], rotation: [0, -350, 0] });
        assert_eq!(tilt_right, Motion { translation: [0.0; 3], rotation: [0.0, 0.0, 1.0] });
        let twist_clockwise = DriverMapping::object_mode().apply(Axes { translation: [0; 3], rotation: [0, 0, 350] });
        assert_eq!(twist_clockwise, Motion { translation: [0.0; 3], rotation: [0.0, -1.0, 0.0] });
        // Translations and the forward tilt are the raw axes.
        let all = Axes { translation: [100, -200, 300], rotation: [-150, 0, 0] };
        assert_eq!(DriverMapping::object_mode().apply(all), DriverMapping::default().apply(all));
    }

    #[test]
    fn the_dead_zone_is_rest() {
        assert!(!Axes::default().moving());
        assert!(!Axes { translation: [20, -20, 0], rotation: [0, 20, -20] }.moving());
        for i in 0..6 {
            let mut values = [0_i16; 6];
            values[i] = 21;
            let axes = Axes { translation: [values[0], values[1], values[2]], rotation: [values[3], values[4], values[5]] };
            assert!(axes.moving(), "axis {i}");
            values[i] = -21;
            let axes = Axes { translation: [values[0], values[1], values[2]], rotation: [values[3], values[4], values[5]] };
            assert!(axes.moving(), "axis {i} reversed");
        }
        assert!(Axes { translation: [i16::MIN, 0, 0], rotation: [0, 0, i16::MAX] }.moving());
    }

    #[test]
    fn a_mapping_never_reaches_past_the_six_axes() {
        let mut mapping = DriverMapping::default();
        mapping.axes[0].output = 6;
        mapping.axes[1].output = usize::MAX;
        let motion = mapping.apply(Axes { translation: [350, 350, 350], rotation: [350; 3] });
        assert_eq!(motion.translation, [0.0, 0.0, 1.0]);
        assert_eq!(motion.rotation, [1.0; 3]);
    }

    #[test]
    fn a_frame_moves_the_view_and_keeps_the_next_frame_coming() {
        let ctx = egui::Context::default();
        let mut app = app();
        let before = app.cam;
        assert!(frame(&mut app, &ctx, DriverMapping::default(), PUSH, 0.02));
        assert!(app.cam.distance < before.distance);
        // At rest, or with no time passed, nothing moves.
        let still = app.cam;
        assert!(!frame(&mut app, &ctx, DriverMapping::default(), Axes::default(), 0.02));
        assert!(frame(&mut app, &ctx, DriverMapping::default(), PUSH, 0.0));
        assert_eq!(app.cam.distance, still.distance);
    }

    #[test]
    fn an_open_file_picker_holds_the_view_until_it_closes() {
        let ctx = egui::Context::default();
        let mut app = app();
        let slot: crate::picker_test_support::Slot = Default::default();
        crate::picker_test_support::open(&mut app, &slot);
        app.pick_open(|_, _| {});
        assert!(app.file_picker_busy());
        let before = app.cam;
        assert!(!frame(&mut app, &ctx, DriverMapping::default(), PUSH, 0.02));
        assert_eq!(app.cam.distance, before.distance);
        // Cancelled: the 3D mouse moves the view again.
        *slot.borrow_mut() = Some(None);
        crate::file_picker::tick(&mut app, &ctx);
        assert!(!app.file_picker_busy());
        assert!(frame(&mut app, &ctx, DriverMapping::default(), PUSH, 0.02));
        assert!(app.cam.distance < before.distance);
    }

    #[test]
    fn a_long_gap_between_frames_moves_as_far_as_a_short_one() {
        let ctx = egui::Context::default();
        let (mut short, mut long) = (app(), app());
        frame(&mut short, &ctx, DriverMapping::default(), PUSH, 0.05);
        frame(&mut long, &ctx, DriverMapping::default(), PUSH, 30.0);
        assert_eq!(short.cam.distance, long.cam.distance);
        for seconds in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let mut app = app();
            let before = app.cam;
            frame(&mut app, &ctx, DriverMapping::default(), PUSH, seconds);
            assert_eq!(app.cam.distance, before.distance, "{seconds}");
        }
    }

    #[test]
    fn navigation_off_the_start_page_and_a_hostile_speed_leave_the_view_alone() {
        let ctx = egui::Context::default();
        let mut off = app();
        off.preferences.space_mouse_enabled = false;
        let mut home = app();
        home.home.open = true;
        let mut nan = app();
        nan.preferences.space_mouse_speed = f32::NAN;
        for mut app in [off, home, nan] {
            let before = app.cam;
            assert!(!frame(&mut app, &ctx, DriverMapping::default(), PUSH, 0.02));
            assert_eq!(app.cam.distance, before.distance);
        }
        // Speed follows the preference, within 0.1 to 5 times.
        let speeds = [0.5, 2.0, 5.0, f32::MAX, 0.25, 0.0, -3.0];
        let distances: Vec<f64> = speeds
            .iter()
            .map(|&speed| {
                let mut app = app();
                app.preferences.space_mouse_speed = speed;
                frame(&mut app, &ctx, DriverMapping::default(), PUSH, 0.02);
                assert!(app.cam.is_valid(), "{speed}");
                app.cam.distance
            })
            .collect();
        assert!(distances[1] < distances[0]);
        assert_eq!(distances[3], distances[2], "past 5 times is 5 times");
        // Under 0.1 times is 0.1 times (the f32 preference can't hold 0.1 exactly): still a
        // motion, less than at 0.25.
        assert_eq!(distances[6], distances[5], "under 0.1 times is 0.1 times");
        assert!(distances[4] < distances[5] && distances[5] < app().cam.distance);
    }

    #[test]
    fn wheel_turns_are_dropped_while_the_cap_moves_and_kept_at_rest() {
        let mut guard = WheelGuard::default();
        // At rest, the wheel is the user's.
        let mut events = vec![wheel(100.0), other()];
        guard.filter(false, 10.0, &mut events);
        assert_eq!(events.len(), 2);
        // Moving: 3DxWare's wheel (and a pinch zoom) goes; everything else stays.
        let mut events = vec![wheel(-100.0), other(), egui::Event::Zoom(1.1), wheel(100.0)];
        guard.filter(true, 11.0, &mut events);
        assert_eq!(events, vec![other()]);
        // Just after the cap stops, a queued one still goes.
        let mut events = vec![wheel(100.0)];
        guard.filter(false, 11.0 + WHEEL_QUIET_S - 0.01, &mut events);
        assert!(events.is_empty());
        // Later, the wheel is the user's again.
        let mut events = vec![wheel(100.0)];
        guard.filter(false, 11.0 + WHEEL_QUIET_S + 0.01, &mut events);
        assert_eq!(events.len(), 1);
        // A clock that goes back never swallows the user's wheel.
        let mut events = vec![wheel(100.0)];
        guard.filter(false, 5.0, &mut events);
        assert_eq!(events.len(), 1);
        let mut events = vec![wheel(100.0)];
        guard.filter(false, f64::NAN, &mut events);
        assert_eq!(events.len(), 1);
    }
}
