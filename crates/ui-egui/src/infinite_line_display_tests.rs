//! An infinite line (#1359) is drawn across the whole view and picked along its whole line,
//! and the Sketch Palette's Infinite mode draws one.

use serde_json::json;

use crate::scenario::Harness;
use crate::viewport::{Hit, pick, projection};

/// An XY sketch with a short line from (0, 0) to (2, 0) made infinite, the view zoomed out.
fn short_infinite_line() -> Harness {
    let mut h = Harness::new();
    h.app.home.open = false;
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.run("sketch.line", json!({"points": [[0, 0], [2, 0]], "ids": ["inf"]})).unwrap();
    h.app.run("sketch.infinite", json!({"curves": ["inf"]})).unwrap();
    h.app.cam.distance = 400.0;
    h.settle();
    h
}

#[test]
fn it_is_drawn_from_edge_to_edge_of_the_view() {
    let h = short_infinite_line();
    let rect = h.app.viewport.rect.unwrap();
    let proj = projection(&h.app, rect);
    let st = h.app.session.world_state();
    let ss = st.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    let i = ss.sketch.curve_index("inf").unwrap();
    let parts = super::curve_parts(&ss.sketch, i, &ss.plane, &proj, 1.0);
    let screen: Vec<_> = parts.iter().flatten().filter_map(|p| proj.to_screen(*p)).collect();
    let (first, last) = (screen.first().unwrap(), screen.last().unwrap());
    // Its two defining points are a few pixels apart; drawn, it spans the view. At the
    // default isometric angle a wide viewport clips the line at the top and bottom instead
    // of the left and right, so either pair of edges is valid.
    for point in [first, last] {
        assert!(rect.expand(2.0).contains(*point) && !rect.shrink(2.0).contains(*point), "endpoint {point:?} must reach an edge of {rect:?}");
    }
    assert!(first.distance(*last) >= rect.width().min(rect.height()) - 4.0, "line must span the view: {screen:?} in {rect:?}");
}

#[test]
fn it_is_picked_far_beyond_its_two_points() {
    let h = short_infinite_line();
    let rect = h.app.viewport.rect.unwrap();
    let proj = projection(&h.app, rect);
    let ss_plane = h.app.session.world_state().sketch(h.app.session.active_sketch.unwrap()).unwrap().plane;
    let far = proj.to_screen(ss_plane.to_world(solidcraft_engine::geom::Vec2::new(150.0, 0.0))).unwrap();
    assert!(rect.contains(far));
    let hits = pick(&h.app, &proj, far);
    assert!(hits.iter().any(|h| matches!(h, Hit::SketchCurve { id, .. } if id == "inf")), "{hits:?}");
}

#[test]
fn infinite_mode_draws_infinite_lines() {
    let mut h = Harness::new();
    h.app.home.open = false;
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.settle();
    crate::sketch_palette::set_mode(&mut h.app, 3);
    let r = h.app.run("sketch.line", json!({"points": [[0, 0], [10, 5]]})).unwrap();
    crate::sketch_palette::apply_mode(&mut h.app, "sketch.line", &r);
    let id = r["curves"][0].as_str().unwrap().to_string();
    let sk = h.app.session.doc.sketch(h.app.session.active_sketch.unwrap()).unwrap();
    let c = sk.curves.get(sk.curve_index(&id).unwrap()).unwrap();
    assert!(c.infinite && c.construction, "{c:?}");
}
