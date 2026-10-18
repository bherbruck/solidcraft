//! Integrated product preview. Shares the installed device/pipelines, not Design's scene policy.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use egui::{Color32, Sense};
use serde_json::json;
use solidcraft_engine::doc::render_settings::RenderSetup;
use solidcraft_engine::render::{
    Availability, Camera, ExecutorDescriptor, ExecutorId, ProductScene, RenderLimits, RenderMode, RenderOperation, RenderProfile, RenderRequest,
    ScratchLayout,
};

use crate::gpu::{Frame, GpuScene, SceneSlot, ViewportCallback};
use crate::{SolveApp, theme::Tokens};

const MAX_GEOMETRY_BYTES: usize = 128 * 1024 * 1024;

#[cfg(test)]
#[path = "render_workspace_tests.rs"]
mod tests;

pub struct State {
    pub host: Option<ExecutorDescriptor>,
    pub image_host: Option<solidcraft_engine::render::job::CpuHost>,
    pub view: ViewState,
    revision: u64,
    design_camera: Option<(Camera, bool)>,
    slot: SceneSlot,
    key: Option<u64>,
    scene_signature: Option<(u64, usize, u64)>,
    scene: Option<ProductScene>,
}

/// Small per-document editor state; GPU uploads and host capabilities stay outside tabs.
#[derive(Clone, Debug)]
pub struct ViewState {
    pub name: String,
    draft: RenderSetup,
    dirty: bool,
    job: Option<solidcraft_engine::render::job::JobId>,
    completed: Option<solidcraft_engine::render::job::JobId>,
    transparent: bool,
    overwrite: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self { name: "Studio".into(), draft: RenderSetup::default(), dirty: false, job: None, completed: None, transparent: false, overwrite: false }
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            host: None,
            image_host: None,
            view: Default::default(),
            revision: u64::MAX,
            design_camera: None,
            slot: Arc::new(Mutex::new(None)),
            key: None,
            scene_signature: None,
            scene: None,
        }
    }
}

pub fn active(app: &SolveApp) -> bool {
    app.ui.tab == "RENDER"
}

pub fn host_executor(max_dimension: u32, depth: bool, samples: u32) -> Result<ExecutorDescriptor, String> {
    if !matches!(samples, 1 | 2 | 4 | 8 | 16) {
        return Err("unsupported GPU sample count".into());
    }
    Ok(ExecutorDescriptor {
        id: ExecutorId::new("gpu-viewport").map_err(|e| e.to_string())?,
        availability: if depth && max_dimension > 0 {
            Availability::Available
        } else {
            Availability::Unavailable { reason: "product preview requires an installed device with a depth buffer".into() }
        },
        profiles: vec![RenderProfile {
            mode: RenderMode::Preview,
            operation: RenderOperation::Viewport,
            formats: Vec::new(),
            features: vec![solidcraft_engine::render::RenderFeature::Pbr],
        }],
        limits: RenderLimits { max_dimension: u64::from(max_dimension.clamp(1, 4096)), ..Default::default() },
        scratch: ScratchLayout {
            sample_scale: 1,
            sample_bytes_per_pixel: 8 * u64::from(samples),
            output_bytes_per_pixel: 8,
            encoding_bytes_per_pixel: 0,
        },
    })
}

pub fn sync_backend(app: &mut SolveApp) {
    if app.session.render_job_host().is_none()
        && let Some(host) = &app.render_workspace.image_host
        && let Err(e) = app.session.set_render_job_host(Some(host.clone()))
    {
        app.set_status(e.to_string(), true);
    }
    if app.session.render_viewport() == app.render_workspace.host.as_ref() {
        return;
    }
    if let Err(e) = app.session.set_render_viewport(app.render_workspace.host.clone()) {
        app.set_status(e.to_string(), true);
    }
}

/// Cooperative work belongs to the frame loop, never to a status/control request. Keep
/// scheduling frames even after leaving Render so a browser job can finish in Design.
pub fn tick(app: &mut SolveApp, ctx: &egui::Context) {
    if let Some(host) = &app.render_workspace.image_host {
        match host.pump(1024) {
            Ok(true) => ctx.request_repaint(),
            Ok(false) => {}
            Err(e) => app.set_status(e.to_string(), true),
        }
    }
}

