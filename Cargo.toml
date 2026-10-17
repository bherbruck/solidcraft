[workspace]
resolver = "3"
members = ["crates/*", "apps/*", "xtask"]
# Third-party code kept with local fixes (see vendor/README.md); not linted as ours.
exclude = ["vendor"]

[workspace.package]
version = "0.15.0"
edition = "2024"
license = "MIT OR Apache-2.0"
rust-version = "1.95"
authors = ["SolidCraft contributors"]
repository = "https://github.com/bherbruck/solidcraft"

[workspace.dependencies]
# internal
solidcraft-geom = { path = "crates/geom" }
solidcraft-sketch = { path = "crates/sketch" }
solidcraft-kernel = { path = "crates/kernel" }
solidcraft-render = { path = "crates/render" }
solidcraft-gpu = { path = "crates/gpu" }
solidcraft-doc = { path = "crates/doc" }
solidcraft-io = { path = "crates/io" }
solidcraft-engine = { path = "crates/engine" }
solidcraft-ui-egui = { path = "crates/ui-egui" }
solidcraft-space-mouse = { path = "crates/space-mouse" }
solidcraft-mcp = { path = "crates/mcp" }
solidcraft-derive = { path = "crates/derive" }
solidcraft-3dx-client = { path = "crates/threedx-client" }

# foundation
thiserror = "2"
serde = { version = "1", features = ["derive", "rc"] }
# derive macros (crates/derive); already in the tree through serde_derive
proc-macro2 = "1"
quote = "1"
syn = { version = "2", features = ["full"] }
serde_json = { version = "1", features = ["float_roundtrip"] }
log = "0.4"
sha2 = "0.10"
moxcms = { version = "0.8.1", default-features = false, features = ["extended_range"] }
semver = "1"

# geometry kernel (Apache-2.0; see plan/adr/0001-geometry-kernel.md). Only crates/kernel uses it.
truck-modeling = "0.6"
truck-shapeops = { version = "0.4", default-features = false }
truck-meshalgo = { version = "0.4", default-features = false, features = ["tessellation", "analyzers"] }
truck-stepio = { version = "0.3", default-features = false }
truck-topology = "0.6"
# constrained Delaunay + Ruppert refinement for trimmed faces (MIT OR Apache-2.0); already in the
# tree through truck-meshalgo.
spade = { version = "2.15", default-features = false, features = ["std"] }
rayon = "1.12"

# packages (3MF)
zip = { version = "9", default-features = false, features = ["deflate"] }
quick-xml = "0.41"
# glTF 2.0 import (MIT OR Apache-2.0): the document model and GLB container only; buffers are
# read by crates/io itself, so no image decoding or file access comes with it.
gltf = { version = "1.4", default-features = false, features = ["names"] }

# FreeCAD and Rhino import (Apache-2.0, pure Rust apart from zstd; native builds only).
# Pinned: its IR and codec APIs change between minor versions. Open formats only: no default
# features, so cadmpeg's codecs for commercial formats (f3d, sldprt, inventor, catia, creo, nx)
# and its ShapeManager/ACIS `sat` codec are not compiled in. `step` is the writer we restate through.
cadmpeg-core = "=0.6.1"
cadmpeg-ir = "=0.6.1"
cadmpeg-registry = { version = "=0.6.1", default-features = false, features = ["fcstd", "rhino", "step"] }

# fonts (sketch text outlines; MIT OR Apache-2.0, pure Rust)
ttf-parser = { version = "0.25", default-features = false, features = ["std"] }

# images
image = { version = "0.25", default-features = false, features = ["png", "hdr"] }
png = "0.18.1"
# LZW for GIF animations (MIT OR Apache-2.0, pure Rust; already in the tree through image's tiff).
weezl = "0.1.12"
# rasterises the UI icon SVGs (BSD-3-Clause, pure Rust, builds for wasm)
tiny-skia = { version = "0.12", default-features = false, features = ["std", "simd"] }
# build scripts: reads the icon palette (MIT OR Apache-2.0)
toml = "1"

# ui
egui = "0.36"
eframe = { version = "0.36", default-features = false, features = ["persistence"] }
# Instant/SystemTime that also work in the browser (std panics on wasm32-unknown-unknown).
web-time = "1"
egui-wgpu = "0.36"
# the wgpu egui-wgpu uses, for compute (crates/gpu); backends come from the apps' features
wgpu = { version = "30", default-features = false, features = ["std", "wgsl"] }
naga = { version = "30", default-features = false }
pollster = "0.4"
rfd = "0.17"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
dbg_macro = "warn"
todo = "warn"
too_many_arguments = "allow"
type_complexity = "allow"
# `!(x > tol)` is our idiom for "not a usable number" (NaN-safe).
neg_cmp_op_on_partial_ord = "allow"

[profile.dev.package."*"]
opt-level = 2

[profile.dev]
opt-level = 1

# Tests and lints (`cargo xtask ci`, GitHub CI): quick to build, quick enough to run. Light
# optimisation, line tables only (file:line in backtraces), no LTO. Debug assertions are off because
# truck's debug-only checks reject valid geometry (seam edges, closed edges); that was the only
# reason the tests used to build with --release, which cost most of CI's time.
[profile.ci]
inherits = "dev"
opt-level = 1
debug = "line-tables-only"
debug-assertions = false
overflow-checks = true
incremental = false

[profile.ci.package."*"]
opt-level = 2

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 16
# Line tables only: crash reports name file:line in their backtraces (no runtime cost; a larger
# binary). See docs/crash-reports.md.
debug = "line-tables-only"

# truck-shapeops with SolidCraft fixes (Apache-2.0, see vendor/truck-shapeops and ATTRIBUTION.md).
[patch.crates-io]
truck-geometry = { path = "vendor/truck-geometry" }
truck-shapeops = { path = "vendor/truck-shapeops" }
truck-stepio = { path = "vendor/truck-stepio" }
truck-meshalgo = { path = "vendor/truck-meshalgo" }
matext4cgmath = { path = "vendor/matext4cgmath" }

# Keep OpenGL pipelines, resolve textures and CAD callbacks at the same safe sample count.
egui-wgpu = { path = "vendor/egui-wgpu" }
