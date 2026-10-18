//! The GPU viewport in egui: the viewport's pipelines ([`solidcraft_gpu::view`]) drawn in an
//! `egui_wgpu` paint callback. Everything that draws lives in the GPU crate, so headless pictures
//! (`solidcraft-cli snapshot`, MCP screenshots, `ui.render`) use the same pipelines; this is the
//! glue that hands them egui's device and render pass.

#[cfg(all(test, not(target_arch = "wasm32")))]
use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;
pub use solidcraft_gpu::view::{
    AccessMap, BlockBytes, Frame, GpuImage, GpuScene, GpuTarget, LINE_SIZE, PBR_SIZE, PlannedBody, Renderer, SceneBlock, SceneSlot, TRI_SIZE,
    supersampled_size,
};

/// A frame of the viewport as an egui paint callback.
pub struct ViewportCallback(pub Frame);

/// Create the pipelines. Call once with the app's render state and its depth/stencil/MSAA
/// settings. Without a stencil the section cap does not hide the inside of cut bodies. Without
/// `gpu_meshing` (safe mode) bodies are meshed on the CPU.
pub fn install(rs: &egui_wgpu::RenderState, depth_bits: u8, stencil_bits: u8, samples: u32, gpu_meshing: bool) -> GpuTarget {
    let tess = if gpu_meshing { solidcraft_gpu::view::tessellation(&rs.adapter, &rs.device, &rs.queue, solidcraft_gpu::SHADER) } else { None };
    let t = GpuTarget { format: rs.target_format, depth: egui_wgpu::depth_format_from_bits(depth_bits, stencil_bits), samples, tess: tess.is_some() };
    let res = Renderer::new(&rs.device, t, tess);
    rs.renderer.write().callback_resources.insert(res);
    t
}

/// Draw this process's headless pictures (`ui.render`, the agent's view image, thumbnails of
/// designs and saves) on the app's own device, with the viewport's pipelines (native only: a
/// browser cannot wait for the GPU to hand a picture back); where the device cannot, the CPU
/// rasterizer draws them.
#[cfg(not(target_arch = "wasm32"))]
pub fn install_picture_renderer(rs: &egui_wgpu::RenderState) {
    match solidcraft_gpu::capture::Capturer::new(&rs.adapter, rs.device.clone(), rs.queue.clone()) {
        Ok(capturer) => {
            let capturer = std::sync::Arc::new(capturer.with_queue_access(rs.queue_access.clone()));
            solidcraft_engine::capture::install(std::sync::Arc::new(move |scene, cam, w, h| {
                let rgba = capturer.rgba(scene, cam, w, h).map_err(|e| e.to_string())?;
                Ok(solidcraft_engine::capture::Picture { rgba, renderer: capturer.renderer() })
            }));
        }
        Err(e) => {
            log::warn!("pictures of the model are drawn by the CPU rasterizer: the GPU cannot draw them: {e}");
            install_cpu_picture_renderer();
        }
    }
}

/// The last tier: draw this process's headless pictures with the CPU rasterizer, and every
/// picture says so. The app uses it when its device cannot draw pictures; the web app always
/// does, since a browser cannot wait for the GPU to hand a picture back.
pub fn install_cpu_picture_renderer() {
    solidcraft_engine::capture::install(std::sync::Arc::new(|scene, cam, w, h| {
        let canvas = solidcraft_engine::render::render(scene, cam, w as usize, h as usize);
        Ok(solidcraft_engine::capture::Picture { rgba: canvas.rgba, renderer: "cpu" })
    }));
}

#[cfg(test)]
mod test_support {
    /// For tests that look at pictures: install the headless GPU renderer (the CLI's). `false`, with
    /// the reason printed, on a machine without a GPU adapter, and the test skips.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn test_picture_renderer() -> bool {
        match solidcraft_gpu::capture::shared() {
            Ok(c) => {
                solidcraft_engine::capture::install(std::sync::Arc::new(move |scene, cam, w, h| {
                    c.rgba(scene, cam, w, h)
                        .map(|rgba| solidcraft_engine::capture::Picture { rgba, renderer: c.renderer() })
                        .map_err(|e| e.to_string())
                }));
                true
            }
            Err(e @ solidcraft_gpu::GpuError::NoAdapter(_)) if std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none() => {
                eprintln!("skipping: {e}");
                false
            }
            Err(e) => panic!("{e}"),
        }
    }
}
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) use test_support::test_picture_renderer;

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get_mut::<Renderer>() {
            res.prepare(device, queue, encoder, &self.0);
        }
        Vec::new()
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(res) = resources.get::<Renderer>() else { return };
        let vp = info.viewport_in_pixels();
        res.paint(&self.0, pass, [vp.left_px as f32, vp.top_px as f32, vp.width_px as f32, vp.height_px as f32]);
    }
}