pub fn reset_document(app: &mut SolveApp) {
    app.ui.ribbon_context = None;
    let host = app.render_workspace.host.clone();
    let image_host = app.render_workspace.image_host.clone();
    app.render_workspace = State { host, image_host, ..Default::default() };
    if active(app) {
        app.ui.tab = "MODEL".into();
    }
    sync_backend(app);
}

pub fn invalidate_scene(app: &mut SolveApp) {
    app.render_workspace.revision = u64::MAX;
    app.render_workspace.scene_signature = None;
    app.render_workspace.scene = None;
    app.render_workspace.key = None;
}

pub fn enter(app: &mut SolveApp, render: bool) -> Result<(), String> {
    if render {
        if app.session.active_sketch.is_some() || app.tool.is_some() || app.dialog.is_some() || app.file_load.busy() {
            return Err("finish the current modelling operation before entering Render".into());
        }
        app.run("render.scene", json!({}))?;
        if !active(app) {
            app.render_workspace.design_camera = Some((app.cam, app.ui.perspective));
            app.ui.tab = "RENDER".into();
            let scene = solidcraft_engine::product_scene::extract(&app.session).map_err(|e| e.to_string())?;
            if !scene.bounds.is_empty() {
                app.cam.fit(&scene.bounds);
            }
        }
    } else {
        if let Some((camera, perspective)) = app.render_workspace.design_camera.take() {
            app.cam = camera;
            app.ui.up_axis = camera.up;
            app.ui.perspective = perspective;
        }
        app.ui.tab = "MODEL".into();
    }
    app.cancel_view_animation();
    Ok(())
}

pub fn panel(app: &mut SolveApp, ui: &mut egui::Ui) {
    if app.render_workspace.revision != app.session.revision && !app.render_workspace.view.dirty {
        app.render_workspace.view.draft = app.session.doc.render.setups.get(&app.render_workspace.view.name).cloned().unwrap_or_default();
        app.render_workspace.revision = app.session.revision;
    }
    egui::Panel::right("sc_render_settings")
        .exact_size(285.0)
        .resizable(false)
        .frame(egui::Frame::NONE.fill(Tokens::get().panel).inner_margin(10))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading(app.tr("cmd.render_settings_get"));
                ui.label(app.tr("render.preview"));
                ui.small(app.tr("render.opaque_preview_help"));
                ui.separator();
                let names: Vec<_> = app.session.doc.render.setups.keys().cloned().collect();
                let old_name = app.render_workspace.view.name.clone();
                egui::ComboBox::from_id_salt("render_setups").selected_text(&old_name).show_ui(ui, |ui| {
                    for name in names {
                        ui.selectable_value(&mut app.render_workspace.view.name, name.clone(), name);
                    }
                });
                ui.label(app.tr("render.setup_name"));
                ui.add(egui::TextEdit::singleline(&mut app.render_workspace.view.name).char_limit(128));
                if app.render_workspace.view.name != old_name {
                    app.render_workspace.view.draft = app.session.doc.render.setups.get(&app.render_workspace.view.name).cloned().unwrap_or_default();
                    app.render_workspace.view.dirty = false;
                }
                ui.separator();
                ui.label(app.tr("render.background"));
                if ui.color_edit_button_srgb(&mut app.render_workspace.view.draft.background).changed() {
                    app.render_workspace.view.dirty = true;
                }
                let mut perspective = app.cam.fov > 0.0;
                if ui.checkbox(&mut perspective, app.tr("render.perspective")).changed() {
                    app.cam.fov = if perspective { std::f64::consts::FRAC_PI_4 } else { 0.0 };
                    app.render_workspace.view.dirty = true;
                }
                if ui.button(app.tr("render.fit")).clicked() {
                    match solidcraft_engine::product_scene::extract(&app.session) {
                        Ok(scene) if !scene.bounds.is_empty() => app.cam.fit(&scene.bounds),
                        Err(e) => app.set_status(e.to_string(), true),
                        _ => {}
                    }
                }
                if ui.button(app.tr("render.restore_camera")).clicked() {
                    let name = app.render_workspace.view.name.clone();
                    if app.run("render.settings.apply", json!({"name":name})).is_ok() {
                        app.follow_view_request(false);
                    }
                }
                if ui.button(app.tr("render.save_setup")).clicked() {
                    let name = app.render_workspace.view.name.clone();
                    let setup = RenderSetup { camera: app.cam, ..app.render_workspace.view.draft.clone() };
                    if app.run("render.settings.set", json!({"name":name,"set":setup})).is_ok() {
                        app.render_workspace.view.dirty = false;
                    }
                }
                if app.render_workspace.view.dirty {
                    ui.label(app.tr("render.unsaved"));
                }
                if ui.button(app.tr("render.reload_settings")).clicked() {
                    app.render_workspace.view.dirty = false;
                    app.render_workspace.revision = u64::MAX;
                }
                if app.render_workspace.view.draft.backend.as_str() != "builtin" || app.render_workspace.view.draft.mode != RenderMode::Preview {
                    ui.label(app.tr("render.unavailable_preview"));
                    if ui.button(app.tr("render.use_preview")).clicked() {
                        app.render_workspace.view.draft.backend = solidcraft_engine::render::builtin_backend().id;
                        app.render_workspace.view.draft.mode = RenderMode::Preview;
                        app.render_workspace.view.dirty = true;
                    }
                }
                ui.separator();
                image_controls(app, ui);
                ui.separator();
                ui.small(app.tr("render.navigation_help"));
                if ui.button(app.tr("render.return_design")).clicked()
                    && let Err(e) = enter(app, false)
                {
                    app.set_status(e, true);
                }
            });
        });
}

