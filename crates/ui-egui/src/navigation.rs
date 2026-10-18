//! Camera navigation shared by every page that shows the model through a camera: the modelling
//! viewport and the Render preview today, and any future canvas (a 2D drawing or flat pattern
//! page calls it with orbit off, so pan and zoom still follow the user's bindings).
//!
//! One entry point, [`handle`], reads a frame's pointer, wheel, trackpad and modifier input over a
//! view and moves `app.cam` the way Preferences › Navigation says: the navigation preset (Fusion,
//! SolidWorks, Blender, Touchpad…), Orbit around, zoom to the cursor, the middle double-click and
//! orbit snapping. The 3D mouse's motion goes through [`device`] too, with the page's
//! [`NavCaps`]. The view keys and the navigators act on `app.cam` independently of the page. New
//! canvases must call `navigation::handle` rather than reading drags or the wheel themselves;
//! `hand_rolled_camera_input_is_rejected` fails when they do.

use egui::{Pos2, Rect, pos2, vec2};
use serde_json::json;
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::render::Camera;

use crate::SolveApp;
use crate::prefs::{OrbitAround, pivot_under};
use crate::space_mouse::Motion;
use crate::viewport::{Hit, NavMode, axis_rect, nearest_quarter_turn, pick, pick_cached, projection};

/// What a page's camera may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NavCaps {
    /// Orbit (and continuous orbit) is allowed; off for flat 2D pages, where pan and zoom remain.
    pub orbit: bool,
    /// What is under the pointer can be picked or dragged, so presses on it don't navigate the
    /// way presses on empty space do (OpenSCAD's left drag, Ctrl+left orbit).
    pub pick: bool,
}

impl NavCaps {
    /// The modelling viewport.
    pub const MODEL: NavCaps = NavCaps { orbit: true, pick: true };
    /// The Render workspace's product preview: the same navigation, nothing to select.
    pub const RENDER: NavCaps = NavCaps { orbit: true, pick: false };
}

/// Raw wheel/trackpad input for one frame, split by source.
///
/// Mouse wheels report lines or pages; trackpads report points. Trackpad
/// scrolls arrive bracketed (`Start` … `Move` … `End`, momentum included)
/// while mouse wheels send bare `Move` events, so phases tell them apart.
pub(crate) struct RawWheel {
    /// Vertical mouse-wheel input in points (zooms).
    zoom: f32,
    /// Trackpad point input, routed like the wheel (zooms when panning is off).
    point_zoom: f32,
    /// Bracketed two-finger scroll in points (pans when panning is on).
    pan: egui::Vec2,
    /// Bare horizontal input in points (thumb wheel: always pans sideways).
    mouse_pan: egui::Vec2,
}

/// Route wheel points to the vertical axis, matching egui's scroll modifiers.
pub(crate) fn route_wheel_vertical(points: egui::Vec2, modifiers: egui::Modifiers, options: egui::InputOptions) -> f32 {
    let horizontal = modifiers.matches_any(options.horizontal_scroll_modifier);
    let vertical = modifiers.matches_any(options.vertical_scroll_modifier);
    if horizontal && !vertical {
        0.0
    } else if vertical && !horizontal {
        points.x + points.y
    } else {
        points.y
    }
}

/// Match egui's wheel units and modifier routing, without its temporal smoothing.
///
/// `gesture` tracks an ongoing bracketed trackpad scroll (`Start` … `End`):
/// moves inside one pan, bare `Move` input is a mouse (vertical zooms,
/// horizontal pans). Only point input brackets; line/page input always zooms.
pub(crate) fn immediate_wheel(input: &egui::InputState, options: egui::InputOptions, page_height: f32, gesture: &mut bool) -> RawWheel {
    let mut out = RawWheel { zoom: 0.0, point_zoom: 0.0, pan: egui::Vec2::ZERO, mouse_pan: egui::Vec2::ZERO };
    for event in &input.events {
        let egui::Event::MouseWheel { unit, delta, modifiers, phase } = event else { continue };
        if modifiers.matches_any(options.zoom_modifier) {
            continue;
        }
        let point = matches!(unit, egui::MouseWheelUnit::Point);
        match phase {
            egui::TouchPhase::Start => {
                if point {
                    *gesture = true;
                }
                continue;
            }
            egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                if point {
                    *gesture = false;
                }
                continue;
            }
            egui::TouchPhase::Move => {}
        }
        let points = match unit {
            egui::MouseWheelUnit::Point => *delta,
            egui::MouseWheelUnit::Line => *delta * options.line_scroll_speed,
            egui::MouseWheelUnit::Page => *delta * page_height,
        };
        if point && *gesture {
            out.pan += points;
            // Legacy path only (ignored while panning is on).
            out.point_zoom += route_wheel_vertical(points, *modifiers, options);
        } else if point {
            // Bare moves are a mouse: vertical zooms, horizontal pans sideways.
            // A wheel needs the opposite sign of fingers for the same motion.
            out.zoom += route_wheel_vertical(points, *modifiers, options);
            out.mouse_pan += egui::vec2(-points.x, 0.0);
        } else {
            out.zoom += route_wheel_vertical(points, *modifiers, options);
        }
    }
    out
}

/// Combined pinch-to-zoom factor for one frame (1 = no pinch input).
pub(crate) fn pinch_factor(input: &egui::InputState) -> f32 {
    input
        .events
        .iter()
        .filter_map(|event| {
            let egui::Event::Zoom(f) = event else { return None };
            (*f).is_finite().then_some(*f).filter(|f| *f > 0.0)
        })
        .fold(1.0, |acc, f| acc * f)
}

