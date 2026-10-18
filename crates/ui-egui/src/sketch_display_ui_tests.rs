//! Independent physical-pixel and interaction proofs for display-only sampling.
use crate::scenario::Harness;
use crate::sketch_display::{curve_sample, projection_key};
use crate::viewport::{Hit, Proj, pick, projection};
use serde_json::json;
use solidcraft_engine::geom::{Plane, Vec2, Vec3};
use solidcraft_engine::render::{SampledPath, SamplingQuality};
use std::sync::Arc;

fn circle(h: &mut Harness) -> u64 {
    h.app.set_origin_shown(false);
    h.app.run("parameters.add", json!({"name":"foreground_diameter","expression":"2 mm"})).unwrap();
    h.app.run("sketch.create", json!({"plane":"XY","name":"PixelCircle"})).unwrap();
    h.app.run("sketch.circle.center", json!({"center":[0,0],"radius":1})).unwrap();
    h.app.run("sketch.dimension", json!({"entities":["c1"],"type":"diameter","value":"foreground_diameter"})).unwrap();
    h.settle();
    h.step(&json!({"call":"ui.view","params":{"view":"top"}})).unwrap();
    h.step(&json!({"call":"ui.view","params":{"view":"fit"}})).unwrap();
    h.settle();
    h.app.session.active_sketch.unwrap()
}

