# Neutral section geometry (W01a)

W01a, [#2053][issue] / [PR #2085][pr], adds the L0
[`solidcraft_geom::section`][section-source] representation and structural validation. See the
[foundation ledger](drawing-foundations.md) for dependencies and the remaining
W01 work. This contract is deliberately smaller than the eventual section and
projection result described in the [Drawing plan][plan].

The module can hold geometry supplied by a caller, validate its structure and
convert selected closed wires/regions to existing `Loop2`/`Region2` values. It does
not intersect model bodies, determine visibility, certify material regions,
attach persistent references or register an engine command. No native design
format, exporter, formatter or new runtime dependency is introduced here.

## Coordinates and frames

Distances and coordinates are millimetres; angular values in `Seg2` are radians.
The model is Z-up. Each result contains the existing [`Plane`][plane-source]:

```text
world(u, v) = origin + u * x + v * y
normal = x cross y
```

`validate_plane(&Plane)` checks a supplied frame without normalizing it. Origin
and both axes must be finite, with every component's absolute value at most
`1e12`. Each axis must be unit length within `1e-8`, and their absolute dot product
must be at most `1e-8`. The normal is derived from the axes; no separate normal or
handedness flag is serialized.

The existing named frames are:

| Plane | Local x | Local y | Derived normal |
| --- | --- | --- | --- |
| XY | +X | +Y | +Z |
| XZ | +X | -Z | +Y |
| YZ | +Y | +Z | +X |

In particular, XZ's local positive y points toward **negative world Z**. Replacing
it with +Z would reverse the normal. `Plane::flipped` reverses an in-plane axis
and therefore the derived normal; validation does not choose an orientation.

The future Stage A request adapter must preserve the existing
[`inspect.section`][section-command] conventions: `offset` follows the named
plane's normal; `at` is the world Z/Y/X coordinate for XY/XZ/YZ respectively;
`offset` and `at` are mutually exclusive; `flip` changes the cut direction without
moving the requested plane. These request fields are not part of W01a's DTO.

Mapping a point already on the plane to local coordinates and back is meaningful.
Mapping an arbitrary 3D point to two coordinates loses its normal component; it
does not have a unique 3D inverse.

## Data and serialization

| Type | Fields and meaning |
| --- | --- |
| `SectionGeometry` | `plane: Plane`, `curves: Vec<SectionCurve>`, `wires: Vec<SectionWire>`, `regions: Vec<SectionRegion>`, `contacts: Vec<Vec2>` |
| `SectionCurve` | `geometry: Seg2`, `accuracy: CurveAccuracy` |
| `SectionWire` | Ordered `curves: Vec<CurveUse>` and explicit `closed: bool` |
| `CurveUse` | `curve: u32` indexes `SectionGeometry.curves`; `forward: bool` selects its direction of use |
| `SectionRegion` | `outer: u32` indexes a wire; `holes: Vec<u32>` indexes its supplied hole wires |

All indices are **local to this one value**. They do not identify a model body,
face, occurrence, revision or an entity across recomputation. W01a has no source
fingerprint, provenance, freshness, persistent-ID or public schema-version field.
Those require a reviewed producer/integration contract in a later W01 slice.

The new section DTOs reject unknown fields through serde. Existing `Plane`,
`Vec2`, `Vec3` and [`Seg2`][segment-source] retain their existing serialization:
vectors are numeric arrays; plane axes are `x_dir` and `y_dir`; segment variants
use a `type` tag with snake-case names. Do not apply a blanket strict-schema claim
to the reused nested types.

`Seg2` represents lines, circular arcs, cubic Béziers, rational quadratic conics
and supported higher-degree polynomial Bézier pieces. This is not every curve in
the neutral B-rep schema, nor evidence of an exact producer for each kind. An
adapter must preserve supported geometry or report its actual conversion and
accuracy; it must not silently coerce unsupported B-rep curves into this shape.

`CurveAccuracy` uses the internally tagged serde field `kind`:

| JSON form | Meaning supplied by the producer |
| --- | --- |
| `{"kind":"analytic"}` | The producer declares an analytic representation without fitting |
| `{"kind":"bounded","max_deviation_mm":0.001}` | The producer claims a deviation bound in model millimetres |
| `{"kind":"sampled","max_sample_deviation_mm":0.001}` | The recorded deviation applies to evaluated samples; it is not a bound between samples |

Validation checks the metadata's numeric domain. It does not independently prove
an analytic classification or a claimed bound against a source surface. Accuracy
is per curve, not a global `exact: true` flag. There is no implicit default tag.
In particular, a requested fitting tolerance is not an established bound, and a
zero sampled deviation remains sampled.

This complete value shows the actual serde spelling for one open line. It is
data, not a request to an implemented Drawing command:

```json
{
  "plane": {
    "origin": [0, 0, 0],
    "x_dir": [1, 0, 0],
    "y_dir": [0, 1, 0]
  },
  "curves": [
    {
      "geometry": {"type": "line", "a": [0, 0], "b": [10, 0]},
      "accuracy": {"kind": "analytic"}
    }
  ],
  "wires": [
    {"curves": [{"curve": 0, "forward": true}], "closed": false}
  ],
  "regions": [],
  "contacts": []
}
```

The value can pass structural validation. Converting wire 0 to a `Loop2` must
fail because it is explicitly open. Merely meeting endpoints does not change a
wire's `closed` flag. Contact points are representable, but W01a does not classify
them as tangent intersections or certify their source.

## Validation and conversion boundary

The public operations are `validate_plane(&Plane)` and
`SectionGeometry::validate_structure(tol)`, `to_loop(wire, tol)` and
`to_region(region, tol)`. Errors use the module's `Invalid` error type. There is
no geometric `bounds()` API or batch region-expansion API in this slice.

`tol` must be finite and in `1e-9..=1` mm. It is a structural endpoint-connectivity
tolerance, separate from each curve's accuracy metadata and from future model
intersection tolerances, paper deviation and dimension display precision.

The public `MAX_ITEMS` is `100_000`. Validation counts the aggregate of curves,
wires, regions, contacts, all curve-use references and all hole references before
walking the geometry. Counting references prevents a small curve set from hiding
an unbounded number of uses. This limit is not a maximum JSON byte size.

Structural validation checks the frame, numeric domains, finite curve/contact
data, bounded collection counts and local reference integrity. It checks all
serialized Bézier control slots, including unused tail slots. Every wire must be
nonempty; oriented consecutive endpoints must meet within `tol`. A wire declared
closed must also meet last-to-first. Region boundaries must reference closed
wires; a region cannot list its outer wire as a hole or repeat the same hole.
Different regions may share a wire.

The arithmetic domains are explicit, not a geometric precision certificate:

| Stored value | Accepted domain |
| --- | --- |
| Coordinates and control slots | Finite; absolute component at most `1e12` mm |
| Arc radius | `1e-12..=1e12` mm |
| Arc start | `-1e6..=1e6` radians |
| Absolute arc sweep | `1e-12..=2*pi + 1e-12` radians |
| Conic weight | `1e-12..=1e12`; only positive weights are supported here |
| Higher-degree Bézier `n` | `4..=9`, using the existing ten-slot control array |
| Bounded/sample deviation | Finite, `0..=1e12` mm |

Line and Bézier/conic controls must not all coincide; evaluated segment endpoints
must be finite. These tests do not certify a minimum physical feature size or a
small absolute arithmetic error at extreme magnitudes. In particular, the stored
coordinate limit is not a computed geometric bounding box.

`to_loop` and `to_region` revalidate the entire value, check the selected index and
preflight the selected output's segment expansion before conversion. A
`forward: false` use copies the reversed segment. They preserve the supplied wire
orientation, outer/hole classification and geometry; they do not fit, repair,
close or correct a boundary's winding to make it acceptable. Repeated conversions
repeat validation. Callers planning many conversions must account for that work;
this API does not provide a shared validation certificate or a batch result.

Conversion also checks the actual converted loop's closure with the same `tol`.
Reversing an arc computes a new start angle; at large radii, floating-point
rounding can make its evaluated endpoints disagree even when the original wire
passed validation. The converter then returns
`converted loop is not closed within tolerance`; it does not snap endpoints.
`Loop2`/`Region2` contain geometry only, not `CurveAccuracy` or the result-local
indices. A caller that needs those records must retain the original metadata;
conversion neither carries an accuracy certificate nor promotes accuracy.

**Structural success does not certify winding, nesting, self-intersection,
separation of regions or a physically valid material section.** Converters do not
establish those properties either. The future section producer must supply and
verify outer-CCW/hole-CW boundaries in `(x,y)` coordinates, nesting, signed/net area
semantics and legitimate material regions. A result may be structurally valid
while violating those future geometric requirements.

Existing profile area and containment methods also have their own curve-specific
limitations; converting to `Loop2` or `Region2` does not make conic/freeform area or
containment exact or certified.

Deserialization does not call structural validation automatically. Validate before
use or serialization; the public data may have changed since an earlier check.
Serde can allocate vectors before these validators run. A future file, command
or transport adapter must impose byte, allocation, nesting and request limits
before or during decoding, then call structural validation before downstream
geometry operations. Producer traversal/iteration budgets, cancellation and
complete source selection are also future caller responsibilities. None is
provided by the `MAX_ITEMS` check after deserialization.

## Fixtures and handoff

These labels connect to the [requirement ledger](drawing-foundations.md).
Current checks exercise manually supplied representation values, not extracted
model geometry; see [`section_tests.rs`][tests-source]. Test results belong to the
tested PR revision, not to this list.

| Fixture | Current check or remaining acceptance gate |
| --- | --- |
| F01: frames | W01a: finite/orthonormal plane validation, XZ sign and plane-coordinate mapping. Degenerate, nonfinite and out-of-range frames fail |
| F02: representation/accuracy | W01a: supported segments and actual serde shape; accuracy numeric-domain checks; sampled error remains distinct from a bound |
| F03: rectangle with a hole | W01a: manual `100 x 60 mm` rectangle and radius-5 circular hole at `(50,30)`, in `Plane::XY.offset(10)`. Conversion preserves signed areas `6000` and `-25*pi`, and net area `6000 - 25*pi mm²`. It is not a body-section or nesting certificate |
| F04: malformed structure/conversion | W01a: invalid indices, disconnected/empty wires, closure, invalid region references, excessive aggregate items, nonfinite data, malformed curves and post-conversion endpoint agreement; no mutation |
| F05: plate through a hole | W03: a `100 x 60 x 20 mm` box with a radius-5 through hole parallel to Z, cut by an XY plane strictly between its end faces, has section area `6000 - 25*pi mm²`; producer must identify and certify outer/hole boundaries |
| F06: tilted cylinder | W03: radius 10, axis Z, plane normal 30° from Z, away from caps: ellipse semiaxes `10` and `10/cos(30°)`, area `100*pi/cos(30°)`. Analytic representation or justified conversion evidence is required |
| F07: section outcomes | W03: empty/tangent/coincident/open/degenerate/unsupported cases, with explicit outcome semantics rather than a guessed empty success |
| F08: source completeness | W02-W04: repeated/nested/mirrored occurrences retain source identity; one unsupported selected body cannot silently disappear from a complete result |
| F09: command/output | W04: explicit plane/selection, same CLI/MCP/engine result, no model/undo mutation, complete physical SVG/DXF output and bounded cancellation/error behavior |

F03 is `line_arc_region_conversion_preserves_winding_and_analytic_area`: one
rectangle edge is stored backwards and used in reverse; two semicircular arcs
are used in reverse order/direction to form the hole. It checks orientation and
unchanged input as well as these analytic line/arc areas. In contrast,
`connected_bow_tie_and_wrong_winding_are_not_certified_or_silently_repaired`
deliberately accepts and preserves structurally valid but geometrically unsuitable
boundaries. Neither test extracts a cut from a solid.

`reversed_large_arc_conversion_rechecks_actual_connectivity_without_snapping`
checks the converted result too: a reversed radius-`1e12` arc with start `0.1` and
sweep `1.0` radians fails at `tol=1e-9` mm but is accepted at `0.01` mm. This is an
arithmetic/closure regression, not evidence of that accuracy against a source.

[NOWN-DEV's PR #2068][producer-pr], reviewed as an open proposal rather than
merged code, contains Stage A producer/export work and a tilted-cylinder test.
Review that evidence and its actual mesh/fitting guarantees before reusing it.
Its body-name/loop/`sweep_deg` output is a distinct schema; W01a supplies no adapter
and promises no automatic compatibility. Agree conversion, accuracy and source
identity handling with that contributor before further overlapping claims.

W01a can unblock review of representation and structural invariants. W02-W04 still
need the producer contract for source/provenance, outcomes, completeness and
accuracy, plus an agreed kernel/solidcore boundary. Persistent Drawing files,
fonts, profiles and PDF are not prerequisites for standalone section work.

[issue]: https://github.com/tangency-labs/solidcraft/issues/2053
[pr]: https://github.com/tangency-labs/solidcraft/pull/2085
[plan]: https://github.com/tangency-labs/solidcraft/issues/860#issuecomment-6234779815
[producer-pr]: https://github.com/tangency-labs/solidcraft/pull/2068
[section-source]: ../crates/geom/src/section.rs
[tests-source]: ../crates/geom/src/section_tests.rs
[plane-source]: ../crates/geom/src/plane.rs
[segment-source]: ../crates/geom/src/profile.rs
[section-command]: ../crates/engine/src/cmd/section.rs