fn image_controls(app: &mut SolveApp, ui: &mut egui::Ui) {
    ui.heading(app.tr("render.image_title"));
    ui.small(app.tr("render.opaque_image_summary"));
    ui.horizontal(|ui| {
        ui.label(app.tr("render.image_width"));
        if ui.add(egui::DragValue::new(&mut app.render_workspace.view.draft.width).range(1..=4096)).changed() {
            app.render_workspace.view.dirty = true;
        }
        ui.label(app.tr("render.image_height"));
        if ui.add(egui::DragValue::new(&mut app.render_workspace.view.draft.height).range(1..=4096)).changed() {
            app.render_workspace.view.dirty = true;
        }
    });
    let draft = &app.render_workspace.view.draft;
    let size = (u64::from(draft.width), u64::from(draft.height));
    if let Some((w, h)) = RenderLimits::default().fit(size.0, size.1, None)
        && (w, h) != size
    {
        ui.small(app.tr("render.image_too_large"));
        if ui.button(format!("{} {w} × {h}", app.tr("render.image_use_largest"))).clicked() {
            app.render_workspace.view.draft.width = u32::try_from(w).unwrap_or(1);
            app.render_workspace.view.draft.height = u32::try_from(h).unwrap_or(1);
            app.render_workspace.view.dirty = true;
        }
    }
    let transparent_label = app.tr("render.image_transparent").to_string();
    ui.checkbox(&mut app.render_workspace.view.transparent, transparent_label);
    if ui.button(app.tr("render.image_check")).clicked() {
        let settings = RenderSetup { camera: app.cam, ..app.render_workspace.view.draft.clone() };
        if app.run("render.preflight", json!({"settings":settings,"transparent_background":app.render_workspace.view.transparent})).is_ok() {
            app.set_status(app.tr("render.image_supported").to_string(), false);
        }
    }
    let mut busy = false;
    if let Some(job) = app.render_workspace.view.job {
        match app.session.execute("render.status", &json!({"job":job})) {
            Ok(status) => {
                let phase = status["progress"]["phase"].as_str().unwrap_or("unknown");
                busy = matches!(phase, "queued" | "preparing" | "running" | "encoding" | "cancelling");
                ui.label(match phase {
                    "queued" => app.tr("render.image_queued"),
                    "preparing" => app.tr("render.image_preparing"),
                    "running" => app.tr("render.image_running"),
                    "encoding" => app.tr("render.image_encoding"),
                    "cancelling" => app.tr("render.image_cancelling"),
                    "cancelled" => app.tr("render.image_cancelled"),
                    "completed" => app.tr("render.image_completed"),
                    "failed" => app.tr("render.image_failed"),
                    _ => app.tr("render.image_unknown"),
                });
                if phase == "completed" {
                    app.render_workspace.view.completed = Some(job);
                }
                if let Some(message) = status["error"]["message"].as_str() {
                    ui.label(message);
                    if status["error"]["code"] == "limit_exceeded" {
                        ui.small(app.tr("render.image_reduce"));
                    }
                }
                if busy {
                    let done = status["progress"]["done"].as_u64().unwrap_or(0);
                    let total = status["progress"]["total"].as_u64().unwrap_or(1).max(1);
                    ui.add(egui::ProgressBar::new((done as f64 / total as f64).clamp(0.0, 1.0) as f32));
                    ui.ctx().request_repaint();
                    if ui.button(app.tr("render.image_cancel")).clicked() {
                        let _ = app.run("render.cancel", json!({"job":job}));
                    }
                }
                if let Some(revision) = status["scene_generation"].as_u64()
                    && revision != app.session.revision
                {
                    ui.small(app.tr("render.image_stale"));
                }
            }
            Err(e) => {
                ui.label(e.to_string());
            }
        }
    }
    if ui.add_enabled(!busy, egui::Button::new(app.tr("render.image_start"))).clicked() {
        let settings = RenderSetup { camera: app.cam, ..app.render_workspace.view.draft.clone() };
        if let Ok(reply) = app.run("render.start", json!({"settings":settings,"transparent_background":app.render_workspace.view.transparent})) {
            app.render_workspace.view.job = serde_json::from_value(reply["job"].clone()).ok();
            ui.ctx().request_repaint();
        }
    }
    let overwrite_label = app.tr("render.image_overwrite").to_string();
    ui.checkbox(&mut app.render_workspace.view.overwrite, overwrite_label);
    if ui.add_enabled(app.render_workspace.view.completed.is_some(), egui::Button::new(app.tr("render.image_export"))).clicked()
        && let Some(job) = app.render_workspace.view.completed
    {
        export_image(app, job, app.render_workspace.view.overwrite);
    }
}

