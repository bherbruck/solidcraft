# Drawing foundations: first geometry slice

This is the W00 bootstrap ledger for the [published Drawing plan][plan] in
[#860][rfc]. The first implementation is W01a, [#2053][w01-issue] /
[PR #2085][w01-pr], owned by `0xlillith`: neutral section-geometry records,
structural validation and conversion to existing planar types. Its concrete API
and limitations are in [the geometry contract](drawing-geometry.md).

W01a is part of W01, not completion of W00, the full W01 contract, Stage A, or the
Drawing workspace. It does not extract a body section, calculate visibility,
register `inspect.section_curves` or `drawing.view`, export a drawing, or save a
Drawing document. The independent formatter slice W09a is tracked in
[#2054][w09-issue] / [PR #2086][w09-pr]; it is not a module supplied by W01a.

## Scope, sources and status

The initial scope was checked against upstream `f7343e9161bc6243cdb076d3b6c3594db97279ff`.
Recheck the implementation revision, current claims, [AGENTS](../AGENTS.md) and
[CONTRIBUTING](../CONTRIBUTING.md) before taking the next slice. Local `plan/`
documents were unavailable when this work began; this file does not reconstruct
them or replace project instructions.

The published plan reconciles the [original RFC][rfc],
[SychPL's investigation][visibility], [NOWN-DEV's two-stage proposal][fabrication]
and CamWight's [study and questionnaire][study-comment]. The [attachment][study]
retains the detailed requirements, alternatives and sources. Product research is
requirements evidence; its descriptions and proposed standards rules are not
implementation code, independently verified conformance or permission to copy
protected content.

Here, **W01a** means code in this slice; **future** means a requirement still to
implement or decide. A merged representation or validator does not establish a
geometry producer's capability. Drawing milestones in the RFC are distinct from
the repository roadmap's M0-M14; the Drawing workspace remains roadmap M11.

## First-section requirements and fixtures

Fixture IDs below identify checks, not saved designs or registered commands.
The current fixture details and future producer gates are in
[the geometry contract](drawing-geometry.md#fixtures-and-handoff).

| ID | Requirement and source | Package | Fixture / status |
| --- | --- | --- | --- |
| S01 | Model mm/radians, explicit plane axes and correct XZ sign; plan §4.1/W01 | W01a | F01: plane validation and coordinates; W01a |
| S02 | Reuse supported analytic/conic/Bézier representations without silently fitting them; plan §4.1/W01 | W01a, then W02-W03 | F02: representation/serde checks; W01a. Producing exact sections is future |
| S03 | Keep accuracy categories explicit; plan §4.1/W01 | W01a, then W02-W03 | F02/F04: analytic/bounded/sampled metadata and finite values; W01a. Evidence for producer bounds is future |
| S04 | Indexed curves, oriented wire uses and region references; plan W01 and study §§10-12 | W01a | F03: manually constructed rectangle with a hole, orientation-preserving conversion; W01a |
| S05 | Certify closed section boundaries, winding, nesting, material regions and areas; plan W03 | W03 | F05/F06: body-derived hole/oblique-cylinder sections; future. PR #2068 proposes producer tests for review. Structural validation alone does not meet this requirement |
| S06 | Empty, tangent, open, coincident, degenerate and unsupported cuts are distinguishable; plan W03 | W01 follow-up, W02-W03 | F07: producer outcomes; future. Current empty vectors/contact points are only representable data |
| S07 | Complete source/body/occurrence identity, provenance and freshness; plan §4.1/W01, NOWN-DEV Stage A | W01 follow-up, W02-W04 | F08: repeated placements and failure in one selected body; future. Current indices are result-local |
| S08 | Bound input/work and fail without mutation or partial success; project rules, plan W01-W04 | W01a, then W02-W04 | F04: structural bounds; W01a. Byte/allocation limits, producer work budgets and cancellation are future caller responsibilities |
| S09 | Standalone read-only `inspect.section_curves`, existing plane forms, explicit selection and no sketch/viewport prerequisite; NOWN-DEV Stage A | W04 after W03 | F09: command/CLI/MCP parity, unchanged document/undo and error cases; future |
| S10 | SVG/DXF with units, source groups/layers, declared format accuracy and atomic optional file output; NOWN-DEV Stage A | W04, W10 geometry subset | F09: physical coordinates and complete output; future |
| S11 | One section provider shared with the visibility investigation; SychPL/NOWN-DEV and plan W03/W05 | W02-W06 | F05-F08 and a reviewed capability matrix; future |
| S12 | Transient dimensioned snapshots remain distinct from associative drawings; NOWN-DEV Stage B | W07-W08 | Future snapshot/measurement/output fixtures after their contracts settle |

The remaining industrial requirements are retained in the published plan's
coverage ledger and W09-W47: formatting, persistent sheets/references, dimensions,
BOM/balloons, standards, derived views, production output and specialist domains.
Expanding this bootstrap into atomic traceability for all 48 packages is remaining
W00 work; absence from this first-section table does not remove a requirement.

## Dependencies, ownership and review

Claim links below identify the bounded slices already tracked here. Other rows do
not reserve work or assign the contributors who proposed it. Check live issues and
draft PRs before claiming. Review columns name the expertise needed, not reviewers
who have already agreed to an assignment.

| Package | Depends on | Ownership / scope here | Review and next handoff |
| --- | --- | --- | --- |
| W00 | RFC and project rules | Bootstrap in W01a; full ledger/decision ownership remains open | Integration review: preserve sources, unresolved decisions and child-issue boundaries |
| W01 | W00 immediate scope | `0xlillith`, [#2053][w01-issue]/[#2085][w01-pr], W01a only | Geometry/kernel and engine review: validate representation limits; finish source/provenance/outcome contracts before producer handoff |
| W02 | Reviewed W01 producer contract and backend placement | Review overlap with NOWN-DEV's [#2068][section-pr] before another claim | Kernel/solidcore review: guarded live-body adapter, supported surfaces, complete source identity and no cache-byte authority |
| W03 | W02 | Review/reuse producer proposal [#2068][section-pr]; exact supported subset remains to be agreed | Geometry/document review: analytic cuts, classified outcomes, winding/nesting/material checks and error evidence |
| W04 | W03 and W10 geometry-only subset | NOWN-DEV's [#2068][section-pr] proposes Stage A command/output; not merged at the recorded review | Engine/IO review: explicit selection, request limits, no mutation, physical SVG/DXF, DTO adaptation and complete-or-error output |
| W05 | Reviewed W01; W02 where extraction is needed | Coordinate with [SychPL's offer][visibility]; this is not an assignment | Geometry/kernel review: bounded HLR investigation and findings in the RFC before a substantial backend proposal |
| W06 | Reviewed W05 findings and W02 | Future projection/visibility slice | Geometry/document review: supported scene-wide visibility, split intervals, provenance and cache keys |
| W07 | W01, W09; W03 for cuts; an agreed preview adapter or W06 | Future transient `drawing.view` | Engine/presentation review: meaningful measurements, snapshot status and explicit approximation |
| W08 | W07 and W10 text/dimension subset; W04 adapters as reused | Future preview and SVG/DXF/PNG slice | UI/IO review: shared navigation/layout, output scale and UI/CLI/MCP parity |

Stage A does not depend on persistent Drawing storage, PDF, fonts, a drafting
profile or the W09 formatter. W10's geometry-only primitives are a separate
increment from its text/dimension layout. W09a's [independent PR][w09-pr] can advance
in parallel without introducing a dependency into W01a.

NOWN-DEV's [PR #2068][section-pr] is an overlapping Stage A producer/export
proposal to review before further W02-W04 claims. It was open and ready, not
merged, when inspected at head `4e945d8af594b8cc252cc4e5ffede3684b7bf3bd` on
2026-10-17. It proposes `doc/project_section.rs` and the
`inspect.section_curves` engine command with mesh/fitted loops and SVG/DXF.
Its body-name/loop JSON and `sweep_deg` angles do not imply compatibility with
W01a's radians, accuracy records and indexed regions. Agree an explicit adapter,
accuracy and source/provenance contract before integration; preserve that
contributor's claim and branch. Its tilted-cylinder test is proposed producer
evidence to review, not an implemented W01a or current-main capability.

## Decision ledger

An **implemented slice choice** below describes W01a, not approval of every
proposal in the RFC. Record later decisions with their evidence, reviewer, date,
affected contract revision and dependent child issues.

| Decision | Current disposition | Next owner/review and gate |
| --- | --- | --- |
| D01: representation placement | W01a uses existing `geom::Plane`, `Seg2`, `Loop2` and `Region2`; neutral section records live at L0 | W01a geometry review. This does not decide the section/HLR algorithm's kernel placement |
| D02: identity/status scope | W01a has only indices within one value and per-curve accuracy metadata; no persistent source names, provenance or freshness record | W01 follow-up with kernel/document/engine reviewers before W02-W04 depend on source contracts |
| D03: validity claim | W01a validates structure and preserves supplied orientation. It does not certify winding, nesting, self-intersections or a material section | W03 owns producer/geometric validity evidence; callers must not promote structure checks into manufacturing certification |
| D04: shared producer/backend | One reusable section provider is the plan; guarded extraction and substantial HLR placement must be coordinated with [solidcore][solidcore] | Kernel/solidcore and integration review before W02/W05 expand into a substantial backend |
| D05: supported geometry/accuracy | Representation support is documented; producer surface/curve coverage and defensible approximation bounds remain future | W02-W03 publish per-case capabilities and F05-F08 results; W05 does the same for visibility |
| D06: public commands | [#2068][section-pr] proposes `inspect.section_curves`; transient `drawing.view` remains future. W01a registers neither | Review #2068's distinct output schema before W04 integration; live registry/claim check in W04/W07; preserve existing viewport commands |
| D07: later Drawing choices | Storage/undo, PDF/fonts, standards/profile semantics and release rules remain in the RFC decision register | Their relevant later packages decide them; none blocks this representation slice or standalone section work |

## Completion and handoff

Each subsequent child issue should state supported cases, owned paths, contract
revision, exact dependencies, analytic expectations, error behavior and required
cross-lane review. Follow current draft-claim, isolation and quality-gate rules;
use `Refs #860` and close the bounded child issue rather than the umbrella RFC.

Report what was actually tested at the proposed merge revision. A structure-only
API has no new CAD result to photograph; later producer, command and UI changes
need real app evidence when their effect can be shown. Keep generated reference
documentation, assets and native-format changes outside this slice. Root project
instructions govern any later change to those areas.

[rfc]: https://github.com/tangency-labs/solidcraft/issues/860
[plan]: https://github.com/tangency-labs/solidcraft/issues/860#issuecomment-6234779815
[visibility]: https://github.com/tangency-labs/solidcraft/issues/860#issuecomment-6167030912
[fabrication]: https://github.com/tangency-labs/solidcraft/issues/860#issuecomment-6231770565
[study-comment]: https://github.com/tangency-labs/solidcraft/issues/860#issuecomment-6216908058
[study]: https://github.com/user-attachments/files/33548772/inventor-drawings-research.md
[w01-issue]: https://github.com/tangency-labs/solidcraft/issues/2053
[w01-pr]: https://github.com/tangency-labs/solidcraft/pull/2085
[w09-issue]: https://github.com/tangency-labs/solidcraft/issues/2054
[w09-pr]: https://github.com/tangency-labs/solidcraft/pull/2086
[section-pr]: https://github.com/tangency-labs/solidcraft/pull/2068
[solidcore]: https://github.com/tangency-labs/solidcraft/issues/1597