/// Zoom anchor: the model point under the cursor, else the target's depth.
/// Preferences › Navigation › Zoom to the cursor (off: toward the view's centre).
pub(crate) fn zoom_anchor(app: &mut SolveApp, rect: egui::Rect, hover: Option<egui::Pos2>, surface: bool) -> Option<Vec3> {
    hover.filter(|_| app.preferences.zoom_to_cursor).map(|p| {
        let proj = projection(app, rect);
        // Toward the model's surface under the cursor, else the target's depth.
        if surface && let Some(q) = pick_cached(app, &proj, p).iter().find_map(|h| if let Hit::Face { point, .. } = h { Some(*point) } else { None })
        {
            return q;
        }
        let (o, d) = proj.ray(p);
        let n = app.cam.back();
        let den = d.dot(n);
        if den.abs() > 1e-9 { o + d * ((app.cam.target - o).dot(n) / den) } else { app.cam.target }
    })
}

/// Modifier-only navigation never borrows pointer movement from another widget or gesture.
pub(crate) fn modifier_motion(
    app: &mut SolveApp,
    ui: &egui::Ui,
    inside: bool,
    hover: Option<Pos2>,
    mods: egui::Modifiers,
    picking: bool,
) -> Option<(NavMode, egui::Vec2)> {
    let owns = inside
        && ui.input(|i| i.focused && !i.pointer.any_down() && !i.pointer.any_released() && !i.pointer.any_pressed())
        && !ui.ctx().egui_wants_keyboard_input()
        && !egui::Popup::is_any_open(ui.ctx())
        && !crate::dim_view::editing()
        && app.viewport.context_menu.is_none()
        && app.viewport.axis_drag.is_none()
        && app.viewport.point_drag.is_none()
        && app.viewport.text_drag.is_none()
        && app.viewport.boxsel.is_none()
        && !(picking && hover.is_some_and(|p| app.viewport.rect.is_some_and(|rect| crate::canvas::owns_pointer(app, &projection(app, rect), p))))
        && !hover.is_some_and(|p| app.viewport.layout.as_ref().is_some_and(|l| l.navigation.contains(p)));
    let next = crate::prefs::nav_preset(&app.preferences.nav).motion_mode(mods).zip(hover.filter(|p| owns && p.is_finite()));
    let previous = app.viewport.modifier_nav;
    if previous.map(|(m, _)| m) != next.map(|(m, _)| m) {
        app.viewport.orbit_pivot = None;
        app.viewport.orbit_free = None;
        app.viewport.orbit_snap = None;
    }
    app.viewport.modifier_nav = next;
    next.map(|(mode, p)| {
        let delta = previous.filter(|(m, _)| *m == mode).map_or(egui::Vec2::ZERO, |(_, last)| p - last);
        let delta = if delta.is_finite() && delta.x.abs() <= 1e4 && delta.y.abs() <= 1e4 { delta } else { egui::Vec2::ZERO };
        (mode, delta)
    })
}

/// View keys (#1541): the arrows turn the view 15° (with Shift a quarter turn, as the navigation
/// cube's arrows), Ctrl+arrows pan it a tenth of the view, and Ctrl+8 looks straight at the
/// selected face, plane or sketch. Not while a menu, the command box or a focused widget has the
/// arrows. Returns whether the key was used.
pub(crate) fn view_key(app: &mut SolveApp, ctx: &egui::Context, key: egui::Key, mods: egui::Modifiers) -> bool {
    use egui::Key;
    let menu_open = egui::Popup::is_any_open(ctx)
        || app.viewport.context_menu.is_some()
        || app.ui.palette_open
        || ctx.memory(|m| m.focused().is_some())
        || ctx.data(|d| d.get_temp::<egui::Id>(egui::Id::new("sc_panel_open")).is_some());
    let ctrl = mods.command || mods.ctrl;
    if mods.alt || menu_open {
        return false;
    }
    if key == Key::Num8 && ctrl && !mods.shift {
        crate::viewport::look_at_selection(app);
        return true;
    }
    let (action, dx, dy) = match key {
        Key::ArrowLeft => ("left", -1.0, 0.0),
        Key::ArrowRight => ("right", 1.0, 0.0),
        Key::ArrowUp => ("up", 0.0, -1.0),
        Key::ArrowDown => ("down", 0.0, 1.0),
        _ => return false,
    };
    if ctrl {
        // The model moves with the arrow, as it does under a pan drag.
        if let Some(rect) = app.viewport.rect {
            let step = rect.height() * 0.1;
            camera_motion(app, rect, json!({"motion": "pan", "delta": [dx * step, dy * step]}));
        }
    } else {
        let degrees = if mods.shift { 90.0 } else { 15.0 };
        let _ = app.run("view.navigate", json!({"camera": app.cam, "action": action, "degrees": degrees}));
    }
    true
}

/// Continuous camera changes use the same JSON command as programmatic callers.
pub(crate) fn camera_motion(app: &mut SolveApp, rect: Rect, mut params: serde_json::Value) {
    let stored_fov = app.cam.fov;
    params["camera"] = json!(projection(app, rect).cam);
    params["viewport"] = json!([rect.width().max(1.0), rect.height().max(1.0)]);
    let _ = app.run("view.navigate", params);
    app.cam.fov = stored_fov;
}

/// What one frame of camera input saw and did, for the caller's hover, selection and drawing.
pub(crate) struct CameraInput {
    pub hover: Option<Pos2>,
    pub scroll: f32,
    pub mods: egui::Modifiers,
    pub middle: bool,
    pub secondary_down: bool,
    pub primary_down: bool,
    pub primary_pressed: bool,
    pub delta: egui::Vec2,
    /// The pointer is over this view (not a window, menu or the axis view above it).
    pub inside: bool,
    /// The axis view or navigation cube's rectangle.
    pub axes: Rect,
    pub pinch: f32,
    pub pan: egui::Vec2,
    /// The navigation bar's fixed mode, or the preset's left-button binding.
    pub primary_nav: Option<NavMode>,
    /// The navigation this frame's pointer input drives.
    pub mode: Option<NavMode>,
}

