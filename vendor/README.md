# Vendored third-party crates

Copies of third-party crates that SolidCraft builds with local fixes, wired in with
`[patch.crates-io]` in the workspace `Cargo.toml`. They keep their own licence; changed lines
are marked `SolidCraft:` in the source.

## truck-shapeops 0.4.0 (Apache-2.0, © RICOS Co. Ltd., https://github.com/ricosjp/truck)

Changes:
- `transversal/intersection_curve/mod.rs`: reject empty/single-point polylines and
  collapsed or non-finite segments before endpoint indexing and normalization. A hidden
  constructor re-export lets the kernel workspace tests exercise the actual vendored code.
- `transversal/integrate/mod.rs`: mesh both shells with `robust_triangulation` (faces made by
  earlier booleans have boundaries that lie on their surface only within tolerance).
- `transversal/divide_face/mod.rs`: when projecting a loop onto a face's surface, fall back to
  the nearest parameter when the exact search fails (intersection curves are approximations).
- `transversal/integrate/mod.rs`: the result's shells and faces keep the order of the shell
  they come from (truck's connected components followed hash order of face addresses, so a
  boolean's faces came out in a different order on every run).
- `transversal/loops_store/mod.rs`: a point at an end of a boundary curve (within the point
  tolerance) is that end, whatever its parameter: a polyline's parameter counts segments, so
  an intersection curve ending 2e-7 mm from another's end could read as inside it, cut it
  into a sliver with a vertex of its own, and leave the result's shell open (#391).
- `transversal/loops_store/mod.rs`: an intersection edge added to a face at a vertex that more
  than one wire visits (several curves end on it: the touching point of two equal cylinders) goes
  into the corner that holds its direction, swept counterclockwise about the face's normal from
  the wire's outgoing edge to the way back along its incoming one (`add_edge_by_angle`, told on
  the polygon loops and followed by the geometry loops). Taking the first visit of each end
  joined wires that are different regions of a face and left a wire visiting a vertex twice.
  With no direction to go by, the edge goes where both of its ends are in one wire.
- `transversal/divide_face/mod.rs`: a face piece whose wire is not simple makes the operation
  give up (`Face::try_new`) instead of panicking.
- `lib.rs`: `tr!` reports where an operation gives up when `SHAPEOPS_TRACE` is set; compiler
  warnings no longer fail the build.
- Face and edge history (`transversal/history.rs`, new): `and_with_history` / `or_with_history`
  also return, for each face of the result (in `face_iter` order), the operand face it is a
  region of and whether it is that face unchanged, and for each edge (distinct edges in order
  of first use along the faces) the operand edge it is (a piece of) or the two operand faces
  whose intersection made it. The code that creates faces and edges records them as it goes:
  `divide_face` (face pieces), `loops_store` (intersection edges, split edges, edges rebuilt on
  a moved vertex), across the passes that fold in the operands' extra shells. `and` / `or` are
  wrappers that drop the history; the `*_with_history` forms return the same solid with the
  history as an `Option` (`None` where a face or edge went unrecorded).

## truck-geometry 0.5.0 (Apache-2.0, © RICOS Co. Ltd., https://github.com/ricosjp/truck)

Changes:
- `decorators/intersection_curve.rs`: `IntersectionCurve::parameter_division` divides as
  `algo::curve::parameter_division` does (the same split points, in the same order), but each
  point's projection onto the two surfaces starts from the parameters of the point that begins
  its piece, instead of a search of the whole surfaces (a grid pre-search of a B-spline surface
  for every point: a small hole on a large curved face took seconds, #1038). A hinted result
  farther from the leader's point than the curve's tolerance (the projection settled on another
  part of a surface, such as the far side of a small hole) is dropped for the search from
  nothing, as before. `crates/kernel/src/issue_1038_tests.rs` holds it to that search.
- `decorators/revolved_curve.rs`: a point on a revolved surface's seam (angle 0, the same place
  as 2π) takes the end of the period nearer the hinted angle. The angle was decided by the sign of
  a cross product that is rounding noise there, so a boundary followed point by point could come
  back to the seam at the other end of the period and fold its parameter polygon over itself (no
  intersection loop then lay inside the face). `crates/kernel/src/seam_faces_tests.rs` holds it
  to a cone read from STEP.