/// The shader's analysis colours and flags for the theme's colours.
pub fn analysis_colors(c: &crate::theme::AnalysisColors) -> [f32; 36] {
    solidcraft_gpu::view::analysis_colors(c)
}

/// The backends the offscreen tests start, chosen once per test process as the app chooses them
/// (`apps/solidcraft/src/startup.rs`, `choose_backends`): what `WGPU_BACKEND` names; on Windows one
/// backend, the first of D3D12, Vulkan and OpenGL with a hardware adapter (else D3D12); elsewhere
/// wgpu's default. wgpu's default on Windows starts Vulkan, D3D12 and OpenGL together, and tests
/// starting them side by side hung or crashed the test process inside the Vulkan driver.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn offscreen_backends() -> wgpu::Backends {
    static CHOSEN: std::sync::OnceLock<wgpu::Backends> = std::sync::OnceLock::new();
    *CHOSEN.get_or_init(|| {
        let default = wgpu::InstanceDescriptor::new_without_display_handle_from_env().backends;
        if !cfg!(target_os = "windows") || std::env::var("WGPU_BACKEND").is_ok_and(|v| !v.trim().is_empty()) {
            return default;
        }
        let hardware = |backends: wgpu::Backends| {
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
            desc.backends = backends;
            let adapters = pollster::block_on(wgpu::Instance::new(desc).enumerate_adapters(backends));
            adapters.iter().any(|a| a.get_info().device_type != wgpu::DeviceType::Cpu)
        };
        [wgpu::Backends::DX12, wgpu::Backends::VULKAN, wgpu::Backends::GL].into_iter().find(|b| hardware(*b)).unwrap_or(wgpu::Backends::DX12)
    })
}

/// A device of its own for an offscreen render or check (`None` without a GPU adapter), on
/// [`offscreen_backends`].
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn offscreen_device() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    desc.backends = offscreen_backends();
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: solidcraft_gpu::depth_features(&adapter),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .ok()?;
    Some((adapter, device, queue))
}

/// Whether [`install`] would evaluate plans on the GPU with `shader`, on an offscreen device
/// (`None` without a GPU adapter).
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn offscreen_tessellates(shader: &str) -> Option<bool> {
    let (adapter, device, queue) = offscreen_device()?;
    Some(solidcraft_gpu::view::tessellation(&adapter, &device, &queue, shader).is_some())
}