/// Camera input for one frame over `rect`: the navigation preset's buttons and modifiers, Orbit
/// around, wheel, trackpad and pinch zoom (to the cursor), middle double-click, continuous orbit
/// and orbit snapping. Every camera page calls this, so the mouse moves the camera the same way
/// in every workspace (#1454). With `caps.pick` off nothing under the pointer is treated as
/// selectable, so a press that would start a selection navigates as on empty space; with
/// `caps.orbit` off, bindings that orbit do nothing and pan and zoom work as usual.
pub(crate) fn handle(app: &mut SolveApp, ui: &mut egui::Ui, rect: Rect, resp: &egui::Response, caps: NavCaps) -> CameraInput {
    let picking = caps.pick;
    let (hover, scroll, mods, middle, secondary_down, primary_down, primary_pressed, primary_released, delta) = ui.input(|i| {
        (
            i.pointer.hover_pos(),
            i.smooth_scroll_delta.y,
            i.modifiers,
            i.pointer.middle_down(),
            i.pointer.secondary_down(),
            i.pointer.primary_down(),
            i.pointer.primary_pressed(),
            i.pointer.primary_released(),
            i.pointer.delta(),
        )
    });
    // The right button belongs to the marking menu while it is open.
    let secondary_down = secondary_down && app.viewport.context_menu.is_none();
    // The axis view handles its own clicks; the model underneath must not see them.
    let axes = axis_rect(app, rect);
    // Floating windows and menus own input over the model, including wheel navigation. egui's
    // hit test also counts the model as under the pointer a few points past its own edge and
    // inside a window's or a list's edge (#678), so the pointer must be in the viewport too,
    // with no window or menu above it.
    let inside = resp.contains_pointer() && ui.rect_contains_pointer(rect) && hover.is_some_and(|p| !axes.contains(p));
    app.viewport.mouse = hover.filter(|_| inside);
    // Ctrl or Alt held: drawing and dragging don't snap.
    crate::drag_snap::set_suppressed(mods.ctrl || mods.alt || mods.command);
    middle_double_click(app, ui, inside);
    // The 3D mouse's motion this frame, with what this page's camera may do.
    device(app, rect, caps);

    // ---- navigation ----
    let trackpad_pan = app.preferences.trackpad_pans();
    if !app.preferences.smooth_wheel_zoom {
        app.viewport.wheel_zoom_raw_until_idle = true;
    } else if !ui.input(|i| i.is_scrolling()) {
        app.viewport.wheel_zoom_raw_until_idle = false;
    }
    let options = ui.ctx().options(|o| o.input_options);
    let page_height = ui.ctx().viewport_rect().height();
    let raw = ui.input(|i| immediate_wheel(i, options, page_height, &mut app.viewport.trackpad_gesture));
    if trackpad_pan && raw.pan != egui::Vec2::ZERO {
        // Swallow egui's smoothing tail while the trackpad runs (a wheel-only
        // tail ends the moment trackpad input starts; mixed input is rare).
        app.viewport.trackpad_until_idle = true;
    } else if !ui.input(|i| i.is_scrolling()) {
        app.viewport.trackpad_until_idle = false;
        // Self-healing: with no scroll activity no gesture can be mid-flight
        // (its End was lost); a stuck flag would pan future mouse input.
        app.viewport.trackpad_gesture = false;
    }
    let mut pan = if trackpad_pan { raw.pan } else { egui::Vec2::ZERO } + raw.mouse_pan;
    pan.x *= crate::prefs::pan_sign_x(app);
    let scroll = if app.viewport.wheel_zoom_raw_until_idle || (trackpad_pan && app.viewport.trackpad_until_idle) {
        raw.zoom + if trackpad_pan { 0.0 } else { raw.point_zoom }
    } else {
        scroll
    };
    // Pinch-to-zoom (trackpad): direct manipulation, never smoothed.
    let pinch = ui.input(pinch_factor);
    let h = rect.height().max(1.0) as f64;
    if inside && pinch != 1.0 {
        app.viewport.orbit_snap = None;
        app.viewport.orbit_free = None;
        // Spread zooms in: the camera distance shrinks by the pinch factor.
        let f = f64::from(pinch).powf(-crate::prefs::zoom_sign(app));
        let anchor = zoom_anchor(app, rect, hover, caps.orbit);
        if app.preferences.nav == crate::prefs::TOUCHPAD_NAV {
            let mut params = json!({"motion":"zoom", "factor":f.clamp(0.05,20.0)});
            if let Some(anchor) = anchor {
                params["anchor"] = json!(anchor);
            }
            camera_motion(app, rect, params);
        } else {
            app.cancel_view_animation();
            app.cam.zoom_at(f, anchor);
        }
    }
    if inside && scroll != 0.0 {
        app.viewport.orbit_snap = None;
        app.viewport.orbit_free = None;
        let f = (-scroll as f64 * 0.0015 * crate::prefs::zoom_sign(app)).exp();
        let anchor = zoom_anchor(app, rect, hover, caps.orbit);
        if app.preferences.nav == crate::prefs::TOUCHPAD_NAV {
            let mut params = json!({"motion":"zoom", "factor":f.clamp(0.05,20.0)});
            if let Some(anchor) = anchor {
                params["anchor"] = json!(anchor);
            }
            camera_motion(app, rect, params);
        } else {
            app.cancel_view_animation();
            app.cam.zoom_at(f, anchor);
        }
    }
    // Two-finger scroll pans the model with the fingers.
    if inside && pan != egui::Vec2::ZERO {
        app.viewport.orbit_snap = None;
        app.viewport.orbit_free = None;
        if app.preferences.nav == crate::prefs::TOUCHPAD_NAV {
            camera_motion(app, rect, json!({"motion":"pan","delta":[pan.x,pan.y]}));
        } else {
            app.cancel_view_animation();
            app.cam.pan(f64::from(pan.x), f64::from(pan.y), h);
        }
    }
    // A left press with the preset's keys (Alias: Shift+Alt) navigates until the button is let
    // go (kept through the release frame, so the end of the drag doesn't select either).
    if primary_pressed && inside {
        let empty_space = |app: &SolveApp| {
            app.viewport.context_menu.is_none()
                && hover.is_some_and(|p| {
                    if !picking {
                        return true;
                    }
                    let proj = projection(app, rect);
                    pick(app, &proj, p).is_empty() && crate::dim_view::hit(p).is_none() && crate::sketch_tools::glyph_at(p).is_none()
                })
        };
        let openscad_orbit =
            app.preferences.nav == crate::prefs::OPENSCAD_NAV && app.tool.is_none() && app.dialog.is_none() && !mods.shift && empty_space(app);
        let preset = if app.preferences.nav != crate::prefs::OPENSCAD_NAV || openscad_orbit {
            crate::prefs::nav_mode(app, crate::prefs::NavButton::Left, mods)
        } else {
            None
        };
        // Ctrl+left drag from empty space orbits in every preset (Cmd on macOS). A Ctrl+click
        // still selects, and Ctrl on geometry still drags it without snapping.
        let ctrl_orbit = (mods.ctrl || mods.command) && !mods.shift && !mods.alt && empty_space(app);
        app.viewport.primary_nav = preset.or(ctrl_orbit.then_some(NavMode::Orbit));
    } else if !primary_down && !primary_released {
        app.viewport.primary_nav = None;
    }
    // Hold Space for a momentary pan: the hand tool of design apps. It overrides whatever the
    // left button would otherwise do - selecting, dragging geometry, the preset's binding or the
    // navigation bar's fixed mode - and lasts only while the key is held. Space still types in a
    // text field, and while a dimension's text is being edited, so those keep the keyboard.
    let space_pan = (inside || primary_down)
        && !ui.ctx().egui_wants_keyboard_input()
        && !crate::dim_view::editing()
        && ui.input(|i| i.key_down(egui::Key::Space));
    if space_pan {
        ui.ctx().set_cursor_icon(if primary_down { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    }
    // The fixed navigation mode (navigation bar), a held Space, or the preset's left-button binding.
    let primary_nav = if space_pan { Some(NavMode::Pan) } else { app.viewport.nav.or(app.viewport.primary_nav) };
    let button = if middle {
        Some(crate::prefs::NavButton::Middle)
    } else if secondary_down {
        Some(crate::prefs::NavButton::Right)
    } else {
        None
    };
    let dragging = resp.dragged() || (inside && button.is_some());
    // Without orbit (a flat page), bindings that orbit do nothing.
    let allowed = |m: NavMode| caps.orbit || m != NavMode::Orbit;
    let motion = modifier_motion(app, ui, inside, hover, mods, picking).filter(|(m, _)| allowed(*m));
    let mode = motion
        .map(|(m, _)| m)
        .or_else(|| {
            if dragging && app.viewport.axis_drag.is_none() {
                match button {
                    Some(b) => crate::prefs::nav_mode(app, b, mods),
                    None if primary_down => primary_nav,
                    None => None,
                }
            } else {
                None
            }
        })
        .filter(|m| allowed(*m));
    if let Some((mode, delta)) = motion {
        if delta != egui::Vec2::ZERO {
            let mut params = json!({"motion":if mode == NavMode::Pan {"pan"} else {"orbit"},"delta":[delta.x,delta.y]});
            if mode == NavMode::Orbit {
                let pivot = match app.preferences.orbit_around {
                    crate::prefs::OrbitAround::ViewCentre => None,
                    crate::prefs::OrbitAround::Origin => Some(Vec3::ZERO),
                    crate::prefs::OrbitAround::Cursor => Some(match app.viewport.orbit_pivot {
                        Some(pivot) => pivot,
                        None => {
                            let pivot = crate::prefs::pivot_under(app, &projection(app, rect), hover.unwrap_or(rect.center()) - delta);
                            app.viewport.orbit_pivot = Some(pivot);
                            pivot
                        }
                    }),
                };
                if let Some(pivot) = pivot {
                    params["pivot"] = json!(pivot);
                }
            }
            camera_motion(app, rect, params);
        }
    } else if mode == Some(NavMode::Orbit) {
        let press = ui.input(|i| i.pointer.press_origin());
        app.cancel_view_animation();
        // Shift can be required to enter orbit (Fusion/Inventor middle drag, Alias Shift+Alt),
        // rather than an additional request for snapping. Only an extra modifier snaps.
        let without_shift = egui::Modifiers { shift: false, ..mods };
        let snap = app.preferences.nav != crate::prefs::OPENSCAD_NAV
            && mods.shift
            && match button {
                Some(b) => crate::prefs::nav_mode(app, b, without_shift) == Some(NavMode::Orbit),
                None => app.viewport.nav.is_some(),
            };
        orbit_drag(app, rect, f64::from(delta.x), f64::from(delta.y), press, snap, true);
    } else {
        if matches!(mode, Some(NavMode::Pan | NavMode::Zoom)) {
            app.viewport.orbit_snap = None;
        }
        if app.viewport.axis_drag.is_none() {
            app.viewport.orbit_free = None;
        }
        if delta != egui::Vec2::ZERO {
            if mode.is_some() {
                app.cancel_view_animation();
            }
            match mode {
                Some(NavMode::Pan) => app.cam.pan(f64::from(delta.x), f64::from(delta.y), h),
                Some(NavMode::Zoom) => app.cam.zoom_at((f64::from(delta.y) * 0.01).exp(), None),
                _ => {}
            }
        }
    }
    if !dragging && motion.is_none() {
        app.viewport.orbit_pivot = None;
    }
    if app.viewport.spin {
        // A click on the navigation bar (its own button) is not a stop.
        let bar = app
            .viewport
            .layout
            .as_ref()
            .map(|l| l.navigation)
            .unwrap_or_else(|| Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 22.0), vec2(320.0, 30.0)));
        let (pressed, key, dt) = ui.input(|i| {
            (
                i.pointer.any_pressed() && inside && !i.pointer.press_origin().is_some_and(|p| bar.contains(p)),
                i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. })),
                i.stable_dt,
            )
        });
        if pressed || key || dragging || motion.is_some() || !caps.orbit {
            app.viewport.spin = false;
        } else {
            app.cam.orbit(f64::from(dt.min(0.1)) * 30.0, 0.0);
            ui.ctx().request_repaint();
        }
    }
    if !app.cam.is_valid() {
        app.viewport.orbit_free = None;
        app.viewport.orbit_snap = None;
        app.cam = Camera::default();
        app.fit_view();
    }

    step_orbit_snap(app, rect, ui.input(|i| f64::from(i.stable_dt)));
    if app.viewport.orbit_snap.is_some() {
        ui.ctx().request_repaint();
    }
    CameraInput { hover, scroll, mods, middle, secondary_down, primary_down, primary_pressed, delta, inside, axes, pinch, pan, primary_nav, mode }
}

