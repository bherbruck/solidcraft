//! Sketches: points and curves on a plane, geometric constraints and driving dimensions, a
//! Levenberg–Marquardt constraint solver with degree-of-freedom analysis, and closed-profile
//! (region) detection for features.
//!
//! The solver is our own clean-room implementation (residual functions per constraint, local
//! finite-difference Jacobians, damped Gauss–Newton per connected component, rank analysis by
//! Gaussian elimination). See `plan/architecture.md` §sketch.
//!
//! Before iterating, a constructive seed places every point that two loci on already placed
//! points pin down (circles, rows and columns, lines through placed points, offsets from
//! placed lines, tangency, midpoints, mirrored points, coincidence), on the branch that keeps
//! the orientation the point had before the solve, so dimension edits do not fold the sketch
//! into a mirrored solution or flip an arc out of its corner. The seed only moves the starting
//! point: the iteration still verifies every constraint and the rank analysis still decides
//! the degrees of freedom.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod construct;
mod curves;
mod decompose;
mod dims;
mod ends;
mod fonts;
mod freeform_offset;
pub mod linalg;
mod link;
mod loci;
mod model;
mod offset;
mod offset_topology;
mod pattern;
mod periodic;
mod profiles;
mod shape;
mod solver;
mod text;
mod thin;
mod union_find;
mod wire;

pub use curves::{
    MAX_SPLINE_POINTS, bspline_beziers, conic_point, conic_rho_from_point, ellipse_point, end_curvature, fit_beziers, fit_beziers_mode,
    spline_end_tangent, spline_end_tangent_mode, spline_point, spline_point_mode, spline_polyline, spline_polyline_mode,
};
pub use decompose::Budget;
pub use dims::{DimFrame, DimLayout, chain_centres, default_text, dim_frame, dim_layout, dim_layout_reaching, dim_reach, encode_text};
pub use ends::{END_TOL, EndReport, EndTouch, UntiedEnd, loose_ends};
pub use fonts::{DEFAULT_FONT_FAMILY, FontData, families_in, find_style_in, font_data, font_data_styled, installed_families, set_font_file_check};
pub use freeform_offset::{FreeformOffset, OFFSET_TOLERANCE};
pub use link::{Link, LinkGeom, LinkKind, LinkSource, MAX_LINK_ENTITIES, SKETCH_AXIS_HALF, TextExprs, sketch_axis_geom};
pub use model::{
    Constraint, ConstraintKind, Curve, CurveKind, MAX_CONSTRAINTS, MAX_COORD, MAX_CURVES, MAX_POINTS, SPoint, Sketch, SketchError, SketchView,
};
pub use offset::OffsetChain;
pub use offset_topology::{OffsetCreated, OffsetRole, OffsetRoleFlags, OffsetRoleKind, OffsetTopology};
pub use pattern::{PatternAlignment, PatternInstance, SketchPattern, SketchPatternKind};
pub use periodic::periodic_beziers;
pub use profiles::{
    CurvePick, Profile, distinct_curves, find_drawn_profiles, find_profiles, merge_regions, profile_at_point, profile_by_curves,
    without_text_counters,
};
pub use shape::{Shape, intersections};
pub use solver::{
    Analysis, CONSISTENT, DrivingValues, Hold, SolveReport, SolveStatus, constraint_residual, determined_by, measure_dimension, solve,
    solve_after_edit, solve_after_value_edit, solve_holding, solve_with,
};
pub use text::{MAX_TEXT_CHARS, TextAlign, TextStyle, text_geometry, text_geometry_in, text_geometry_styled};
pub use thin::{MAX_THIN_SEGMENTS, thin_wall_regions};
pub use wire::{MAX_FIT_POINTS, MAX_WIRE_POINTS, Wire, WireSpace, fit_polyline};

#[cfg(test)]
mod construct_rounds_tests;
#[cfg(test)]
mod construct_tests;
#[cfg(test)]
mod continuation_tests;
#[cfg(test)]
mod ends_tests;
#[cfg(test)]
mod font_tests;
#[cfg(test)]
mod least_motion_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod thin_tests;

#[cfg(test)]
mod virtual_corner_tests;
