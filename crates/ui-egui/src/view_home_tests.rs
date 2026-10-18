// Home and Fit frame what is shown now, and a middle-button double-click goes Home.
use crate::scenario::Harness;
use serde_json::json;
use solidcraft_engine::geom::{Aabb3, Vec3};
use solidcraft_engine::render::StandardView;

/// A block drawn at the origin and extruded, then moved 500 mm along X: its sketch stays at the
/// origin, used by the extrude (so not drawn).
fn moved_block(h: &mut Harness) {
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [10, 10]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("solid.extrude", json!({"distance": 5})).unwrap();
    let body = h.app.session.world_state().bodies[0].name.clone();
    h.app.run("solid.move", json!({"bodies": [body], "translate": [500, 0, 0]})).unwrap();
    h.app.preferences.look_at_anim = false;
    h.settle();
}

fn near_box(a: Aabb3, lo: [f64; 3], hi: [f64; 3]) {
    let (lo, hi) = (Vec3::new(lo[0], lo[1], lo[2]), Vec3::new(hi[0], hi[1], hi[2]));
    assert!(a.min.dist(lo) < 1e-6 && a.max.dist(hi) < 1e-6, "{a:?} != {lo:?}..{hi:?}");
}

/// The camera Home gives: the iso view framing what is shown.
fn home(h: &Harness) -> solidcraft_engine::render::Camera {
    let mut cam = h.app.cam;
    cam.set_view(StandardView::Iso);
    h.app.fitted(cam)
}

/// Somewhere in the view away from the model and the overlays.
fn spot(h: &Harness) -> egui::Pos2 {
    let d = h.app.viewport.layout.as_ref().unwrap().drawable;
    d.left_top() + egui::vec2(d.width() * 0.3, d.height() * 0.7)
}

fn same_cam(a: &solidcraft_engine::render::Camera, b: &solidcraft_engine::render::Camera) -> bool {
    a.target.dist(b.target) < 1e-6 && (a.distance - b.distance).abs() < 1e-6 && (a.yaw - b.yaw).abs() < 1e-9 && (a.pitch - b.pitch).abs() < 1e-9
}