/// The 3D mouse's motion this frame (#1336), on a page `rect` whose camera may do `caps`: the cap
/// pans and zooms every page, and turns the view only where the page may orbit and no sketch is
/// being edited (a sketch keeps its view square to it). [`handle`] calls it for its page; the 3D
/// mouse only reads the device and queues the motion ([`crate::space_mouse::navigate_frame`]).
pub(crate) fn device(app: &mut SolveApp, rect: Rect, caps: NavCaps) {
    let Some(queued) = app.viewport.device_motion.take() else { return };
    let turn = caps.orbit && app.session.active_sketch.is_none();
    let height = f64::from(rect.height().max(1.0));
    device_camera(&mut app.cam, queued.motion, queued.seconds, height, turn, app.preferences.zoom_reverse);
}

/// `seconds` of 3D mouse `motion` on `cam`, in a view `height` points tall. A full deflection
/// pans about 0.6 of the view's height a second and turns about two radians a second, whatever
/// the frame rate. With `turn` off the view keeps its orientation.
pub(crate) fn device_camera(cam: &mut Camera, motion: Motion, seconds: f64, height: f64, turn: bool, reverse_zoom: bool) {
    if !(seconds > 0.0) || !seconds.is_finite() {
        return;
    }
    let [x, y, z] = motion.translation;
    let [rx, ry, rz] = motion.rotation;
    // Logical 3DxWare coordinates are X = left/right, Y = zoom and Z = up/down. The user's
    // profile can swap the physical forward/back and up/down axes before they reach here.
    cam.pan(x * height * 0.6 * seconds, z * height * 0.6 * seconds, height);
    let zoom_sign = if reverse_zoom { -1.0 } else { 1.0 };
    cam.zoom_at((-y * zoom_sign * 1.5 * seconds).exp(), None);
    if turn {
        // These are logical 3DxWare coordinates after the profile mapping: Rx is Tilt, Ry is
        // Roll and Rz is Spin. The KMJ profile swaps physical Ry/Rz for the selected navigation
        // mode, so swapping them again here would make Roll behave like Spin.
        cam.orbit(ry * 250.0 * seconds, rx * 250.0 * seconds);
        cam.roll_by(rz * 2.0 * seconds);
    }
}

