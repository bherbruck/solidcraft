# Durable icon maintenance — implementation plan

The goal is for a new contributor or agent, without any chat history, to produce
icons that match the finished set and to find every icon that still needs review.
The SVG masters and valid acceptance records must survive this work unchanged.

## 1. Establish ownership and clean the resource tree

- Keep production assets, font licences, platform exports, editable native source
  documents and the original SVG masters. Never remove an asset used by packaging.
- Retire superseded numbered authoring batches: they contain duplicate functions,
  earlier revisions and implicit overrides. The SVGs are the editable authority.
  Keep a reusable geometry library, explicit bases and a new-icon scaffold instead.
- Remove the generated inventory CSV from the source collection; generate reports
  under ignored `target/`. Remove the empty proposal-era stand-in list.
- Remove the unused historical macOS raster/SVG exports while preserving their
  editable VectorCraft document. Keep a local recovery copy under `target/`.
- Update attribution alongside every resource addition, retirement and move.

## 2. Give future authors an unmissable specification

- Link the rules from root AGENTS.md, assets/AGENTS.md and icons/ui/AGENTS.md.
- Specify the categories, exact palette and weights, isometric projection and
  face shading, canonical cube/slab bases, action arrows, points, holes, badges,
  overlap order, detail limits and optical padding.
- Name finished SVG references for each category and explain what to copy from
  their visual language. Commercial artwork remains forbidden.
- Provide a safe starter command that refuses to overwrite an existing icon,
  and a validation command that checks attribution, structure and coverage.
- Record a short first-pass acceptance rubric and the complete review workflow.

## 3. Preserve a small, repository-owned review application

- Keep the existing Python entry point and loopback URL, requiring only Python 3.11+.
- Split the page, CSS and JavaScript into readable versioned source files.
- Discover SVG masters, aliases, command references, nested UI callers and known
  wrapper/table arguments on every launch and explicit rescan. Show unused masters.
- Every discovered, non-final icon is Open, including icons with notes. Missing
  artwork is visible and cannot be accepted. Report unassigned commands separately.
- Final means the current artwork matches its saved hash. Changed artwork returns
  to Open and can be accepted again deliberately; never silently refresh its hash.
- A normal opening starts at Open / Everything with an empty search. Retain theme
  and size preferences; preserve temporary filters only for a live artwork reload.
- Keep autosave, light/dark, small-size variants, notes and portable JSON export.
  Export queues neutral artwork feedback locally; it does not invoke a model.
- Preserve notes edited since an export, stale-page merge behavior and accepted art.

## 4. Verify the basic implementation

- Test discovery of newly added and unused masters, aliases, missing artwork,
  changed acceptance hashes, nested UI callers, stale saves and exact export notes.
- Check the web UI in both themes and at small sizes. Check the style-reference
  page and scaffold against the finished geometry.
- Run focused checks in the background; report actual results. Full repository CI
  is still required before a local commit. Existing engine robustness failures do
  not authorize changing unrelated geometry or accepting failed quality gates.
- Keep this iteration local; do not push or change the existing draft PR #853.

## Scope of this first implementation

No icon redesign, palette change, packaging change, new approval state database,
cloud service, model-specific automation or replacement of Rust viewport graphics.
Style quality still needs visual review; lint cannot decide whether a new symbol
communicates its operation well.

## Basic version implemented

- The media collection retains the production SVGs and platform assets. The
  superseded bulk generators, generated inventory, empty stand-in list and unused
  historical exports have been retired; the historical editable native master remains.
- Mandatory instructions are linked from root, assets and UI-specific AGENTS.md.
  The style guide, curated references and reusable bases are included in the repo.
- The review interface is split into versioned HTML/CSS/JavaScript, with discovery
  on opening/rescan, valid-hash acceptance, visible shared masters, atomic state
  saves, exact local exports and protection against stale acceptance and note loss.
- `cargo xtask icons` and the CI repository-check job enforce the icon checks and
  regression tests. All existing SVG bytes and initial acceptance hashes were preserved.

Remaining limits: source discovery recognises the documented literal call patterns,
not every possible Rust expression. Add a fixture when introducing a new icon
wrapper convention. Visual quality, semantic clarity and acceptance remain human
review decisions; the tool does not automatically redraw or finalise artwork.
