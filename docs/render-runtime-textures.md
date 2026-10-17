# Runtime planar opaque maps

Progressive GPU jobs accept immutable per-body base-color and OpenGL tangent-space
normal maps. No document, appearance, recovery, or saved material format changes.
Only explicit runtime commands capture file bytes; queued work does not reopen paths.

```json
{"progressive":true,"raytrace":{"textures":[{"body_index":3,"body_name":"Walnut tabletop","frame":{"origin":[0,0,480],"u":[900,0,0],"v":[0,90,0]},"base_color_srgb":"C:/art/wood.png","normal_opengl":"C:/art/wood-normal.png","normal_strength":0.35}]}}
```

Each `u` and `v` world-space vector spans one complete repeat, in millimetres. The
vectors must be nonzero, orthogonal, and 0.001–1,000,000 mm long. The origin is UV
zero. Image V increases downwards; normal red and green point along the positive
projected U and V directions, with handedness preserved. Projection is planar,
not triplanar: surfaces parallel to the projection axes may stretch. Each body's
bounding-box corners must remain within ±128 repeats; the frame origin must be
within 10 million mm of the camera target. These caps limit f32 coordinate loss.

Base RGB is explicitly sRGB, converted to linear reflectance before bilinear
filtering and multiplied by the body's linear base color. Use a white base to
reproduce the map's color. Normal channels are untransformed linear data. Normal
strength scales tangent XY in 0..1 before normalization; degenerate vectors, an
invalid tangent projection, or a normal behind the geometric/view hemisphere
fall back to the original shading normal. Alpha is ignored by this opaque-only
consumer. Glass, scalar opacity, roughness, metalness, displacement, height and
alpha maps are rejected or unsupported; they are not silently interpreted.

Sampling uses bilinear level-zero periodic filtering at texel centers, including
negative UV repeats. There is no mipmapping, anisotropic filter or UV mesh import
in this increment, so fine distant patterns may alias. PNG/JPEG input is 8-bit,
maximum 1024×1024 per map. Embedded ICC profiles and nonidentity orientation must
be normalized beforehand; no implicit color-management guess is made. The sRGB
base/data normal distinction follows the public
[Khronos glTF material semantics](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#materials),
without adopting its document format or copying implementation code.

At most 16 exact name-and-index body bindings retain 16 MiB encoded capacity and
32 MiB linear pixel data collectively. Admission counts captured bytes, codec
scratch, normalized pixels, staging and device storage against the existing job
budget. Codecs execute on the worker with cancellation checks before/after each
bounded decode and during conversion/upload preparation. Public status contains
metadata only; completion or cancellation releases captured source bytes even
when callers retain job/status/frame handles. GPU preparation also releases
source bytes after preparing the pixel buffer.

The independent progressive device requests five storage buffers per shader
stage. An adapter limited to four cannot install this executor; raster and
tessellation device requirements remain unchanged.

Independent f64 tests cover explicit sRGB decoding, linear normal channels,
repeat filtering, handedness, degenerate fallback, admission and lifecycle.
GPU reference tests compare the actual packed storage and WGSL sampler/normal
frame against those CPU utilities. GPU execution and final visual evidence must
be reported separately; source/shader validation alone is not rendering proof.