/// Six principal views, twelve edge diagonals and eight isometric corner directions.
pub(crate) fn snap_directions() -> impl Iterator<Item = Vec3> {
    [-1.0, 0.0, 1.0].into_iter().flat_map(|x| {
        [-1.0, 0.0, 1.0].into_iter().flat_map(move |y| [-1.0, 0.0, 1.0].into_iter().filter_map(move |z| Vec3::new(x, y, z).normalized()))
    })
}

/// Snap to the nearest standard or diagonal direction in the user's up-axis frame.
pub(crate) fn snapped_axis_camera(cam: Camera) -> Camera {
    let b = cam.to_frame(cam.back());
    let dir = snap_directions().max_by(|a, c| a.dot(b).total_cmp(&c.dot(b))).unwrap_or(Vec3::Z);
    let mut to = cam.looking_from(cam.from_frame(dir));
    if dir.x == 0.0 && dir.y == 0.0 {
        to.yaw = nearest_quarter_turn(cam.yaw);
    }
    to
}

pub(crate) struct OrbitSnap {
    pub(crate) target: Camera,
    pub(crate) yaw_velocity: f64,
    pub(crate) pitch_velocity: f64,
    pub(crate) pivot: Option<(Vec3, Pos2)>,
}

/// Pointer motion accumulates independently of the spring preview, so many small deltas
/// still cross snap boundaries. Keep a small angular margin to avoid boundary flicker.
pub(crate) fn orbit_drag(app: &mut SolveApp, rect: Rect, dx: f64, dy: f64, press: Option<Pos2>, snap: bool, around_cursor: bool) {
    let displayed = app.cam;
    if let Some(free) = app.viewport.orbit_free.take() {
        app.cam = free;
    }
    if around_cursor {
        orbit(app, rect, dx, dy, press);
    } else {
        app.cam.orbit(dx, dy);
    }
    let free = app.cam;
    app.viewport.orbit_free = Some(free);
    if snap || !around_cursor || app.viewport.orbit_snap.is_some() {
        let mut target = if snap { snapped_axis_camera(free) } else { free };
        if snap && let Some(previous) = app.viewport.orbit_snap.as_ref() {
            let b = free.back();
            if previous.target.back().dot(b) + 0.025 >= target.back().dot(b) {
                target.yaw = previous.target.yaw;
                target.pitch = previous.target.pitch;
            }
        }
        // The cursor and origin pivots stay still while a snap eases in; the axis gizmo has no
        // pivot (it orbits about the view centre, see `axis_view`).
        let pivot = if around_cursor {
            match app.preferences.orbit_around {
                crate::prefs::OrbitAround::Origin => Some(Vec3::ZERO),
                _ => app.viewport.orbit_pivot,
            }
        } else {
            None
        }
        .and_then(|p| {
            app.cam = displayed;
            projection(app, rect).to_screen(p).map(|at| (p, at))
        });
        let spring = app.viewport.orbit_snap.get_or_insert(OrbitSnap { target, yaw_velocity: 0.0, pitch_velocity: 0.0, pivot });
        spring.target = target;
        spring.pivot = pivot;
        app.cam = displayed;
    }
}

