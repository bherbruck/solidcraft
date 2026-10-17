# Glass shadow connections

Runtime dielectric overrides accept optional `shadow_transmission: true|false`.
Thin sheets default to true. Closed boxes and meshes default to false and opt in
explicitly. This changes only a progressive job's captured optical data; document
formats, saved appearances and scalar preview opacity are unchanged.

```json
{"progressive":true,"raytrace":{"dielectrics":[{"body_index":0,"body_name":"Glass","ior":1.5,"absorption_per_mm":[0.02,0.005,0],"boundary":"closed_mesh","thickness_mm":0,"shadow_transmission":true}]}}
```

Direct lighting traverses at most 64 dielectric interfaces before the emitter.
It uses the closest hit, so geometry beyond a finite light does not shadow it.
Opaque and disabled interfaces block. Unmatched exits, overlapping active media,
an unclosed infinite connection, or an exhausted interface budget also block.
A direct shadow connection starting at an opaque surface inside an active solid
is currently unsupported and conservatively blocks at the unmatched exit. Such
embedded opaque receivers should use disabled solid shadow transmission; they
are outside the enabled solid approximation contract.
The existing host topology/disjoint-volume admission remains in force. Coincident
or sub-offset sheets are outside the existing ray-offset precision contract.

For a thin sheet the connection multiplies the existing exact unpolarized slab
transmittance, including Beer absorption along thickness/refracted cosine and
repeated internal reflections summed separately for s/p polarization. The path
direction stays straight. Light MIS and BSDF emitter hits use the same mixture
PDF, including each sheet's transmission branch probability. The last opaque
scattering point is kept separately from the Beer segment origin. Reflection
starts a delta path; straight enabled sheet transmission preserves the MIS
history. If the remaining path depth cannot traverse all sheets and reach the
emitter, the light technique gets weight one. This avoids both double counting
and losing energy at the depth limit.

An opted-in solid uses an explicitly approximate straight connection. It applies
reciprocal external-angle Fresnel at each interface and Beer absorption over the
actual, unshifted geometric entry/exit distance. For a finite emitter inside a
certified volume only the partial distance to that emitter is absorbed, and the
uncancelled radiance factor `1 / ior^2` is retained. The physical endpoint is kept
separate from the offset visibility cutoff. Applying
inside critical angles to an unrefracted shadow ray would create artificial TIR,
so the shadow approximation uses air-to-solid Fresnel at both boundaries.

Solid direct transmission is **NEE only**: the direct connection gets weight one,
and an opaque-bounce path transmitted through an enabled solid does not also add
emitter/environment emission. Camera paths and paths after dielectric reflection
retain the physical Snell/Fresnel transport. Another opaque hit starts a new direct
lighting estimate. This approximation deliberately replaces transmitted direct
caustic paths; it does not simulate focusing, caustics, refracted light silhouettes
or internal multi-bounce transmission for direct lighting. Disable solid shadow
transmission to retain the previous physical path estimator.

Tests cover normal/oblique slab RGB against f64 Fresnel/Beer oracles, finite
endpoints before/inside/beyond glass, disabled/opaque blockers, unmatched exits,
stacked sheet PDFs, the 64/65 crossing boundary, and index-matched thin/solid
fixtures at several path depths that reject doubled direct energy. The independent
CPU oracle also checks reciprocity, defaults, hostile values and finite grazing
behavior. GPU execution must be validated separately on available adapters;
final combined backend and showcase acceptance remains incomplete.

The estimator follows the public mathematical description of
[multiple importance sampling in PBRT](https://pbr-book.org/4ed/Light_Transport_I_Surface_Reflection/A_Better_Path_Tracer).
The straight solid policy is an explicit SolidCraft preview approximation, not a
claim made by that reference. No reference source code was copied.

Medium crossings are tested against the true finite endpoint distance. The analytic area-emitter self-hit tolerance is separate and does not shorten the geometric connection; exits or entries arbitrarily close before the endpoint still change its medium and radiance factor.
