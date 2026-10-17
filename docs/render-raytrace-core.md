# Optional CPU triangle visibility core (RT1, #1368)

This page documents the RT1 numerical core, not the complete image consumer.
The [RT2 image contract](render-raytrace-image.md) covers native jobs and hard
shadows. The [combined HDR/reflection integration](render-hdr-reflections-integration.md)
extends that consumer; its capabilities do not change this core's standalone scope.
See the RT2 page's current integration limits before treating historical source
checks as validation of the combined revision.


`solidcraft-render` exposes `raytrace` only with the explicit `raytrace-cpu`
feature. The feature is **off by default** and adds no dependencies. This page
describes the core alone. The separate [RT2 consumer](render-raytrace-image.md)
connects it to the existing image executor; neither unit adds a worker pool or
persisted settings. This numerical core alone does not render an image or implement cast shadows, progressive
transport, soft shadows, textures, HDRI, glass, denoising or GPU tracing.

The source is original safe Rust using elementary slab, barycentric and binary
tree mathematics. No Blender/Cycles, PBRT or other tracer source was consulted
or adapted. Only L0 `Vec3` geometry and the existing render cancellation token
are consumed; no truck, B-rep or SolidCore interface is involved.

## Geometry and hit policy

Inputs are already world-space tessellation in millimetres. Instance transforms
must not be applied again. Each admitted `Ray` normalizes its finite nonzero
direction using scaled arithmetic; its closed finite `[min, max]` interval is
therefore measured in millimetres. Origins and triangle vertices are limited to
`+/-1e12` mm; maximum ray distance is `4e12` mm. Ray direction magnitudes are not
subject to a fixed small-vector epsilon.

`Triangle` admits coincident and collinear vertices, which produce no hits.
Intersection projects into the plane perpendicular to the dominant ray axis,
applies a shear, and evaluates closed-edge barycentrics. Both sides can hit.
Geometric normals and the front-face flag are returned separately from any
future shading normal. Zero projected area is a miss, including geometry that
collapses in f64 arithmetic; this is not an exact CAD predicate or a universal
watertightness guarantee. No fixed determinant threshold masks small triangles.
Analytic source tests cover scales from `1e-120` to `1e11` mm.

Closest-hit ties at exactly equal computed distance choose the lowest captured
input ordinal. That ordinal is **not a persistent CAD face identifier**: the
later consumer must preserve its own triangle-to-appearance mapping. Shared
edges and vertices are included; no artificial half-open coverage rule is
introduced. Ray origins are not offset automatically. In particular, `min=0`
allows a hit at the origin; RT2 must explicitly define a scale-aware self-hit
policy for shadow rays without erasing thin geometry.

Boxes use closed slab overlap and special-case zero/signed-zero directions.
Almost-parallel division may produce infinity, which is clipped against the
finite ray interval; the implementation does not multiply zero by an infinite
reciprocal. Triangle boxes expand outward by one representable floating-point
step, rather than a fixed millimetre tolerance.

## Bounded preparation

`BvhBuilder` takes ownership of a vector of admitted triangles. Input capacity,
not just input length, participates in memory admission. It reserves ordinal,
flat-node and pending-task buffers before constructing the hierarchy. There is
no sorting copy of the scene, recursion, unbounded partition retry or hidden
secondary worker. All buffer allocation is fallible.

`step(operations, cancel)` accepts 1–4096 operations. Each operation initializes
one ordinal, scans one primitive's bounds/centroid, partitions one ordinal, or
performs a constant-size state/node transition. Work and cancellation are
checked before every operation. Source scanning and partitioning are resumable,
so a large node does not hide a full-scene operation inside one step.

The split plane is the midpoint of the widest centroid extent, with X/Y/Z tie
order. Partitioning is deterministic. An empty partition falls back to the
middle of the existing ordinal range. The private task stack is bounded by
depth; at the depth cap a leaf retains its remaining range. `finish()` refuses
an incomplete builder; partial acceleration structures are not published.

Defaults allow 100,000 triangles, 128 MiB logical memory, 10 million operations,
depth 32 and leaves of eight primitives. Hard admission caps are one million
triangles, 512 MiB, one billion operations, depth 48 and leaf target 64. Checked
arithmetic rejects overflow before allocations. Maximum node reservation is
`2*N-1`; empty scenes reserve no nodes. If a platform returns larger Vec capacities,
those capacities are checked again before use.

The memory figure includes owned triangle capacity, ordinal/node/task capacities,
fixed builder/BVH metadata and one traversal stack. It is a logical buffer bill,
not allocator-overhead/RSS measurement and not an acquired host lease. The RT2
image consumer must admit these bytes through the existing host pool together
with capture, output, encoding and any other job resources. No such integration
is claimed here.

## Queries and cancellation

`closest_hit` and `any_hit` use a fixed 49-slot traversal stack. They check the
existing cancellation token and consume a caller-owned `WorkBudget` for every
node visit and primitive test. One budget can span many rays. Budget exhaustion
returns `TraceError::WorkLimit`; it never returns the best hit found so far as
if traversal had completed. Any-hit stops at a real primitive hit. A leaf at the
depth cap can be large, but each primitive still has its own work/cancel check.

These are bounded synchronous queries, not an installed cooperative browser
renderer. Host scheduling, per-job wall deadlines, late-result identity and
publication/cancellation policy remain responsibilities of the existing job
system and the RT2 consumer. Default builds do not compile or initialize this
module and ordinary modelling/opening/saving routes are unchanged by RT1.

## Validation status and next unit

The source includes analytic distance/barycentric/normal tests, tiny/huge and
grazing cases, shared-edge ties, slab/parallel cases, BVH-versus-brute-force
seeded closest/any-hit comparisons, hostile float admission, deterministic
incremental stepping, depth/degenerate termination and work/memory/cancellation
checks. These are planned executable checks, **not passing results until run**.

Required validation before publication: project quality gates, default and
feature-enabled render tests/strict Clippy, and feature-enabled wasm checking.
The current source-only preparation intentionally does not run Cargo while
another owner holds the shared machine's build window.

RT2 is the separate useful image unit: explicitly selected opaque CPU visibility
and cast shadows, using the existing camera/scene and host ownership/accounting.
That unit needs an original native scene and completed output, cancellation and
two-document/late-result checks, unsupported-optics refusal and measured costs.
Nothing in this core alone satisfies that visible acceptance milestone.