/// A lightly damped spring: mostly settled in about a third of a second, with a small
/// elastic finish. Bounded substeps keep the behavior consistent across frame rates.
pub(crate) fn step_orbit_snap(app: &mut SolveApp, rect: Rect, dt: f64) {
    let Some(mut spring) = app.viewport.orbit_snap.take() else { return };
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.05) } else { 1.0 / 60.0 };
    let steps = (dt * 240.0).ceil().max(1.0) as usize;
    let h = dt / steps as f64;
    let omega = 16.0;
    let damping = 0.82;
    let yaw_error = |yaw: f64| (spring.target.yaw - yaw + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
    for _ in 0..steps.min(12) {
        spring.yaw_velocity += (omega * omega * yaw_error(app.cam.yaw) - 2.0 * damping * omega * spring.yaw_velocity) * h;
        spring.pitch_velocity += (omega * omega * (spring.target.pitch - app.cam.pitch) - 2.0 * damping * omega * spring.pitch_velocity) * h;
        app.cam.yaw += spring.yaw_velocity * h;
        app.cam.pitch = (app.cam.pitch + spring.pitch_velocity * h).clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
    }
    app.cam.target = spring.target.target;
    let done = yaw_error(app.cam.yaw).abs() < 1e-5
        && (spring.target.pitch - app.cam.pitch).abs() < 1e-5
        && spring.yaw_velocity.abs() < 1e-4
        && spring.pitch_velocity.abs() < 1e-4;
    if done {
        app.cam = spring.target;
    }
    if let Some((pivot, at)) = spring.pivot
        && let Some(after) = projection(app, rect).to_screen(pivot)
    {
        app.cam.pan(f64::from(at.x - after.x), f64::from(at.y - after.y), f64::from(rect.height().max(1.0)));
    }
    if !done {
        app.viewport.orbit_snap = Some(spring);
    }
}

/// A middle-button double-click goes Home (or fits, or nothing: Preferences › Navigation). Only
/// clicks count: a press that moves (a pan or orbit drag) or is held is not one, and the two
/// clicks must both be the middle button's, close together in time and place.
pub(crate) fn middle_double_click(app: &mut SolveApp, ui: &egui::Ui, inside: bool) {
    use egui::PointerButton::{Extra1, Extra2, Middle, Primary, Secondary};
    let (double, releases, middle, other, dragging) = ui.input(|i| {
        (
            i.pointer.button_double_clicked(Middle),
            i.events.iter().filter(|e| matches!(e, egui::Event::PointerButton { button: Middle, pressed: false, .. })).count(),
            i.pointer.button_clicked(Middle),
            [Primary, Secondary, Extra1, Extra2].into_iter().any(|b| i.pointer.button_clicked(b)),
            i.pointer.is_decidedly_dragging(),
        )
    });
    // egui pairs clicks of any buttons; the first click must have been the middle one too (or
    // both arrive in this frame).
    let go = double && inside && !other && (app.viewport.middle_clicked || releases >= 2);
    app.viewport.middle_clicked = if other || dragging || go { false } else { middle || app.viewport.middle_clicked };
    if !go {
        return;
    }
    match crate::prefs::MiddleDoubleClick::of(app) {
        crate::prefs::MiddleDoubleClick::Home => app.animate_view("home"),
        crate::prefs::MiddleDoubleClick::Fit => app.animate_fit_all(),
        crate::prefs::MiddleDoubleClick::Off => {}
    }
}

/// Orbit by pixel deltas about the configured centre: the view's centre (default), the world
/// origin, or the model point that was under the cursor when the drag began. The origin and the
/// cursor pivot are kept still on screen; a cursor drag that begins over empty space turns around
/// the centre of the visible geometry instead, so the model never swings around a far-off point.
/// The cursor pivot is chosen once, when the drag begins.
pub fn orbit(app: &mut SolveApp, rect: egui::Rect, dx: f64, dy: f64, press: Option<Pos2>) {
    let pivot = match app.preferences.orbit_around {
        OrbitAround::ViewCentre => None,
        OrbitAround::Origin => Some(Vec3::ZERO),
        OrbitAround::Cursor => Some(match app.viewport.orbit_pivot {
            Some(p) => p,
            None => {
                let proj = crate::viewport::projection(app, rect);
                let at = press.unwrap_or(rect.center());
                let p = pivot_under(app, &proj, at);
                app.viewport.orbit_pivot = Some(p);
                p
            }
        }),
    };
    let Some(pivot) = pivot else {
        app.cam.orbit(dx, dy);
        return;
    };
    orbit_about(app, rect, dx, dy, pivot);
}

/// Orbit by pixel deltas, keeping `pivot` still on screen: the camera turns about it and a pan
/// correction puts it back where it was.
pub(crate) fn orbit_about(app: &mut SolveApp, rect: egui::Rect, dx: f64, dy: f64, pivot: Vec3) {
    let before = crate::viewport::projection(app, rect).to_screen(pivot);
    app.cam.orbit(dx, dy);
    let after = crate::viewport::projection(app, rect).to_screen(pivot);
    if let (Some(a), Some(b)) = (before, after) {
        app.cam.pan(f64::from(a.x - b.x), f64::from(a.y - b.y), f64::from(rect.height().max(1.0)));
    }
}

