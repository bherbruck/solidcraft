# Bounded primary spatial AA proposal (#620 / existing render-quality work)

SOURCE ONLY: written tests have NOT RUN. No compile, layout measurement, PNG,
performance, full-CI or presentation-quality acceptance is claimed. This isolated
unit is based on exact central fa8c source1601, with real claim28414 and real
pending main04b ancestry. It is not incorporated into the central candidate.

`raytrace.primary_samples` is a transient request option: omitted/1 retains the
legacy fixed-centre route,4 uses a2x2 pixel-box grid,16 uses4x4. Other counts
refuse. Default1 is omitted from serialized options to preserve the default
request representation. Nothing is added to document/RenderSetup persistence,
undo, UI, GPU MSAA, capabilities, libraries, glass, LED or the separate VNDF unit.
Use the existing engine/MCP request transport; the current CLI render wrapper
still does not expose raytrace options or a persistent raytrace job workflow.

The single-primary branch keeps its former generator, reuse proof, R addition
order/divisor, primary-background shortcut, seeds, work debits and encoding.
New modes explicitly revoke primary reuse and complete ALL S*R attempts, even
for a primary miss or nonselected surface. R remains the existing reflection
attempt count (or1 with no active selections), bounded1..64. Null reflected
continuations contribute zero and still count in R; they never erase primary
coverage. Grid centres are strictly inside the pixel, fixed row-major, without
jitter/temporal sampling or a progressive-prefix claim. New reflection keys use
checked pixel*S+s plus a fixed AA domain xor; the existing sampler receives the
same reflection ordinal, bounce and R as before. No PDF or BSDF is changed.

For geometry hit indicator H and scene-linear path radiance L, C=sum(H)/(S*R)
and P=sum(L)/(S*R). New modes compute conditional geometry radiance G=P/C,
apply exposure/Reinhard once to G, then composite C*T(G)+(1-C)*decode(background)
in display-linear sRGB before one sRGB encoding. All-miss pixels retain the exact
background bytes; full coverage omits background operations. Background remains
display-referred, not an emitter and not exposure-controlled. Do not averageRGB8,
individually tone-mapped paths or tone the premultiplied P. Output alpha stays255;
coverage is internal quadrature, not transparency/refraction support.

One ImageTask/PendingPixel/BVH/host pool and one cumulative1e9 WorkBudget remain.
Only fixed scalar/Vec3 state is added. Per-R-add radiance limits remain unchanged;
spatial normalized sums have a finite1e12 bound. Coverage is checkedu16<=1024.
Separate spatial finalization and pixel emission states each debit work and yield;
cancellation is checked again just before pixel bytes are emitted. Running
progress still counts completed OUTPUT pixels. No partial PNG or samples reduced
on exhaustion. Existing384MiB job/512MiB host limits and PNG encoder are retained.
A compile-time type-length check and written layout regression require actual
ImageTask<=4096 bytes; because no build has run, actual layout is NOT qualified.

Cost can grow with S*R, traversal, lights, HDR and bounces; revoking nonselected
reuse means some paths cost much more than S times the former fast path. Limits
can refuse jobs that previously completed. No work/ray equality, speedup, smooth
metal, universal antialiasing or final-mode claim follows. Thin/repeated geometry
can still alias under fixed quadrature, and reflection noise is a separate issue.

Nine written regressions cover strict options/default serialization; grid and
analytic discrete edge coverage; conditional tone/display-linear compositing;
actual hit-first/miss-first half-plane traversal acrossR1/4/16/64 with nulls;
all-miss bytes and quantum parity; full coverage; cancellation/work exhaustion
in later spatial samples/finalization/emission; and S1 isolation/bytes/work with
same caps/layout. Old tests/assertions remain untouched. Execution is pending.
Default-vs-historical exact-build byte/work comparison, engine/host preflight
validation, arbitrary projection scenes, allocation/layout and ownership gates,
Clippy/wasm and required cargo xtask ci are required before publication. Designer
must inspect actual equal-camera edge comparisons; source math is not visual
acceptance. No external code/assets/dependencies were added.