fn segment_distance(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f64 {
    let dx = f64::from(b.x - a.x);
    let dy = f64::from(b.y - a.y);
    let px = f64::from(p.x - a.x);
    let py = f64::from(p.y - a.y);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 { ((px * dx + py * dy) / length).clamp(0.0, 1.0) } else { 0.0 };
    (px - t * dx).hypot(py - t * dy)
}

fn dense_circle_error(path: &SampledPath, plane: &Plane, proj: &Proj, radius: f64, ppp: f32) -> f64 {
    assert_eq!(path.quality, SamplingQuality::Complete);
    assert!(path.vertex_count <= 4096);
    let lines: Vec<_> =
        path.parts.iter().flat_map(|part| part.windows(2)).map(|pair| (proj.to_screen(pair[0]).unwrap(), proj.to_screen(pair[1]).unwrap())).collect();
    assert!(!lines.is_empty());
    (0..8192)
        .map(|i| {
            let angle = i as f64 * std::f64::consts::TAU / 8192.0;
            let p = proj.to_screen(plane.to_world(Vec2::new(radius * angle.cos(), radius * angle.sin()))).unwrap();
            lines.iter().map(|&(a, b)| segment_distance(p, a, b)).fold(f64::INFINITY, f64::min) * f64::from(ppp)
        })
        .fold(0.0, f64::max)
}

#[test]
fn pixel_circle_fit_exact_half_chord_hover_click_and_model_invariance() {
    let mut h = Harness::new();
    let sid = circle(&mut h);
    let baseline = h.app.session.doc.to_json();
    let history = (h.app.session.undo.len(), h.app.session.redo.len());
    let state = h.app.session.world_state();
    let ss = state.sketch(sid).unwrap();
    let plane = ss.plane;
    let index = ss.sketch.curve_index("c1").unwrap();
    // Ordinary circles already expose exact analytic Arc spans through segs;
    // exact_segs is the supplemental freeform extractor.
    let area: f64 = ss.sketch.segs(index).iter().map(|s| s.area_term()).sum();
    assert!((area.abs() - std::f64::consts::PI).abs() < 1e-10);
    let proj = projection(&h.app, h.app.viewport.rect.unwrap());
    let radius_px = proj.to_screen(plane.to_world(Vec2::new(1.0, 0.0))).unwrap().distance(proj.to_screen(plane.origin).unwrap())
        * h.app.viewport.pixels_per_point();
    assert!(radius_px > 180.0, "fixture must expose the old R1 Fit sag: {radius_px}");
    let sampled = curve_sample(&h.app, sid, index, &plane, &proj);
    assert!(dense_circle_error(&sampled, &plane, &proj, 1.0, h.app.viewport.pixels_per_point()) <= 1.0);
    // Midpoints of the old ten-chord pick outline. No centre/point handle is nearby.
    for k in [0, 2, 5, 7] {
        let angle = (2 * k + 1) as f64 * std::f64::consts::PI / 10.0;
        let world = plane.to_world(Vec2::new(angle.cos(), angle.sin()));
        let screen = proj.to_screen(world).unwrap();
        assert!(pick(&h.app, &proj, screen).iter().any(|hit| matches!(hit, Hit::SketchCurve { sketch, id, .. } if *sketch == sid && id == "c1")));
        h.step(&json!({"move":{"world":[world.x,world.y,world.z]}})).unwrap();
        assert!(
            matches!(&h.app.viewport.hover, Some(Hit::SketchCurve{sketch,id,..}) if *sketch == sid && id == "c1"),
            "hover: {:?}",
            h.app.viewport.hover
        );
        h.step(&json!({"click":{"world":[world.x,world.y,world.z]}})).unwrap();
        assert!(h.app.session.selection.iter().any(|s| matches!(s, solidcraft_engine::Sel::SketchCurve{id} if id == "c1")));
        h.app.session.selection.clear();
    }
    assert_eq!(h.app.session.doc.to_json(), baseline, "display and selection must not alter stored geometry, constraints or expressions");
    assert_eq!((h.app.session.undo.len(), h.app.session.redo.len()), history);
}

#[test]
fn pixel_cache_full_view_resize_and_hidpi_invalidate_within_same_grid_bucket() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"corner":[-0.1,-0.1,-0.2],"length":0.2,"width":0.2,"height":0.2,"body_name":"CachedStock"})).unwrap();
    let sid = circle(&mut h);
    // This cache test needs a controlled projected radius, independent of floating controls
    // used by Fit. At this radius, doubling physical scale crosses a subdivision threshold.
    h.app.cancel_view_animation();
    h.app.cam.distance = 5.0;
    h.app.cam.target = Vec3::ZERO;
    let baseline = h.app.session.doc.to_json();
    let history = (h.app.session.undo.len(), h.app.session.redo.len());
    let state = h.app.session.world_state();
    assert_eq!(state.bodies.len(), 1, "body batch invariance needs an actual mesh");
    let ss = state.sketch(sid).unwrap();
    let index = ss.sketch.curve_index("c1").unwrap();
    let plane = ss.plane;
    let first = projection(&h.app, h.app.viewport.rect.unwrap());
    let keys = crate::viewport::display_scene_keys(&h.app, &first);
    let a = curve_sample(&h.app, sid, index, &plane, &first);
    assert!(Arc::ptr_eq(&a, &curve_sample(&h.app, sid, index, &plane, &first)));
    let grid = solidcraft_engine::view::grid_step(h.app.cam.half_height());
    // Find a real zoom change which keeps the complete grid bucket unchanged.
    let distance = h.app.cam.distance;
    for factor in [0.8, 0.9, 1.1, 1.2] {
        h.app.cam.distance = distance * factor;
        if solidcraft_engine::view::grid_step(h.app.cam.half_height()) == grid {
            break;
        }
    }
    assert_eq!(solidcraft_engine::view::grid_step(h.app.cam.half_height()), grid);
    assert_ne!(h.app.cam.distance, distance);
    let zoom = projection(&h.app, first.rect);
    let zoom_keys = crate::viewport::display_scene_keys(&h.app, &zoom);
    assert_eq!(keys.0, zoom_keys.0, "sketch zoom must not invalidate the body GPU batch");
    assert_ne!(keys.1, zoom_keys.1);
    assert_ne!(projection_key(&first, 1.0), projection_key(&zoom, 1.0));
    let b = curve_sample(&h.app, sid, index, &plane, &zoom);
    assert!(!Arc::ptr_eq(&a, &b), "full VP must invalidate even when grid spacing is unchanged");
    assert!(dense_circle_error(&b, &plane, &zoom, 1.0, 1.0) <= 1.0);
    let resized = projection(&h.app, egui::Rect::from_min_size(first.rect.min, first.rect.size() * 0.75));
    let resized_keys = crate::viewport::display_scene_keys(&h.app, &resized);
    assert_eq!(keys.0, resized_keys.0);
    assert_ne!(zoom_keys.1, resized_keys.1);
    let c = curve_sample(&h.app, sid, index, &plane, &resized);
    assert!(!Arc::ptr_eq(&b, &c));
    h.app.viewport.set_pixels_per_point(2.0);
    let hidpi_keys = crate::viewport::display_scene_keys(&h.app, &resized);
    assert_eq!(keys.0, hidpi_keys.0);
    assert_ne!(resized_keys.1, hidpi_keys.1);
    let d = curve_sample(&h.app, sid, index, &plane, &resized);
    assert!(!Arc::ptr_eq(&c, &d));
    assert!(d.vertex_count > c.vertex_count, "HiDPI must refine physical error, not just invalidate a key");
    assert!(dense_circle_error(&d, &plane, &resized, 1.0, 2.0) <= 1.0);
    assert_eq!(h.app.session.doc.to_json(), baseline);
    assert_eq!((h.app.session.undo.len(), h.app.session.redo.len()), history);
}

