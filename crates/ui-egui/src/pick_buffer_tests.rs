use egui::{Rect, pos2};
use serde_json::json;
use solidcraft_engine::geom::Vec3;
use solidcraft_gpu::pick::{PickTile, TILE, tile_around};

use super::*;
use crate::viewport::{Hit, pick, projection};

fn app() -> SolveApp {
    let mut s = solidcraft_engine::Session::default();
    s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20})).unwrap();
    s.execute("solid.fillet", &json!({"edges": [[40, 15, 20]], "radius": 4})).unwrap();
    s.execute("solid.cylinder", &json!({"radius": 8, "height": 35, "base": [10, 10, 0], "operation": "new_body"})).unwrap();
    let mut app = SolveApp::new(s, crate::Services::default());
    app.set_origin_shown(false);
    app.cam.target = Vec3::new(20.0, 15.0, 12.0);
    app.cam.distance = 110.0;
    // A GPU viewport (no device is needed to hand the picker a tile).
    app.viewport.gpu = Some(crate::gpu::GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess: false });
    app
}

const RECT: Rect = Rect { min: pos2(0.0, 0.0), max: pos2(640.0, 480.0) };

/// Installs the pass's scene, as `paint` makes it, and returns the view's key.
fn install_scene(app: &SolveApp, proj: &Proj) -> u64 {
    let st = app.session.world_state();
    let model = model_key(app, &st);
    let s = scene(app, &st, model).unwrap();
    *app.viewport.pick_buffer.scene.lock().unwrap() = Some(s);
    view_key(app, proj, model)
}

/// A tile only narrows the edges: the edge it names near the cursor is picked as the CPU picks
/// it, an edge it does not name (hidden there) is not, and without a tile the CPU indexes answer
/// and a tile is asked for.
#[test]
fn a_tile_names_the_edges_the_picker_tests() {
    let app = app();
    let proj = projection(&app, RECT);
    // A point on the box's top front edge (y = 0, z = 20).
    let at = proj.to_screen(Vec3::new(20.0, 0.0, 20.0)).unwrap();
    let cpu = pick(&app, &proj, at);
    let Some(Hit::Edge { index, body, .. }) = cpu.iter().find(|h| matches!(h, Hit::Edge { .. })).cloned() else { panic!("{cpu:?}") };
    assert!(app.viewport.pick_buffer.want.lock().unwrap().is_some(), "no tile yet: one is asked for");
    let key = install_scene(&app, &proj);
    let st = app.session.world_state();
    let bi = st.bodies.iter().position(|b| b.name == body).unwrap();
    let s = app.viewport.pick_buffer.scene.lock().unwrap().clone().unwrap();
    let id = s.edges.iter().position(|&(b, e)| b as usize == bi && e as usize == index).unwrap() as u32 + 1;
    let size = view_size(&app, &proj);
    let (origin, tsize) = tile_around([at.x as u32, at.y as u32], size);
    let tile = |ids: Vec<u32>| PickTile { key, origin, size: tsize, ids };
    let n = (tsize[0] * tsize[1]) as usize;
    // The edge's id at the cursor's pixel.
    let mut ids = vec![0; n];
    ids[((at.y as u32 - origin[1]) * tsize[0] + (at.x as u32 - origin[0])) as usize] = id;
    app.viewport.pick_buffer.out.lock().unwrap().tiles = vec![tile(ids)];
    assert_eq!(pick(&app, &proj, at), cpu, "the named edge is picked as the CPU picks it");
    // A tile naming no edge there: no edge or vertex is picked.
    app.viewport.pick_buffer.out.lock().unwrap().tiles = vec![tile(vec![0; n])];
    let none = pick(&app, &proj, at);
    assert!(!none.iter().any(|h| matches!(h, Hit::Edge { .. } | Hit::Vertex { .. })), "{none:?}");
    assert!(none.iter().any(|h| matches!(h, Hit::Face { .. })), "faces are still picked: {none:?}");
    // A tile of another view does not answer.
    app.viewport.pick_buffer.out.lock().unwrap().tiles = vec![PickTile { key: key ^ 1, ..tile(vec![0; n]) }];
    assert_eq!(pick(&app, &proj, at), cpu);
    // Wireframe: nothing hides an edge, and the buffer is not used.
    let mut wire = app;
    wire.ui.visual_style = 2;
    wire.viewport.pick_buffer.out.lock().unwrap().tiles = vec![tile(vec![0; n])];
    assert!(edges_near(&wire, &proj, at, &wire.session.world_state()).is_none());
}

/// On a real device: the pass drawn for the view, and picks over a grid of the view helped by its
/// tiles answer as the CPU picks (an edge's pixels and the CPU's visibility test can disagree by
/// a pixel at a silhouette, so a few may differ). Skipped without a GPU adapter.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn picks_through_the_pass_match_the_cpu() {
    let Some((_, device, queue)) = crate::gpu::offscreen_device() else {
        assert!(std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none(), "a GPU is required here");
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let mut app = app();
    for (perspective, yaw, pitch) in [(false, 0.6, 0.5), (true, 2.4, 0.3)] {
        app.ui.perspective = perspective;
        app.cam.yaw = yaw;
        app.cam.pitch = pitch;
        let proj = projection(&app, RECT);
        let key = install_scene(&app, &proj);
        let s = app.viewport.pick_buffer.scene.lock().unwrap().clone().unwrap();
        let e = proj.cam.eye().to_f32();
        let view = PickView {
            key,
            view_proj: proj.vp.to_f32(),
            eye: [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }],
            back: proj.cam.back().to_f32(),
            lift: lift(&app) as f32,
            size: view_size(&app, &proj),
        };
        let mut pb = PickBuffer::new(&device);
        let out = app.viewport.pick_buffer.out.clone();
        // Tiles over the whole view.
        let mut wanted = Vec::new();
        for ty in (0..view.size[1]).step_by(TILE as usize - 32) {
            for tx in (0..view.size[0]).step_by(TILE as usize - 32) {
                wanted.push([tx + TILE / 2, ty + TILE / 2]);
            }
        }
        let mut tiles = Vec::new();
        for c in wanted {
            for _ in 0..6 {
                let mut encoder = device.create_command_encoder(&Default::default());
                pb.prepare(&device, &queue, &mut encoder, s.key, &s.scene, &view, Some(c), &out);
                queue.submit([encoder.finish()]);
                let _ = device.poll(egui_wgpu::wgpu::PollType::wait_indefinitely());
            }
            tiles.append(&mut out.lock().unwrap().tiles);
        }
        let (mut same, mut differ, mut edges) = (0, 0, 0);
        for j in 0..46 {
            for i in 0..62 {
                let p = pos2(10.0 + i as f32 * 10.0 + (j % 3) as f32, 10.0 + j as f32 * 10.0);
                let r = 8i64;
                let w = [p.x as i64 - r, p.y as i64 - r, p.x as i64 + r, p.y as i64 + r];
                let Some(t) = tiles.iter().find(|t| t.covers(key, view.size, w)).cloned() else { continue };
                out.lock().unwrap().tiles = vec![t];
                let through = pick(&app, &proj, p);
                out.lock().unwrap().tiles.clear();
                let cpu = pick(&app, &proj, p);
                edges += usize::from(cpu.iter().any(|h| matches!(h, Hit::Edge { .. })));
                if through == cpu { same += 1 } else { differ += 1 }
            }
        }
        eprintln!("perspective {perspective}: {same} same, {differ} differ, {edges} with an edge");
        assert!(edges > 100, "the grid reaches edges ({edges})");
        assert!(differ * 100 <= same + differ, "{differ} of {} picks differ", same + differ);
    }
}