#[cfg(test)]
mod tests {
    /// Every page with a camera navigates through [`super::handle`] (#1454). A page that reads
    /// the wheel or moves the view camera itself would drift from the user's navigation settings,
    /// so outside this module the camera may only be moved through it, the view commands or the
    /// navigators. The 3D mouse in both apps hands its motion to [`super::device`].
    #[test]
    fn hand_rolled_camera_input_is_rejected() {
        const PATTERNS: [&str; 7] =
            ["smooth_scroll_delta", "raw_scroll_delta", "zoom_delta()", "cam.orbit(", "cam.pan(", "cam.zoom_at(", "cam.roll_by("];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dirs = [root.join("src"), root.join("../../apps/solidcraft/src"), root.join("../../apps/solidcraft-web/src")];
        let allowed = ["navigation.rs"];
        let mut offenders = Vec::new();
        let mut scanned = 0;
        let mut stack: Vec<std::path::PathBuf> = dirs.iter().filter(|d| d.is_dir()).cloned().collect();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !name.ends_with(".rs") || name.ends_with("_tests.rs") || allowed.contains(&name.as_str()) {
                    continue;
                }
                scanned += 1;
                // A Windows checkout's CRLF lines, read as written, so the split below finds the tests.
                let text = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
                // In-file unit tests set cameras up directly; only the code above them counts.
                let code = text.split("\n#[cfg(test)]\nmod ").next().unwrap_or_default();
                for (n, line) in code.lines().enumerate() {
                    if !line.trim_start().starts_with("//") && PATTERNS.iter().any(|p| line.contains(p)) {
                        offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                    }
                }
            }
        }
        assert!(scanned > 50, "the source tree was scanned ({scanned} files)");
        assert!(offenders.is_empty(), "camera input outside navigation::handle; call it instead:\n{}", offenders.join("\n"));
    }

    /// A flat page (orbit off) pans and zooms with the same bindings and never turns the view.
    #[test]
    fn flat_pages_pan_and_zoom_with_the_same_bindings_but_never_orbit() {
        use egui::{Event, Modifiers, PointerButton, pos2, vec2};
        let flat = super::NavCaps { orbit: false, pick: false };
        for preset in crate::prefs::NAV_PRESETS.iter().map(|p| p.id) {
            let mut s = solidcraft_engine::Session::default();
            s.execute("solid.box", &serde_json::json!({"length":40,"width":30,"height":20})).unwrap();
            let mut app = crate::SolveApp::new(s, crate::Services::default());
            app.preferences.nav = preset.into();
            app.viewport.spin = true;
            let ctx = egui::Context::default();
            let mut time = 0.0;
            let mut frame = |app: &mut crate::SolveApp, mods: Modifiers, events: Vec<Event>| {
                time += 1.0 / 60.0;
                let events = std::iter::once(Event::ModifiersChanged(mods)).chain(events).collect();
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))),
                    time: Some(time),
                    focused: true,
                    events,
                    ..Default::default()
                };
                let mut out = ctx.run_ui(raw, |ui| {
                    let rect = ui.max_rect();
                    app.viewport.rect = Some(rect);
                    let resp = ui.interact(rect, ui.id().with("flat"), egui::Sense::click_and_drag());
                    super::handle(app, ui, rect, &resp, flat);
                });
                out.textures_delta.clear();
            };
            frame(&mut app, Modifiers::NONE, vec![]);
            assert!(!app.viewport.spin, "{preset}: continuous orbit stops on a flat page");
            let mut panned = false;
            let at = pos2(200.0, 300.0);
            for button in [PointerButton::Primary, PointerButton::Middle, PointerButton::Secondary] {
                for mods in [Modifiers::NONE, Modifiers::SHIFT, Modifiers::CTRL, Modifiers::ALT, Modifiers::SHIFT | Modifiers::ALT] {
                    let before = app.cam;
                    frame(&mut app, mods, vec![Event::PointerMoved(at)]);
                    frame(&mut app, mods, vec![Event::PointerButton { pos: at, button, pressed: true, modifiers: mods }]);
                    for k in 1..5 {
                        frame(&mut app, mods, vec![Event::PointerMoved(at + vec2(12.0, 7.0) * k as f32)]);
                    }
                    frame(&mut app, mods, vec![Event::PointerButton { pos: at + vec2(48.0, 28.0), button, pressed: false, modifiers: mods }]);
                    frame(&mut app, Modifiers::NONE, vec![]);
                    assert_eq!((app.cam.yaw, app.cam.pitch), (before.yaw, before.pitch), "{preset}: {button:?} with {mods:?} turned a flat page");
                    panned |= app.cam.target != before.target;
                }
            }
            assert!(panned, "{preset}: some binding pans");
            let before = app.cam;
            let wheel = Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, 3.0),
                modifiers: Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            };
            frame(&mut app, Modifiers::NONE, vec![Event::PointerMoved(at), wheel]);
            for _ in 0..20 {
                frame(&mut app, Modifiers::NONE, vec![]);
            }
            assert_ne!(app.cam.distance, before.distance, "{preset}: the wheel zooms");
        }
    }

    /// The 3D mouse pans and zooms a flat page with the cap and never turns it, as the mouse
    /// does there; pages that orbit turn with it (#1336).
    #[test]
    fn the_3d_mouse_pans_and_zooms_a_flat_page_but_never_turns_it() {
        use crate::space_mouse::{DeviceMotion, Motion};
        let every_axis = DeviceMotion { motion: Motion { translation: [1.0, 1.0, 1.0], rotation: [1.0, 1.0, 1.0] }, seconds: 0.05 };
        let flat = super::NavCaps { orbit: false, pick: false };
        for (caps, turns) in [(flat, false), (super::NavCaps::MODEL, true), (super::NavCaps::RENDER, true)] {
            let mut s = solidcraft_engine::Session::default();
            s.execute("solid.box", &serde_json::json!({"length":40,"width":30,"height":20})).unwrap();
            let mut app = crate::SolveApp::new(s, crate::Services::default());
            let ctx = egui::Context::default();
            let before = app.cam;
            app.viewport.device_motion = Some(every_axis);
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                time: Some(0.0),
                focused: true,
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                let rect = ui.max_rect();
                app.viewport.rect = Some(rect);
                let resp = ui.interact(rect, ui.id().with("page"), egui::Sense::click_and_drag());
                super::handle(&mut app, ui, rect, &resp, caps);
            });
            out.textures_delta.clear();
            assert!(app.viewport.device_motion.is_none(), "{caps:?}: the page takes the motion");
            assert_ne!(app.cam.target, before.target, "{caps:?}: the cap pans");
            assert!(app.cam.distance < before.distance, "{caps:?}: the cap zooms");
            let turned = (app.cam.yaw, app.cam.pitch, app.cam.roll) != (before.yaw, before.pitch, before.roll);
            assert_eq!(turned, turns, "{caps:?}: the cap turns the view only where the page orbits");
        }
    }

    /// While a sketch is edited the cap pans and zooms, and the view stays square to the sketch.
    #[test]
    fn the_3d_mouse_never_turns_a_sketch_being_edited() {
        use crate::space_mouse::{DeviceMotion, Motion};
        let mut s = solidcraft_engine::Session::default();
        s.execute("sketch.create", &serde_json::json!({"plane": "XY"})).unwrap();
        let mut app = crate::SolveApp::new(s, crate::Services::default());
        assert!(app.session.active_sketch.is_some());
        let before = app.cam;
        app.viewport.device_motion = Some(DeviceMotion { motion: Motion { translation: [1.0; 3], rotation: [1.0; 3] }, seconds: 0.05 });
        let page = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        super::device(&mut app, page, super::NavCaps::MODEL);
        assert_eq!((app.cam.yaw, app.cam.pitch, app.cam.roll), (before.yaw, before.pitch, before.roll));
        assert_ne!(app.cam.target, before.target);
        assert!(app.cam.distance < before.distance);
        // Nothing queued, nothing moves.
        let still = app.cam;
        super::device(&mut app, page, super::NavCaps::MODEL);
        assert_eq!(app.cam, still);
    }
}

