use egui::{Rect, pos2, vec2};
use serde_json::json;

use crate::SolveApp;
use crate::viewport::{Hit, pick, pick_all, projection};

/// Overlapping bodies with flat and curved faces, filleted edges and seams.
fn assembly() -> SolveApp {
    let mut s = solidcraft_engine::Session::default();
    s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
    s.execute("solid.fillet", &json!({"edges": [[40, 15, 20]], "radius": 4})).unwrap();
    s.execute("solid.cylinder", &json!({"radius": 8, "height": 35, "base": [10, 10, 0], "operation": "new_body"})).unwrap();
    s.execute("solid.sphere", &json!({"radius": 9, "center": [30, 25, 22]})).unwrap();
    let mut app = SolveApp::new(s, crate::Services::default());
    app.set_origin_shown(false);
    app
}

/// Every cursor position on a grid over the view, for each camera: the indexed pick answers what
/// scanning every triangle and edge answers (the nearest of each kind, and Select Other's list).
#[test]
fn indexed_picks_match_the_full_scan() {
    let mut app = assembly();
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 480.0));
    let mut compared = (0, 0, 0);
    for (perspective, yaw, pitch) in [(false, 0.6, 0.5), (true, 0.6, 0.5), (true, 2.4, -0.3), (false, 0.0, 1.5)] {
        app.ui.perspective = perspective;
        app.cam.yaw = yaw;
        app.cam.pitch = pitch;
        app.cam.target = solidcraft_engine::geom::Vec3::new(20.0, 15.0, 12.0);
        app.cam.distance = 110.0;
        let proj = projection(&app, rect);
        for j in 0..48 {
            for i in 0..64 {
                let p = pos2(5.0 + i as f32 * 10.0 + (j % 3) as f32, 5.0 + j as f32 * 10.0);
                let fast = (pick(&app, &proj, p), pick_all(&app, &proj, p));
                super::SCAN.with(|s| s.set(true));
                let scan = (pick(&app, &proj, p), pick_all(&app, &proj, p));
                super::SCAN.with(|s| s.set(false));
                assert_eq!(fast, scan, "at {p:?} (perspective {perspective}, yaw {yaw}, pitch {pitch})");
                compared.0 += usize::from(fast.0.iter().any(|h| matches!(h, Hit::Face { .. })));
                compared.1 += usize::from(fast.0.iter().any(|h| matches!(h, Hit::Edge { .. })));
                compared.2 += usize::from(fast.0.iter().any(|h| matches!(h, Hit::Vertex { .. })));
            }
        }
    }
    // The grid reaches every kind many times over.
    assert!(compared.0 > 2000 && compared.1 > 100 && compared.2 > 10, "{compared:?}");
    assert_eq!(app.viewport.pick_index.len(), 3, "one index per body mesh");
}

/// A new model state lets go of the indexes of meshes it no longer has.
#[test]
fn indexes_follow_the_model_state() {
    let mut app = assembly();
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 480.0));
    app.cam.target = solidcraft_engine::geom::Vec3::new(20.0, 15.0, 12.0);
    let proj = projection(&app, rect);
    let centre = proj.to_screen(solidcraft_engine::geom::Vec3::new(20.0, 15.0, 10.0)).unwrap();
    pick(&app, &proj, centre);
    assert!(app.viewport.pick_index.len() >= 1);
    app.session.execute("solid.box", &json!({"length": 5, "width": 5, "height": 5, "corner": [-20, -20, 0], "operation": "new_body"})).unwrap();
    pick(&app, &proj, centre);
    assert!(app.viewport.pick_index.len() <= 4, "indexes of meshes gone with the old state are dropped");
    app.session.execute("solid.box", &json!({"length": 5, "width": 5, "height": 5, "corner": [-40, -20, 0], "operation": "new_body"})).unwrap();
    pick(&app, &proj, centre);
    assert!(app.viewport.pick_index.len() <= 5);
}

/// Timing, not a check (`cargo test -p solidcraft-ui-egui --lib pick_timing -- --ignored
/// --nocapture`): picks over a grid of the view, scanning every triangle and edge versus through
/// the indexes, on many curved bodies and on one body with many curved faces.
#[test]
#[ignore]
fn pick_timing_on_curved_models() {
    let mut many = solidcraft_engine::Session::default();
    for i in 0..12 {
        for j in 0..12 {
            let (x, y) = (f64::from(i) * 30.0, f64::from(j) * 30.0);
            many.execute("solid.cylinder", &json!({"radius": 9, "height": 30, "base": [x, y, 0], "operation": "new_body"})).unwrap();
            many.execute("solid.sphere", &json!({"radius": 8, "center": [x, y, 40], "operation": "new_body"})).unwrap();
        }
    }
    let mut one = solidcraft_engine::Session::default();
    one.execute("solid.box", &json!({"length": 340, "width": 340, "height": 10, "corner": [-15, -15, -5]})).unwrap();
    for i in 0..8 {
        for j in 0..8 {
            let (x, y) = (f64::from(i) * 42.0, f64::from(j) * 42.0);
            one.execute("solid.cylinder", &json!({"radius": 14, "height": 30, "base": [x, y, 0], "operation": "join"})).unwrap();
        }
    }
    for (name, s) in [("many bodies", many), ("one body", one)] {
        let mut app = SolveApp::new(s, crate::Services::default());
        app.set_origin_shown(false);
        app.cam.target = solidcraft_engine::geom::Vec3::new(160.0, 160.0, 15.0);
        app.cam.distance = 900.0;
        let st = app.session.world_state();
        let (tris, segs) = st
            .bodies
            .iter()
            .map(|b| b.mesh())
            .fold((0, 0), |(t, s), m| (t + m.triangles.len(), s + m.edges.iter().map(|e| e.len().saturating_sub(1)).sum::<usize>()));
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 900.0));
        let proj = projection(&app, rect);
        let points: Vec<_> = (0..20).flat_map(|j| (0..30).map(move |i| pos2(30.0 + i as f32 * 45.0, 30.0 + j as f32 * 42.0))).collect();
        // Indexes made once, before timing.
        for p in &points {
            pick(&app, &proj, *p);
        }
        let time = |scan: bool| {
            super::SCAN.with(|s| s.set(scan));
            let t = std::time::Instant::now();
            for p in &points {
                std::hint::black_box(pick(&app, &proj, *p));
            }
            super::SCAN.with(|s| s.set(false));
            t.elapsed().as_secs_f64() * 1000.0 / points.len() as f64
        };
        let (scan, indexed) = (time(true), time(false));
        eprintln!("{name}: {} bodies, {tris} triangles, {segs} edge segments: scan {scan:.3} ms per pick, indexed {indexed:.3} ms", st.bodies.len());
    }
}