#[test]
fn pixel_adapter_uses_exact_ellipse_periodic_fit_and_control_spline_spans() {
    for (command, params) in [
        ("sketch.ellipse", json!({"center":[0,0],"major":[10,0],"minor_radius":0.3})),
        ("sketch.spline.fit_point", json!({"points":[[0,0],[10,2],[5,10],[-1,4]],"periodic":true,"connect":false})),
        ("sketch.spline.control_point_5", json!({"points":[[0,0],[2,12],[4,-12],[6,12],[8,-12],[10,0]],"connect":false})),
    ] {
        let mut h = Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run(command, params).unwrap();
        h.settle();
        h.step(&json!({"call":"ui.view","params":{"view":"top"}})).unwrap();
        h.step(&json!({"call":"ui.view","params":{"view":"fit"}})).unwrap();
        h.settle();
        let baseline = h.app.session.doc.to_json();
        let sid = h.app.session.active_sketch.unwrap();
        let state = h.app.session.world_state();
        let ss = state.sketch(sid).unwrap();
        let exact = ss.sketch.exact_segs(0).unwrap();
        assert!(
            exact.iter().all(|s| !matches!(s, solidcraft_engine::geom::Seg2::Line { .. })),
            "freeform display must reach the exact span extractor"
        );
        let proj = projection(&h.app, h.app.viewport.rect.unwrap());
        let path = curve_sample(&h.app, sid, 0, &ss.plane, &proj);
        assert_eq!(path.quality, SamplingQuality::Complete, "{command}");
        assert!(path.vertex_count <= 4096);
        let lines: Vec<_> =
            path.parts.iter().flat_map(|p| p.windows(2)).map(|p| (proj.to_screen(p[0]).unwrap(), proj.to_screen(p[1]).unwrap())).collect();
        let mut worst = 0.0f64;
        for segment in exact {
            for i in 0..=2048 {
                let point = segment.point_at(i as f64 / 2048.0);
                let screen = proj.to_screen(ss.plane.to_world(point)).unwrap();
                let distance = lines.iter().map(|&(a, b)| segment_distance(screen, a, b)).fold(f64::INFINITY, f64::min);
                worst = worst.max(distance * f64::from(h.app.viewport.pixels_per_point()));
            }
        }
        assert!(worst <= 1.0, "{command} projected error: {worst}px");
        assert_eq!(h.app.session.doc.to_json(), baseline);
    }
}