#[test]
fn home_and_fit_frame_the_moved_body_not_its_old_place() {
    let mut h = Harness::new();
    moved_block(&mut h);
    near_box(crate::viewport::view_bounds(&h.app), [500., 0., 0.], [510., 10., 5.]);
    for view in ["home", "fit"] {
        h.app.cam.pan(300.0, 200.0, 500.0);
        h.app.animate_view(view);
        h.settle();
        // Near the body's centre (the fit leaves room for the palettes), not 250 mm away.
        assert!(h.app.cam.target.dist(Vec3::new(505.0, 5.0, 2.5)) < 5.0, "{view} centres the body: {:?}", h.app.cam.target);
        assert!(h.app.cam.distance < 200.0, "{view} frames 10 mm, not 510: {}", h.app.cam.distance);
    }
    // The orbit pivot over empty space (#464) is the centre of the same box.
    let pivot = crate::viewport::view_bounds(&h.app).center();
    assert!(pivot.dist(Vec3::new(505.0, 5.0, 2.5)) < 1e-6);
    // The Sketches folder hidden: an unused sketch no longer counts either.
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.run("sketch.circle.center", json!({"center": [-300, 0], "radius": 5})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    near_box(crate::viewport::view_bounds(&h.app), [-305., -5., 0.], [510., 10., 5.]);
    h.app.session.visibility.sketches_hidden = true;
    near_box(crate::viewport::view_bounds(&h.app), [500., 0., 0.], [510., 10., 5.]);
}

#[test]
fn middle_double_click_goes_home_in_every_preset() {
    for preset in crate::prefs::NAV_PRESETS.iter().map(|p| p.id) {
        let mut h = Harness::new();
        h.call("ui.prefs", json!({"nav": preset}));
        moved_block(&mut h);
        h.app.cam.set_view(StandardView::Front);
        h.app.cam.pan(120.0, -80.0, 500.0);
        let p = spot(&h);
        h.call("ui.click", json!({"x": p.x, "y": p.y, "button": "middle", "double": true}));
        h.settle();
        let want = home(&h);
        assert!(same_cam(&h.app.cam, &want), "{preset}: {:?} != {:?}", h.app.cam, want);
        assert!(h.app.cam.target.dist(Vec3::new(505.0, 5.0, 2.5)) < 5.0, "{preset}: {:?}", h.app.cam.target);
    }
}

#[test]
fn middle_drags_and_single_clicks_do_not_go_home() {
    for preset in crate::prefs::NAV_PRESETS.iter().map(|p| p.id) {
        let mut h = Harness::new();
        h.call("ui.prefs", json!({"nav": preset}));
        moved_block(&mut h);
        h.app.cam.set_view(StandardView::Front);
        h.app.cam.pan(120.0, -80.0, 500.0);
        let p = spot(&h);
        // A middle drag (pan or orbit), then a click where it ended: a drag and one click.
        h.call("ui.drag", json!({"x0": p.x, "y0": p.y, "x1": p.x + 40.0, "y1": p.y + 25.0, "button": "middle", "steps": 4}));
        h.settle();
        let dragged = h.app.cam;
        let q = p + egui::vec2(40.0, 25.0);
        h.call("ui.click", json!({"x": q.x, "y": q.y, "button": "middle"}));
        h.settle();
        assert!(same_cam(&h.app.cam, &dragged), "{preset}: a drag and a click went home");
        // A left click and then a middle click are not a middle double-click.
        h.call("ui.click", json!({"x": q.x, "y": q.y}));
        h.call("ui.click", json!({"x": q.x, "y": q.y, "button": "middle"}));
        h.settle();
        assert!(same_cam(&h.app.cam, &dragged), "{preset}: left + middle went home");
    }
}

#[test]
fn middle_double_click_preference_fit_or_off() {
    let mut h = Harness::new();
    moved_block(&mut h);
    let bad = h.call("ui.prefs", json!({"middle_double_click": "zoom"}));
    assert!(bad.to_string().contains("middle_double_click"), "{bad}");
    assert_eq!(h.app.preferences.middle_double_click, "home");

    h.call("ui.prefs", json!({"middle_double_click": "fit"}));
    h.app.cam.set_view(StandardView::Front);
    h.app.cam.pan(120.0, -80.0, 500.0);
    let (yaw, pitch) = (h.app.cam.yaw, h.app.cam.pitch);
    let p = spot(&h);
    h.call("ui.click", json!({"x": p.x, "y": p.y, "button": "middle", "double": true}));
    h.settle();
    assert_eq!((h.app.cam.yaw, h.app.cam.pitch), (yaw, pitch), "Fit keeps the orientation");
    assert!(h.app.cam.target.dist(Vec3::new(505.0, 5.0, 2.5)) < 5.0, "{:?}", h.app.cam.target);
    // Fit frames everything shown, not the selection.
    let body = h.app.session.world_state().bodies[0].name.clone();
    h.app.run("select.set", json!({"items": [{"type": "body", "name": body}]})).unwrap();
    let fitted = h.app.cam;
    h.app.cam.pan(60.0, 0.0, 500.0);
    // Well after the last pair (a third click soon after a double-click is a triple-click).
    h.frames(60);
    h.call("ui.click", json!({"x": p.x, "y": p.y, "button": "middle", "double": true}));
    h.settle();
    assert!(same_cam(&h.app.cam, &fitted));

    h.call("ui.prefs", json!({"middle_double_click": "off"}));
    h.app.cam.pan(60.0, 0.0, 500.0);
    let moved = h.app.cam;
    h.frames(60);
    h.call("ui.click", json!({"x": p.x, "y": p.y, "button": "middle", "double": true}));
    h.settle();
    assert!(same_cam(&h.app.cam, &moved), "Off does nothing");
}
