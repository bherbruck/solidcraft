//! The SolidCraft document: user and model parameters, and a parametric timeline of features
//! (sketches, extrudes, revolves, fillets, primitives, combines…). Evaluating the timeline with
//! the kernel produces the model (bodies and solved sketches). Evaluation is incremental: a
//! feature is recomputed only when it, a parameter it uses, or an earlier feature changed.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod appearance;
mod assembly;
mod body_transfer;
mod body_transfer_sheet;
pub mod pbr_appearance;
pub use body_transfer::{valid_transfer_matrix, validate_body_transfer};
pub mod canvas;
mod clipboard;
pub use clipboard::{clone_expression, remap_topology};
pub mod config;
pub mod construct;
pub mod display_quality;
pub mod display_units;
mod document;
mod eval;
pub mod expr;
pub mod face_anchor;
mod feature_geometry;
mod feature_refs;
pub use face_anchor::{FaceAnchor, FaceAttachmentStatus};
pub mod format;
mod format_offsets;
mod history_names;
pub use format_offsets::whole_profile_selection_from_value;
pub mod joints;
mod lineage;
pub mod metadata;
pub use metadata::Metadata;
pub mod motion;
mod name_lookup;
mod names;
pub mod naming;
mod params;
pub mod physical_material;
mod piece_names;
mod points;
mod profile_refs;
pub mod progress;
mod walk;
pub use profile_refs::{capture_profile_selection, profile_selection_indices};
pub mod plastic;
pub mod project;
mod project3d;
pub mod render_settings;
pub use project3d::iso_grid;
pub mod sheet;

pub use assembly::{
    IDENTITY, MAX_PLACEMENT_DEPTH, MAX_PLACEMENTS, Mat, Occurrence, apply_point, apply_vector, is_identity, map_point_expr, mat_inverse, mat_mul,
    rigid,
};
pub use document::*;
pub use eval::{
    EdgeRef, FeatureResult, Model, ModelBody, ModelState, PlacedBodies, RevolveExtent, SolvedSketch, ThreadInfo, curve_pick_error, edge_names_for,
    edge_refs, face_point_names, face_point_state, hole_direction_at, import_body_names, metric_thread_sizes, object_height, parse_metric_thread,
    pattern_matrices, revolve_extent, revolve_extent_with_convention, select_profiles, sketch_hole_direction, sketch_line_plane, state_in_frame,
    state_in_frame_at, suggested_thread, through_all_distance, world_state, world_state_reusing,
};
pub use expr::Expr;
pub use names::{BodyName, FeatureName, SketchId};
pub use params::{ParamRow, ParamUser, cycles};
pub use points::{Dir, FacePick, Position, plain};
pub use solidcraft_kernel as kernel;
pub use solidcraft_sketch as sketch;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DocError {
    #[error("expression: {0}")]
    Expr(String),
    #[error("{0}")]
    Invalid(String),
    #[error("unknown {0}")]
    Unknown(String),
    #[error(transparent)]
    Sketch(#[from] solidcraft_sketch::SketchError),
    #[error(transparent)]
    Kernel(#[from] solidcraft_kernel::KernelError),
}

pub type Result<T> = std::result::Result<T, DocError>;

#[cfg(test)]
mod feature_walk_tests;
#[cfg(test)]
mod named_projection_tests;
#[cfg(test)]
mod tests;

mod anchor_frame;
pub use anchor_frame::state_in_face_anchor_frame;