- `nurbs/bspsurface.rs`: `uuder` and `vvder` multiply by p·(p − 1), as `BSplineCurve::der2`
  does. They multiplied by p only, so the second derivatives of every B-spline and NURBS surface
  of degree p ≥ 3 were 1/(p − 1) of their value (a cubic surface's curvature read as half, #763).
  `crates/kernel/src/surface_second_derivative_tests.rs` holds them to finite differences.

## truck-stepio 0.3.0 (Apache-2.0, © RICOS Co. Ltd., https://github.com/ricosjp/truck)

Only the writer (`out`) is used; SolidCraft reads STEP with its own reader.

Changes:
- `out/geometry.rs`: an `INTERSECTION_CURVE`'s second surface was written at the first
  surface's entity index, so every boolean-made edge produced duplicate entity ids.
- `out/geometry.rs`: surfaces of revolution carry the right `same_sense` for inverted and
  mirrored revolutions (ISO 10303-42 parameterises them by angle then profile, so their normal
  is the opposite of truck's; a mirroring transform reverses it again), and the modeling-surface
  wrapper passes the revolution's sense through instead of always `true`.
- `out/geometry.rs`: a surface of revolution under a transform that commutes with turning about
  its axis (a scale across the axis that differs from the scale along it, as a shell or offset
  makes of a revolved face) is written as the transformed profile revolved about the
  transformed axis. Only uniform scales were taken. Other transforms are still refused: the
  writer stops with a formatting error, which SolidCraft's `step.rs` now reports as an export
  error (`to_string` panicked on it).
- `lib.rs`: compiler warnings no longer fail the build. Example and test targets are dropped
  (their sources are not vendored).

## truck-meshalgo 0.4.0 (Apache-2.0, © RICOS Co. Ltd., https://github.com/ricosjp/truck)

Changes:
- `tessellation/triangulation.rs`, `closed.rs`: retry nearest trim projections outside a
  spline's knot domain without the preceding point's hint, preferring a bounded fit within
  the caller's chord tolerance, with a bounded damped least-squares fallback where
  Newton's method diverges. Affine plane charts remain unbounded; modelling surfaces may
  retain genuine continuation when no bounded fit is accurate enough. Geometrically closed,
  smooth spline seams get a local periodic chart during meshing, with wrapped evaluation,
  so a trim crossing the seam stays a small patch. The stored geometry is unchanged (#1974).
- `tessellation/mod.rs`, `triangulation.rs`: a separate provisional `preview_face`
  path accepts at most 4,096 already sampled boundary points and a fixed grid of
  at most 16 subdivisions per direction, including periodic seam connections.
  It does not run adaptive subdivision or grid balancing. The kernel caller bounds
  projection complexity and total allocation before using it; ordinary modelling
  tessellation keeps its existing accuracy policy.
- `tessellation/triangulation.rs`: exact, robust and provisional-preview projections prefer an
  equivalent in-domain parameter at collapsed surface edges, and snap to exact bounded
  endpoints only at collapsed edges when that improves the spatial fit within the existing
  search tolerance. Closed spline seams retain their distinct parameter endpoints.
  This prevents huge arbitrary pole parameters from corrupting trimmed meshes (#796),
  while retaining genuine extrapolation for unbounded planes. Work is bounded to four
  endpoint evaluations per projected point; no search or tessellation tolerance is relaxed.
- `tessellation/triangulation.rs`: a boundary loop is projected onto its surface point by point,
  each searched from the one before; the first has no hint. When that walk fails, it is tried
  once more from the first point an unhinted search finds. An unhinted search can miss a point
  that lies on the surface, such as the corner of a fillet strip on a surface extended past it,
  and the face was then left unmeshed, which also failed every boolean with the body (#882). A
  walk that succeeds is kept as it was: where a loop starts decides how it is laid out across a
  periodic surface's seam.
- `tessellation/mod.rs`, `triangulation.rs`: edges are sampled through a new `TrySubs` trait
  (`try_subs`, `approx_subs`), a bound of `PolylineableCurve`, instead of `subs`. truck's
  `IntersectionCurve::subs` unwraps a projection onto its two surfaces that can fail to
  converge, which panicked while meshing (#1252). A failed sample at an interval's end uses the
  intersection curve's leader point, so the polyline stays finite; a failed interior probe is
  NaN, which already counts as flat. `truck-shapeops`' `ShapeOpsCurve` carries the same bound,
  and `truck-modeling` is now a regular dependency (it was a dev-dependency).
- `tessellation/triangulation.rs`: the inside test of a face's boundary skips loops whose
  parameter box is clear of the point (it tested every segment of every loop, so a face with
  hundreds of holes meshed in quadratic time).
- `tessellation/triangulation.rs`: meshed faces and edges are built unchecked: faces whose loop
  runs along a seam edge both ways and closed edges (a full circle on one vertex) are valid,
  but the debug-build checks rejected them (debug builds could not mesh them).
- `tessellation/triangulation.rs`: parameter searches that land outside a surface's bounded,
  non-periodic domain are rejected (a hinted B-spline search can run off and extrapolate to
  thousands of parameter units, and meshing that grid never finishes); the last-resort nearest
  search is clamped into the domain.
- `tessellation/triangulation.rs`, bounded work (hostile files must not hang): edge polylines
  and surface grids come from our own bounded divisions (at most 20 000 points per edge, a
  grid budget per face shared out of 10 million points per shell, 24 refinement rounds; a
  point that does not evaluate counts as flat instead of recursing). Surface grids refine only
  the direction that bends, and a ruling left undivided gets cells comparable to the other
  direction (long triangles from a trim loop to the grid's far ends cut through cylinders).
- `tessellation/triangulation.rs`: an edge's division evaluates each parameter once. A piece
  carries the points of its ends and its midpoint is an end of both halves: 2 evaluations per
  piece instead of 4, and the same polyline (a point of an intersection curve costs two grid
  searches of its surfaces). `bounded_polyline` and `Deflection::turn2`/`turns` are public but
  hidden, for the kernel's regression against the old division.
- `tessellation/mod.rs`, `tessellation/triangulation.rs`, for display: a `Deflection` is the
  chord tolerance plus an optional bound on the angle one facet spans, and
  `RoundMeshableShape::robust_triangulation_within` divides by it: an edge or a grid direction
  within the chord tolerance is still halved while a piece turns more than the angle, until its
  sag is under a floor (a radius that is small beside the tolerance was one flat facet).
  Bounded work: a face that would be bounded by more than 4096 segments has its curved edges
  divided at twice the angle, and again, down to the chord tolerance alone, and the angle
  stops refining a face's grid at 16 384 points. Each edge is still divided once for both its
  faces. `triangulation` and `robust_triangulation` divide by the chord tolerance alone, as
  before.
- `analyzers/collision.rs`: the collision search builds each triangle's box once, filters the
  sweep's open list by box overlap as it goes (it cloned the list for every triangle and built
  both boxes for every pair), and runs the exact triangle tests on the rayon pool, in order
  (sequential on wasm). Same collisions in the same order. The old search is kept as
  `analyzers::collision_test_hooks`, behind a `test-hooks` feature that only
  `solidcraft-kernel`'s tests turn on, and `crates/kernel/tests/mesh_collision_equivalence.rs`
  compares the two.
- `tessellation/triangulation.rs`: the inside test of a face's boundary also skips runs of 32
  segments whose box, grown by the tolerance, the test's ray cannot meet. Same answers. The
  old test is kept as `tessellation::include_test_hooks`, behind a `test-hooks` feature that
  only `solidcraft-kernel`'s tests turn on, and `crates/kernel/tests/tess_include_equivalence.rs`
  compares the two.
- `lib.rs`: compiler warnings no longer fail the build. Example, test and bench targets are
  dropped (their sources are not vendored).
- `tessellation/triangulation.rs`: adaptive division also splits a cell that bows off its
  corners' plane by more than the chord tolerance (a quarter of its twist `p00 - p01 - p10 +
  p11`, across that plane, at the middle). A bilinear patch equals the blend of its corners the
  division compared against, so a twisted ruled face (a loft between polylines) went out as two
  triangles per face (#1666); a flat cell such as a cone's trapezoid between rulings is not split.
- `tessellation/triangulation.rs`: a shell's face meshes share their edges' points exactly. A
  face's mesh puts each point at its parameters on the face's own surface, so a point of an edge
  that lies off a surface within tolerance (an intersection-curve sample whose projection did not
  converge, an imported edge) came out in a different place on each of the edge's faces, and the
  exported mesh had a crack there. In display and export meshes (`Deflection::rulings`), a
  vertex at a point of a face's boundary loops that is farther than `TOLERANCE` from the nearest
  point of the face's edges now goes to that point, when it is within the chord tolerance
  (`onto_edges`). Points within `TOLERANCE` stay on the surface, so exact geometry meshes as
  before. The meshes Boolean operations intersect keep truck's points.

- `tessellation/triangulation.rs`: in display and export meshes (`Deflection::rulings`), a
  direction left undivided along the rulings of a face ruled one way also gets a grid line
  through each point of the face's boundary that lies inside the face along it
  (`with_boundary_lines`), when there are at most 16 such points: corners standing into the
  face, not a boundary curve running through it, which the triangulation already follows. A hole in a cone or cylinder smaller than one cell (a notch into a
  chamfer) or a trim crossing the rulings of a narrow band (an oblique plane through a bore's
  chamfer) had no grid point beside it, and the triangulation fanned from it to the far ends of
  the face, many degrees round: those triangles cut through the surface and folded over each
  other (a cone face with a small hole meshed several mm² too large, and the boolean that made
  it was refused for its volume).

## matext4cgmath 0.1.0 (Apache-2.0, © RICOS Co. Ltd., https://github.com/ricosjp/matext4cgmath)

truck-geometry uses it for a matrix's operator norm (an intersection curve scales its tolerance
by it whenever it is moved).

Changes:
- `solver.rs`: the Newton refinement of cubic and quartic roots stops after 100 steps. Near a
  double root it could cycle forever: moving a body whose edges a boolean made (the retries
  move both bodies and the result back) hung the app.
- `lib.rs`: compiler warnings no longer fail the build. Test targets are dropped (their sources
  are not vendored).

## egui-wgpu 0.36.2 (MIT OR Apache-2.0, Emil Ernerfeldt and contributors)

Original package sources from https://github.com/emilk/egui/tree/0.36.2/crates/egui-wgpu,
with both original licence texts retained. No images or bundled documentation are copied.

SolidCraft changes:
- `src/lib.rs`: use 1x MSAA on the selected OpenGL adapter before creating pipelines,
  exposing the effective sample count so host paint callbacks can match it.
- `src/winit.rs`: use that same effective count for depth and resolve attachments.
- `src/renderer.rs`: when the GPU refuses a staging buffer for a frame's meshes (device lost
  or out of memory), log a warning and draw nothing that frame instead of panicking (#1469).
- Other backends retain requested MSAA, without another adapter or device probe.
- `src/lib.rs`: clamp MSAA to the largest sample count the device supports for the surface and
  depth formats (`supported_msaa_samples`, `RenderState::msaa_supported`): a saved 8x on a
  device without it failed pipeline validation, a panic at start.
- `src/setup.rs`: the default device requests `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` when
  the adapter has it, so 2x and 8x work where the adapter supports them (WebGPU only
  guarantees 1x and 4x).
- `src/setup.rs`: the default device asks for the adapter's largest 2D texture (at least 8192)
  instead of exactly 8192.
- `src/lib.rs`, `src/winit.rs`: `set_startup_hook` is told before each step of getting the
  device and the window's surface up (instance, surface, adapter request, the chosen adapter,
  device, renderer, surface configure, first present), so the app's crash journal can say which
  step a crash inside the graphics driver happened in.
- `src/lib.rs`, `src/winit.rs`: a window larger than the device's largest texture gets a surface
  clamped to it (`clamp_surface_extent`, aspect ratio kept) and egui is drawn at the matching
  smaller scale, instead of a `Surface::configure` validation panic (#869). The resize path and
  an unsupported surface no longer unwrap.
- `src/winit.rs`: acquire a drawable before preparing paint callbacks, so a skipped
  surface frame cannot discard initialization commands for cached viewport meshes.