/// Keep the selected image and overwrite consent fixed while the upstream picker holds model input.
fn export_image(app: &mut SolveApp, job: solidcraft_engine::render::job::JobId, overwrite: bool) {
    let origin = app.session.doc.clone();
    app.pick_save("render.png", &["png"], move |app, path| {
        let Some(path) = path else { return };
        if !Arc::ptr_eq(&origin, &app.session.doc) {
            app.set_status("The render export document changed while choosing its destination", true);
            return;
        }
        let _ = app.run("render.export", json!({"job":job,"path":path,"overwrite":overwrite}));
    });
}

/// Convert only product surfaces; geometry budgets are admitted before vertex allocation.
pub fn gpu_scene(scene: &ProductScene) -> Result<(u64, GpuScene), String> {
    let bytes = scene
        .meshes
        .iter()
        .try_fold(0usize, |n, m| m.mesh.triangles.len().checked_mul(3 * crate::gpu::PBR_SIZE).and_then(|x| n.checked_add(x)))
        .ok_or("product geometry size overflow")?;
    if bytes > MAX_GEOMETRY_BYTES {
        return Err("product preview exceeds the 128 MiB geometry upload budget".into());
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut gpu = GpuScene::default();
    for body in &scene.meshes {
        if body.faces.len() > 262_144 {
            return Err("product preview exceeds the face-appearance admission cap".into());
        }
        if !body.opacity.is_finite()
            || !(0.0..=1.0).contains(&body.opacity)
            || body.faces.iter().any(|f| !f.opacity.is_finite() || !(0.0..=1.0).contains(&f.opacity))
        {
            return Err("invalid product appearance opacity".into());
        }
        (Arc::as_ptr(&body.mesh) as usize).hash(&mut hash);
        body.name.hash(&mut hash);
        body.color.hash(&mut hash);
        body.opacity.to_bits().hash(&mut hash);
        hash_surface(body.surface, &mut hash);
        let looks: std::collections::BTreeMap<_, _> = body.faces.iter().map(|l| (l.face, l)).collect();
        for face in &body.faces {
            face.face.hash(&mut hash);
            face.color.hash(&mut hash);
            face.opacity.to_bits().hash(&mut hash);
            hash_surface(face.surface, &mut hash);
        }
        for (index, triangle) in body.mesh.triangles.iter().enumerate() {
            let face = body.mesh.tri_face.get(index).and_then(|f| looks.get(f));
            let (color, opacity, surface) = face.map_or((body.color, body.opacity, body.surface), |l| (l.color, l.opacity, l.surface));
            if surface.is_some() && opacity != 1.0 {
                return Err("opaque PBR requires opacity 1".into());
            }
            if opacity <= 0.0 {
                continue;
            }
            let vertices = triangle.map(|i| {
                let p = body.mesh.positions.get(i as usize)?.to_f32();
                let n = *body.mesh.normals.get(i as usize)?;
                let n = if surface.is_some() { solidcraft_engine::render::surface::unit_direction(n).ok()? } else { n }.to_f32();
                p.into_iter().chain(n).all(f32::is_finite).then_some((p, n))
            });
            let [Some(a), Some(b), Some(c)] = vertices else { return Err("invalid product triangle".into()) };
            let rgba = [color[0], color[1], color[2], (opacity * 255.0).round() as u8];
            let (buffer, stride) = if surface.is_some() {
                (&mut gpu.pbr, crate::gpu::PBR_SIZE)
            } else if opacity < 0.999 {
                (&mut gpu.glass, crate::gpu::TRI_SIZE)
            } else {
                (&mut gpu.tris, crate::gpu::TRI_SIZE)
            };
            buffer.try_reserve(3 * stride).map_err(|_| "product preview vertex allocation failed".to_string())?;
            for (p, n) in [a, b, c] {
                if let Some(surface) = surface {
                    if n.iter().all(|v| *v == 0.0) {
                        return Err("PBR requires nonzero surface normals".into());
                    }
                    gpu.pbr_tri(p, n, surface);
                } else if opacity < 0.999 {
                    gpu.glass_tri(p, n, rgba);
                } else {
                    gpu.tri(p, n, rgba);
                }
            }
        }
    }
    Ok((hash.finish(), gpu))
}

fn hash_surface(surface: Option<solidcraft_engine::render::surface::OpaqueSurface>, hash: &mut impl Hasher) {
    surface.is_some().hash(hash);
    if let Some(s) = surface {
        let c = s.base_color();
        for value in [c.x, c.y, c.z, s.metallic(), s.roughness(), s.dielectric_ior()] {
            value.to_bits().hash(hash);
        }
    }
}

/// The size the product preview draws at for a viewport of `full` physical pixels: the full
/// size when the viewport executor admits it, else the largest size at the same aspect it does.
/// Without an executor the request is left for resolution to report.
pub fn preview_size(executor: Option<&ExecutorDescriptor>, full: (u64, u64)) -> (u64, u64) {
    executor.and_then(|e| e.fit(full.0, full.1)).unwrap_or(full)
}

pub fn show(app: &mut SolveApp, ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    app.viewport.rect = Some(rect);
    crate::viewport::refresh_layout(app, ui.ctx().pixels_per_point());
    let response = ui.interact(rect, ui.id().with("render_preview"), Sense::click_and_drag());
    // The modelling viewport's camera input (preset, Orbit around, zoom to the cursor, middle
    // double-click), with nothing to pick or select here.
    crate::navigation::handle(app, ui, rect, &response, crate::navigation::NavCaps::RENDER);
    let bg = app.render_workspace.view.draft.background;
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(bg[0], bg[1], bg[2]));
    let result = (|| -> Result<(), String> {
        if app.viewport.gpu.is_none() {
            return Err("GPU product preview is unavailable on this host. Saved setups remain available.".into());
        }
        let ppp = ui.ctx().pixels_per_point();
        let setup = &app.render_workspace.view.draft;
        if setup.exposure_ev != 0.0 || setup.environment_rotation != 0.0 || setup.environment_intensity != 1.0 {
            return Err("fixed studio preview requires exposure 0, environment rotation 0 and intensity 1".into());
        }
        // A 4K or 5K viewport is past the preview's pixel budget: draw it smaller and filter it
        // up into the viewport rather than refusing the frame.
        let full = ((rect.width() * ppp).max(1.0) as u64, (rect.height() * ppp).max(1.0) as u64);
        let (width, height) = preview_size(app.session.render_viewport(), full);
        let mut request = RenderRequest {
            backend: Some(setup.backend.clone()),
            mode: setup.mode,
            operation: RenderOperation::Viewport,
            width,
            height,
            format: None,
            features: Vec::new(),
            policy: Default::default(),
        };
        solidcraft_engine::render::resolve_request(&[app.session.render_backend()], &request).map_err(|e| e.to_string())?;
        if app.session.model.is_stale() {
            return Err("finish the modelling gesture before previewing".into());
        }
        let mut visibility = std::collections::hash_map::DefaultHasher::new();
        app.session.visibility.hidden_bodies.hash(&mut visibility);
        app.session.visibility.hidden_occurrences.hash(&mut visibility);
        let signature = (app.session.revision, Arc::as_ptr(&app.session.world_state()) as usize, visibility.finish());
        if app.render_workspace.scene_signature != Some(signature) {
            let scene = solidcraft_engine::product_scene::extract(&app.session).map_err(|e| e.to_string())?;
            let (key, gpu) = gpu_scene(&scene)?;
            if app.render_workspace.key != Some(key) {
                if let Ok(mut slot) = app.render_workspace.slot.lock() {
                    *slot = Some(gpu);
                }
                app.render_workspace.key = Some(key);
            }
            app.render_workspace.scene = Some(scene);
            app.render_workspace.scene_signature = Some(signature);
        }
        let scene = app.render_workspace.scene.as_ref().ok_or("no product scene")?;
        request.features = scene.required_features();
        solidcraft_engine::render::resolve_request(&[app.session.render_backend()], &request).map_err(|e| e.to_string())?;
        let key = app.render_workspace.key.ok_or("no product geometry")?;
        if scene.bounds.is_empty() {
            return Err("Create a visible solid in Design to preview it here.".into());
        }
        let camera = app.cam;
        let radius = scene.bounds.diagonal().max(10.0) + scene.bounds.center().dist(camera.target);
        let eye = camera.eye();
        let slot = app.render_workspace.slot.clone();
        let callback = ViewportCallback(Frame {
            product: true,
            key,
            slot: slot.clone(),
            sketch_key: 0,
            sketch_slot: slot.clone(),
            hl_key: 0,
            hl_slot: slot.clone(),
            pv_key: 0,
            pv_slot: slot,
            view_proj: camera.view_proj(f64::from(rect.width() / rect.height().max(1.0)), radius).to_f32(),
            back: camera.back().to_f32(),
            size_px: [rect.width() * ppp, rect.height() * ppp],
            clip: [0.0; 4],
            cap: [0.0; 4],
            images: Vec::new(),
            analysis: [0.0; 8],
            analysis_colors: [0.0; 36],
            access: None,
            outline: false,
            eye: [eye.x as f32, eye.y as f32, eye.z as f32, if camera.fov > 0.0 { 1.0 } else { 0.0 }],
            ssaa: if width < full.0 { width as f32 / full.0 as f32 } else { 1.0 },
        });
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));
        Ok(())
    })();
    if let Err(error) = result {
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, error, egui::FontId::proportional(14.0), Tokens::get().text);
    }
    crate::viewport::axis_view(app, ui, rect);
}
