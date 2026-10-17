//! View math shared by the GPU viewport and headless rendering: an orbit camera with
//! perspective or orthographic projection, screen ↔ world mapping and picking rays, plus a small
//! CPU rasterizer (z-buffer, shaded triangles, depth-tested lines) for snapshots without a GPU.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod analysis;
pub mod anisotropic_surface;
mod backend;
mod camera;
mod curve_display;
mod gif;
pub mod job;
pub mod lighting;
mod product;
mod raster;
#[cfg(feature = "raytrace-cpu")]
pub mod raytrace;
mod scene;
pub mod studio;
pub mod surface;
pub mod texture_input;
pub mod texture_interpolation;
pub mod texture_lod;
pub mod texture_sampling;

pub use analysis::{AnalysisColors, AnalysisMode, SceneAnalysis};
pub use backend::{
    Availability, BackendDescriptor, BackendId, ExecutorDescriptor, ExecutorId, MAX_RENDER_ID_BYTES, RenderFeature, RenderFormat, RenderLimits,
    RenderMode, RenderOperation, RenderPolicy, RenderProfile, RenderRequest, ResolveError, ResolvedRequest, ScratchLayout, builtin_backend,
    resolve_request,
};
pub use camera::{Camera, CameraAnim, Mat4, Quat, StandardView, UpAxis, ease_in_out};
pub use curve_display::{PixelCurveView, SampledPath, SamplingBudget, SamplingQuality, sample_segment, sample_segment_unclipped};
pub use gif::gif;
pub use product::{FaceAppearance, ProductMesh, ProductScene};
pub use raster::{Canvas, render, render_png, render_transparent};
pub use scene::{Rgb, SECTION_CAP, Scene, SceneLine, SceneMesh, png};