/// Render `scene` offscreen on a device of its own, through the viewport's real resources and
/// frame callback (`prepare`, then `paint`), and read the pixels back: straight RGBA, `size` ×
/// `size`. `None` without a GPU adapter. With `tess` (the tessellation shader), planned bodies
/// are evaluated on the GPU where the device builds that pipeline, as [`install`] sets it up.
/// `highlight` is drawn as the viewport's highlight scene (hover and selection). `analysis` is the
/// surface analysis uniform (`viewport::analysis_uniform`; zeros for none) and `colors` its
/// colours (`analysis_colors`).
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_offscreen(
    scene: GpuScene,
    highlight: Option<GpuScene>,
    view_proj: [[f32; 4]; 4],
    back: [f32; 3],
    eye: [f32; 4],
    size: u32,
    tess: Option<&str>,
    analysis: [f32; 8],
    colors: [f32; 36],
) -> Option<Vec<u8>> {
    offscreen(
        scene,
        highlight,
        view_proj,
        back,
        eye,
        size,
        Offscreen { tess, product: false, srgb: false, analysis, analysis_colors: colors, ssaa: 1.0 },
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn render_product_offscreen(
    scene: GpuScene,
    view_proj: [[f32; 4]; 4],
    back: [f32; 3],
    eye: [f32; 4],
    size: u32,
    srgb: bool,
) -> Option<Vec<u8>> {
    offscreen(
        scene,
        None,
        view_proj,
        back,
        eye,
        size,
        Offscreen { tess: None, product: true, srgb, analysis: [0.0; 8], analysis_colors: [0.0; 36], ssaa: 1.0 },
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
struct Offscreen<'a> {
    tess: Option<&'a str>,
    product: bool,
    srgb: bool,
    analysis: [f32; 8],
    analysis_colors: [f32; 36],
    ssaa: f32,
}

/// [`render_offscreen`] of a scene supersampled `ssaa` times (1: off).
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn render_offscreen_ssaa(
    scene: GpuScene,
    view_proj: [[f32; 4]; 4],
    back: [f32; 3],
    eye: [f32; 4],
    size: u32,
    ssaa: f32,
) -> Option<Vec<u8>> {
    offscreen(
        scene,
        None,
        view_proj,
        back,
        eye,
        size,
        Offscreen { tess: None, product: false, srgb: false, analysis: [0.0; 8], analysis_colors: [0.0; 36], ssaa },
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
fn offscreen(
    scene: GpuScene,
    highlight: Option<GpuScene>,
    view_proj: [[f32; 4]; 4],
    back: [f32; 3],
    eye: [f32; 4],
    size: u32,
    options: Offscreen<'_>,
) -> Option<Vec<u8>> {
    let Offscreen { tess, product, srgb, analysis, analysis_colors, ssaa } = options;
    let (adapter, device, queue) = offscreen_device()?;
    eprintln!("offscreen adapter: {:?}", adapter.get_info().backend);
    let format = if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
    let depth = solidcraft_gpu::depth_format(&device, true);
    let compute = tess.and_then(|shader| solidcraft_gpu::view::tessellation(&adapter, &device, &queue, shader));
    let target = GpuTarget { format, depth: Some(depth), samples: 1, tess: compute.is_some() };
    let mut resources = egui_wgpu::CallbackResources::default();
    resources.insert(Renderer::new(&device, target, compute));
    let none = || Arc::new(Mutex::new(None));
    let cb = ViewportCallback(Frame {
        product,
        key: 1,
        slot: Arc::new(Mutex::new(Some(scene))),
        sketch_key: 0,
        sketch_slot: none(),
        hl_key: u64::from(highlight.is_some()),
        hl_slot: Arc::new(Mutex::new(highlight)),
        pv_key: 0,
        pv_slot: none(),
        view_proj,
        back,
        size_px: [size as f32; 2],
        clip: [0.0; 4],
        cap: [0.0; 4],
        images: Vec::new(),
        analysis,
        analysis_colors,
        access: None,
        outline: false,
        eye,
        ssaa,
    });
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = texture(format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth_tex = texture(depth, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let (color_view, depth_view) = (color.create_view(&Default::default()), depth_tex.create_view(&Default::default()));
    let mut encoder = device.create_command_encoder(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [size, size], pixels_per_point: 1.0 };
    let extra = egui_wgpu::CallbackTrait::prepare(&cb, &device, &queue, &screen, &mut encoder, &mut resources);
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("offscreen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size as f32, size as f32));
        let info = egui::PaintCallbackInfo { viewport: rect, clip_rect: rect, pixels_per_point: 1.0, screen_size_px: [size, size] };
        egui_wgpu::CallbackTrait::paint(&cb, info, &mut pass, &resources);
    }
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offscreen read"),
        size: u64::from(size * size * 4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &read,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: Some(size) },
        },
        wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
    );
    queue.submit(extra.into_iter().chain([encoder.finish()]));
    let slice = read.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let bytes = slice.get_mapped_range().ok()?.to_vec();
    Some(bytes)
}

#[cfg(test)]
mod tests {
    /// The offscreen tests start the backends the app would: on Windows without `WGPU_BACKEND`
    /// exactly one, from the app's order (D3D12, Vulkan, OpenGL); otherwise wgpu's default.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn offscreen_tests_start_the_backend_the_app_would() {
        use egui_wgpu::wgpu::Backends;
        let chosen = super::offscreen_backends();
        let env = std::env::var("WGPU_BACKEND").is_ok_and(|v| !v.trim().is_empty());
        if cfg!(target_os = "windows") && !env {
            assert!([Backends::DX12, Backends::VULKAN, Backends::GL].contains(&chosen), "one backend, not {chosen:?}");
        } else {
            assert_eq!(chosen, egui_wgpu::wgpu::InstanceDescriptor::new_without_display_handle_from_env().backends);
        }
        assert_eq!(super::offscreen_backends(), chosen, "chosen once per test process");
        // And the offscreen device is made on them (skipped without a GPU adapter).
        if let Some((adapter, _, _)) = super::offscreen_device() {
            let used = Backends::from(adapter.get_info().backend);
            assert!(chosen.contains(used), "the offscreen device runs on {used:?}, not on {chosen:?}");
        }
    }
}