#[test]
fn pixel_curve_second_nested_rotated_occurrence_uses_its_world_plane() {
    let mut h = Harness::new();
    let parent = h.app.run("component.create", json!({"name":"Assembly"})).unwrap();
    let parent_component = parent["component"].as_u64().unwrap();
    let first = parent["occurrence"].as_u64().unwrap();
    let child = h.app.run("component.create", json!({"name":"Part"})).unwrap();
    let component = child["component"].as_u64().unwrap();
    let leaf = child["occurrence"].as_u64().unwrap();
    h.app.run("component.activate", json!({"component":0})).unwrap();
    let second = h.app.run("occurrence.copy", json!({"component":parent_component})).unwrap()["occurrence"].as_u64().unwrap();
    h.app.run("occurrence.move", json!({"occurrence":second,"translate":[20,30,40],"axis":[1,0,0],"angle":"35 deg","capture":true})).unwrap();
    h.app.run("component.activate", json!({"component":component,"occurrence":leaf,"path":[second,leaf]})).unwrap();
    h.app.run("browser.visibility", json!({"paths":[[first]],"visible":false})).unwrap();
    let sid = circle(&mut h);
    let baseline = h.app.session.doc.to_json();
    let state = h.app.session.world_state();
    let ss = state.sketch(sid).unwrap();
    assert!(ss.plane.origin.dist(Vec3::new(20.0, 30.0, 40.0)) < 1e-8);
    let tilt = 35.0f64.to_radians();
    assert!(ss.plane.x.dist(Vec3::X) < 1e-8);
    assert!(
        ss.plane.y.dist(Vec3::new(0.0, tilt.cos(), tilt.sin())) < 1e-8,
        "independent expected occurrence rotation must tilt the actual sketch plane"
    );
    let proj = projection(&h.app, h.app.viewport.rect.unwrap());
    let path = curve_sample(&h.app, sid, 0, &ss.plane, &proj);
    assert!(dense_circle_error(&path, &ss.plane, &proj, 1.0, h.app.viewport.pixels_per_point()) <= 1.0);
    let world = ss.plane.to_world(Vec2::new(0.8090169943749475, 0.5877852522924731));
    h.step(&json!({"click":{"world":[world.x,world.y,world.z]}})).unwrap();
    assert!(h.app.session.selection.iter().any(|s| matches!(s, solidcraft_engine::Sel::SketchCurve{id} if id == "c1")));
    assert_eq!(h.app.session.doc.to_json(), baseline);
}

#[test]
fn pixel_annulus_fill_keeps_hole_and_cpu_bridge_matches_cached_boundaries() {
    let mut h = Harness::new();
    let sid = circle(&mut h);
    h.app.run("sketch.circle.center", json!({"center":[0,0],"radius":0.5})).unwrap();
    h.settle();
    let baseline = h.app.session.doc.to_json();
    let state = h.app.session.world_state();
    let ss = state.sketch(sid).unwrap();
    let proj = projection(&h.app, h.app.viewport.rect.unwrap());
    let ppp = h.app.viewport.pixels_per_point();
    let profiles = solidcraft_engine::sketch::find_profiles(&ss.sketch);
    let ring = profiles.iter().find(|p| p.region.holes.len() == 1).unwrap();
    let display = crate::sketch_display::display_region(&ring.region, &ss.plane, &proj, ppp).unwrap();
    assert_eq!(display.holes.len(), 1);
    let triangles = crate::sketch_display::region_triangles(&ring.region, &ss.plane, &proj, ppp);
    assert!(!triangles.is_empty());
    let mut area = 0.0;
    let center = proj.to_screen(ss.plane.origin).unwrap();
    let inner_px = proj.to_screen(ss.plane.to_world(Vec2::new(0.5, 0.0))).unwrap().distance(center);
    for t in &triangles {
        let cross = |a: Vec2, b: Vec2| a.x * b.y - a.y * b.x;
        area += cross(t[1] - t[0], t[2] - t[0]).abs() * 0.5;
        // No triangle can contain the hole centre; every edge remains outside the
        // exact inner circle up to the advertised physical display error.
        let signs = [cross(t[1] - t[0], -t[0]), cross(t[2] - t[1], -t[1]), cross(t[0] - t[2], -t[2])];
        assert!(!(signs.iter().all(|s| *s >= 0.0) || signs.iter().all(|s| *s <= 0.0)));
        for i in 0..3 {
            let a = proj.to_screen(ss.plane.to_world(t[i])).unwrap();
            let b = proj.to_screen(ss.plane.to_world(t[(i + 1) % 3])).unwrap();
            assert!((f64::from(inner_px) - segment_distance(center, a, b)) * f64::from(ppp) <= 1.0);
        }
    }
    assert!((area - std::f64::consts::PI * 0.75).abs() < 0.02, "adaptive fill area: {area}");
    let cpu = solidcraft_engine::view::sketch_lines_with_sampler(&ss.sketch, &ss.plane, true, &[], |sk, index, plane| {
        crate::sketch_display::curve_parts(sk, index, plane, &proj, ppp)
    });
    let cached: Vec<_> = (0..ss.sketch.curves.len()).flat_map(|index| curve_sample(&h.app, sid, index, &ss.plane, &proj).parts.clone()).collect();
    assert_eq!(
        cpu.iter().map(|(points, _, _)| points.clone()).collect::<Vec<_>>(),
        cached,
        "CPU styling bridge and GPU/picking cache must retain identical boundary parts"
    );
    assert_eq!(h.app.session.doc.to_json(), baseline);
}