#[cfg(test)]
mod view_key_tests {
    use crate::scenario::Harness;
    use serde_json::json;

    fn turned(h: &mut Harness, key: serde_json::Value) -> (f64, f64) {
        let before = h.app.cam;
        h.step(&key).unwrap();
        h.frames(2);
        (h.app.cam.yaw - before.yaw, h.app.cam.pitch - before.pitch)
    }

    /// #1541: the arrows turn the view 15°, Shift+arrows a quarter turn, Ctrl+arrows pan, and none
    /// of them is a shortcut a command can take.
    #[test]
    fn arrow_keys_turn_and_pan_the_view() {
        let mut h = Harness::new();
        h.app.preferences.look_at_anim = false;
        h.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10})).unwrap();
        h.step(&json!({"call": "ui.view", "params": {"view": "front"}})).unwrap();
        h.frames(3);
        let close = |a: f64, b: f64| (a.abs() - b.to_radians()).abs() < 1e-6;
        let (yaw, pitch) = turned(&mut h, json!({"key": "ArrowLeft"}));
        assert!(close(yaw, 15.0) && close(pitch, 0.0), "Left turns 15°: {yaw} {pitch}");
        let (yaw, pitch) = turned(&mut h, json!({"key": "ArrowUp"}));
        assert!(close(yaw, 0.0) && close(pitch, 15.0), "Up turns 15° over: {yaw} {pitch}");
        let (yaw, _) = turned(&mut h, json!({"key": "ArrowRight", "shift": true}));
        assert!(close(yaw, 90.0), "Shift+Right is a quarter turn: {yaw}");
        let (before, angles) = (h.app.cam.target, (h.app.cam.yaw, h.app.cam.pitch));
        h.step(&json!({"key": "ArrowRight", "cmd": true})).unwrap();
        h.frames(2);
        assert_ne!(h.app.cam.target, before, "Ctrl+Right pans");
        assert_eq!((h.app.cam.yaw, h.app.cam.pitch), angles, "and does not turn");
        for k in [egui::Key::ArrowLeft, egui::Key::ArrowRight, egui::Key::ArrowUp, egui::Key::ArrowDown] {
            for mods in [egui::Modifiers::NONE, egui::Modifiers::SHIFT, egui::Modifiers::COMMAND] {
                let name = crate::keymap::key_name(k, mods);
                assert!(crate::keymap::RESERVED.contains(&name.as_str()), "{name} must be reserved");
            }
        }
        assert!(crate::keymap::RESERVED.contains(&crate::keymap::key_name(egui::Key::Num8, egui::Modifiers::COMMAND).as_str()));
    }

    /// #1886: Ctrl+8 looks straight at the sketch; pressed again while the view faces it, it
    /// looks from the other side, and a third press turns it back. Opening a sketch never flips.
    #[test]
    fn ctrl_8_again_looks_from_the_other_side() {
        let mut h = Harness::new();
        h.app.preferences.look_at_anim = false;
        h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
        h.frames(2);
        let back_z = |h: &Harness| h.app.cam.back().z;
        for want in [1.0, -1.0, 1.0] {
            h.step(&json!({"key": "8", "cmd": true})).unwrap();
            h.frames(2);
            assert!((back_z(&h) - want).abs() < 1e-3, "looking from z {want}: back {:?}", h.app.cam.back());
        }
        // Programmatic look-at (as on opening a sketch) keeps the sketch's own side.
        crate::dialogs::look_at_sketch(&mut h.app);
        h.frames(2);
        crate::dialogs::look_at_sketch(&mut h.app);
        h.frames(2);
        assert!((back_z(&h) - 1.0).abs() < 1e-3, "{:?}", h.app.cam.back());
    }
}