#[test]
fn pixel_frame_budget_does_not_cache_unbounded_empty_curve_results() {
    let mut h = Harness::new();
    let sid = circle(&mut h);
    let baseline = h.app.session.doc.to_json();
    let state = h.app.session.world_state();
    let mut plane = state.sketch(sid).unwrap().plane;
    let proj = projection(&h.app, h.app.viewport.rect.unwrap());
    let mut limited = 0;
    for i in 0..5000 {
        // Distinct legal placement keys of one source avoid manufacturing a huge
        // document. Even exhausted samples must not add entries forever.
        plane.origin.x = i as f64 * 1e-7;
        let path = curve_sample(&h.app, sid, 0, &plane, &proj);
        assert!(path.vertex_count <= 4096);
        if path.quality == SamplingQuality::BudgetLimited {
            limited += 1;
        }
    }
    assert!(limited > 0, "fixture must actually exhaust the shared frame budget");
    assert!(h.app.viewport.display.lock().unwrap().len() <= 4096);
    assert_eq!(h.app.session.doc.to_json(), baseline);
}

/// A zoomed-in ring is sampled finely, but each slab of the fill still crosses only its
/// four boundary edges: its fill must fit the hover highlight's budget (#164).
#[test]
fn zoomed_in_ring_fill_fits_the_hover_highlight_budget() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.ui.perspective = true;
    h.app.run("sketch.create", json!({"plane":"XY","name":"Ring"})).unwrap();
    h.app.run("sketch.circle.center", json!({"center":[0,0],"radius":40})).unwrap();
    h.app.run("sketch.circle.center", json!({"center":[0,0],"radius":45})).unwrap();
    h.settle();
    h.step(&json!({"call":"ui.view","params":{"view":"iso"}})).unwrap();
    h.step(&json!({"call":"ui.view","params":{"view":"fit"}})).unwrap();
    h.app.cam.zoom_at(0.15, Some(Vec3::new(42.5, 0.0, 0.0)));
    h.settle();
    let baseline = h.app.session.doc.to_json();
    let sid = h.app.session.active_sketch.unwrap();
    let state = h.app.session.world_state();
    let ss = state.sketch(sid).unwrap();
    let proj = projection(&h.app, h.app.viewport.rect.unwrap());
    let ppp = h.app.viewport.pixels_per_point();
    let ring = ss.profiles.iter().find(|p| p.region.holes.len() == 1).unwrap();
    let display = crate::sketch_display::display_region(&ring.region, &ss.plane, &proj, ppp).unwrap();
    let edges = std::iter::once(&display.outer).chain(&display.holes).map(|l| l.segs.len()).sum::<usize>();
    assert!(edges * edges / 2 > 65_536, "fixture must sample the ring finely enough to exceed the old estimate: {edges} edges");
    let triangles = crate::sketch_display::region_triangles(&ring.region, &ss.plane, &proj, ppp);
    assert!(!triangles.is_empty(), "the ring's fill was dropped at this zoom");
    for t in &triangles {
        let r = ((t[0] + t[1] + t[2]) / 3.0).len();
        assert!((39.9..=45.1).contains(&r), "fill triangle outside the ring: r = {r}");
    }
    assert_eq!(h.app.session.doc.to_json(), baseline);
}
