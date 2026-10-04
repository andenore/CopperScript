# Mechanical editor implementation checklist

The [specification](mechanical-editor.md) defines the contract. Check boxes mean
implemented and tested, not merely planned. Commit major completed increments.
No board-specific data or algorithms belong in `pcbir`.

## A. First preview vertical

1. [x] Add derived scene protocol: real footprints, transformed pads/courtyards,
   true circle/polygon boundaries, cutouts, holes, keepouts, hierarchy/lock/macro
   metadata, source/session revision and capability/warning fields.
2. [x] Add deterministic pad-island ratsnest using shared explicit copper roots
   and nearest-pad Euclidean MST. Cover duplicate numbers without internal edges,
   internal groups and existing explicit copper; no zone-outline connectivity.
3. [x] Add immutable accepted/pending session states, revision-checked operations,
   auto-place preview/apply/discard, temporary full-pose locks and bounded history.
   Reuse planner and legality checks; failed/stale previews preserve accepted state.
4. [x] Add whole-unit manual pose preview. Check fixed source/session poses,
   explicit angles/sides, material/courtyard rules and rigid/macro ownership.
   Source locks stay read-only until persistent edit transactions exist.
5. [x] Add loopback-only service with Host/Origin/capability validation, bounded
   JSON, no arbitrary paths/shell or external assets, serialized operations.
6. [x] Bundle accessible SVG viewer: front/rear visibility, reference selector,
   zoom/fit, mechanical features, numeric pose/drag preview, ratsnest/net/selected-
   component filters, source/session lock distinction, apply/discard/undo/redo.
   Clearly label read-only source and unsupported mechanical editing.
   Follow-up: checked drags now apply immediately by default (optional previews),
   wheel zoom anchors under the pointer, right drag pans, and conflict errors
   refresh the accepted revision rather than leaving subsequent edits blocked.
7. [x] Add CLI launch and deterministic scene export, locked/offline/root/profile
   settings and explicit template/macro loading. Default to real footprints;
   proxy inspection requires explicit opt-in and visible warnings.
8. [x] Unit/HTTP/CLI/packaging tests, browser smoke and real-footprint LED-ring
   inspection. Record commands/results and first-delivery limitations.

Acceptance: open a real board, see its true outline and footprints, preview and
apply rough placement without implicitly locking components, adjust legal movable
poses, toggle/filter ratsnest, retain source locks, undo/redo, and prove no source
bytes were modified. Macro-only connected nets produce no artificial airwires.

## B. Safe persistent source editing

9. [x] Add source spans for declarations/properties including comments/trivia,
   encoding and newline preservation. Tests for nested/imported/hierarchical files.
10. [x] Build targeted patch operations for existing constraint properties and
    deterministic new declarations. Detect ambiguous/multiple/unowned rules.
    Token patches preserve UTF-8/BOM, comments and newlines; imported/profile
    owners and shared/duplicate constraints are rejected for explicit resolution.
11. [x] Add position-only, rotation-only, side and full-pose lock editing with
    explicit conflict resolution; move source-locked parts only in edit-lock mode.
12. [x] Map resolved instance targets to valid source references. Do not edit
    imported library content or freeze all generated placements.
13. [x] Compile/physicalize prospective edits without overwriting disk; validate
    mechanics, electrical equality and physical constraints. Review exact diff.
14. [x] Atomic revision-checked save, source undo/redo and external-change reload
    conflict handling. Persist intended locks only; reset/recompute movable seeds.
15. [x] Mark route/fill/manufacturing outputs stale after applicable source changes;
    rebuild from source, no incremental copper retention in the first version.

Acceptance: edit connector lock, save, reopen and route with unchanged electrical
connectivity and locked pose. Comments survive; stale/concurrent edits never win.

## C. Mechanical authoring

16. [x] Circle/rectangle dimensions and polygon vertex tools; holes and polygonal
    cutouts with stable feature selection and exact source units.
17. [x] Screw-head placement clearances, side-specific placement/copper keepouts
    with distinct previews. Plated holes remain component/net-owned.
18. [x] Measurement and dimension snapping; reasons for illegal geometry/poses.
    Validate topology, material containment, courtyard/hole/copper clearance.
19. [x] Undo/redo and safe source persistence for mechanical declarations. Native
    KiCad outline/drill/export regressions; full routed demo using generic code.

## D. Responsive jobs and editor integration

20. [x] Asynchronous/cancellable auto-placement with revision-bound progress and
    resource budgets. Incremental incident-net ratsnest refresh and profiling.
21. [x] Selected-net/component and side/power filters, labels, optional net costs;
    arbitrary copper-island terminals and validated filled-zone overlay extension.
22. [x] Routed-copper overlay with source/physical digest, stale display and explicit
    remaining-connectivity evidence; never infer fill from an outline.
23. [x] Shared browser core hosted by VS Code, document selection/source links,
    native undo/save, workspace trust and pinned compiler configuration.
24. [x] Accessibility/keyboard editing, large-board performance and release assets.

## E. Advanced mechanical intent

25. [ ] Named datums and stable edge IDs in source/IR; numeric and edge-relative
    component/pad/mating-face attachment with dependency/conflict checks.
26. [ ] Explicit body-overhang allowances independent of copper-edge requirements;
    component/enclosure-height and assembly-access metadata where available.
27. [ ] Exact line/arc/rounded outlines and routed slots through shared queries,
    source editing, physical IR, KiCad and manufacturing export before UI exposure.
28. [ ] Locked DXF/enclosure reference overlays with explicit units/transforms,
    whitelisted entities and no silent healing; optional 3D inspection later.

## Verification record

Populate after each delivered slice with test commands, pass counts, inspected
board/source revision, browser actions and explicit remaining limitations.

### First preview increment — 2026-10-04

- 39 editor tests pass: scene/transform/island MST, internal groups, macro copper,
  whole-unit moves, lock retention, preview CAS, undo/redo, malformed/stale requests,
  HTTP security and CLI source preservation.
- Broader suite: `pytest tests/test_mechanical_editor.py tests/test_cli.py
  tests/test_layout.py tests/test_mechanical_language.py
  tests/test_internal_pad_connections.py tests/test_rigid_clusters.py` — 142 pass.
- `uv build --wheel --out-dir build/editor-wheel`: wheel includes Python editor
  modules and all bundled UI assets. No new runtime dependencies.
- Headless installed Chrome + Playwright smoke passed the real-footprint demo:
  ratsnest toggle/net/side filters, auto-place preview/apply, manual 45-degree pose,
  temporary lock, undo/redo, live airwire drag and discard; zero browser page errors.
  Screenshot inspected at ignored `build/editor-demo.png`. Interactive automation
  helpers were unavailable, so this was a normal headless browser regression.
- Real CopperLedRing source (SHA-256
  `c2820b7bb9bbd576815a3ab3f9bbe948c3cd2f861266afea02920870946c487d`)
  successfully exports a locked/offline 34-component scene using installed KiCad
  footprints and pinned CopperLib. Visual LED-ring editor smoke remains open in
  item 8; this does not replace the existing routing/native-DRC evidence.
- First increment limits: no source saving or mechanical authoring; temporary
  full-pose locks only; source locks read-only; shared unmirrored rear coordinates;
  conservative body envelopes; complete-board pose gate gives a combined rejection
  rather than per-rule diagnostics. Live drag shifts airwire endpoints and recomputes
  the exact graph/tree on release. Macro materialization failures are explicit
  warnings with no private-copper credit. Async/cancel/progress, copper overlays
  and copper-keepout rendering remain later work.

### Continuous editing/navigation and source-patch foundation — 2026-10-04

- Default drags now validate then apply in one UI operation using the existing
  revision-bound preview/apply protocol. Undo remains available. Explicit drag
  previews are opt-in; numeric/auto-placement previews retain Apply/Discard and
  show a blocking-state banner. Revision conflicts reload accepted state.
- Headless installed Chrome smoke passed consecutive drags, live airwires,
  explicit preview/discard, cursor-anchored wheel zoom, right-button panning
  starting on a footprint without changing its pose, and second-client conflict
  recovery. Existing auto-placement, 45-degree rotation and history checks pass;
  zero page errors. Screenshot: ignored `build/editor-interaction.png`.
- Real LED-ring browser inspection passed: true circle, 33 front/1 rear
  components, filters and wheel/fit; zero page errors, no mutations. Screenshot
  inspected at ignored `build/editor-led-ring.png`; source SHA remains the value
  recorded above. This closes item 8's visual inspection gap.
- Source patching has 35 tests: byte-exact UTF-8/BOM/newline/comment preservation,
  scalar token offsets/escapes, deterministic insertion, compile/electrical
  equality probe, stale/overlapping edit rejection, ambiguous/shared constraints,
  imported-content isolation and invalid pose values. No save endpoint or source
  mutation was added; items 9–15 remain incomplete as detailed above.
- Updated wheel builds with bundled UI assets and the new source module. No
  runtime dependencies added.
- Final regression command: `pytest tests/test_editor_source.py
  tests/test_mechanical_editor.py tests/test_compiler.py tests/test_cli.py
  tests/test_layout.py tests/test_mechanical_language.py
  tests/test_internal_pad_connections.py tests/test_rigid_clusters.py
  -q -o addopts=''` — 181 passed.

### Persistent editing, mechanics and bounded placement — 2026-10-04

- Reviewed source save, independent position/rotation/side locks, resolved `/`
  targets, imported-owner conflict rejection, exact diff, source history and
  external reload are implemented. Only intended locks persist; automatic poses
  remain movable seeds. UTF-8 BOM, CRLF/LF, comments and unrelated bytes survive.
- Initial mechanical palette supports exact circle/rectangle/polygon dimensions,
  clicked snapped vertices, stable hole/cutout/keepout IDs, screw-head clearance,
  side/layer scopes and measurement. Prospective compilation checks electrical
  equality, full placement legality and pad/drill/keepout geometry before save.
- Regression suite covering source patches/transactions, editor, placement,
  profiles, physical DRC, compiler and mechanical language: **212 passed**.
  The transaction suite includes a native KiCad circle/2.4 mm NPTH round-trip.
- Installed Chrome/Playwright smoke passed source-lock review/save, hole editing,
  persistent undo/redo and measurement with zero page errors. Final undo restored
  every original source byte. Screenshot: `build/editor-save-smoke/editor.png`.
- Shared Make `EXAMPLE=mechanical-editor route` passes full routing and native
  KiCad 10.0.6 refill/DRC: two nets, **zero violations and zero opens**.
  `build/editor-native-20261004` contains the profiled run and Gerber/drill export.
- Placement uses a cancellable spawned process, 1–300 second wall-time budget,
  bounded component count, phase/candidate progress and cumulative profiling.
  Manual edits cancel older work; stale source/results cannot apply. Five process
  and cache tests pass, including actual cancellation and timeout termination.
- Incident-net ratsnest cache is exactly equivalent to full recomputation; copper,
  macro and inventory changes invalidate it. Browser save smoke also passes with
  asynchronous auto-placement. Sections D (21–24) and E remain open.
- Final affected regression including real placement processes: **217 passed**.
  The wheel builds with editor modules/assets and no added runtime dependencies.
- CopperLedRing's `make edit` is wired to this shared editor. Real-footprint
  browser inspection passed its circle, 33 front/1 rear parts and navigation with
  zero page errors and no source mutations. Its public compiler pin update is
  pending publication approval; development verification used `EDITOR_PYTHON`.

### Routed reference overlays and net filters — 2026-10-04

- Compiler emits `board.editor-intent.json`; generic Make build binds its digest
  to `run.json`. Explicit native Python probe reads tracks, vias and **actual**
  filled-zone outer/interior rings, never rule areas or unfilled zone boundaries.
- `editor-overlay` checks routed poses (including native hierarchy reference
  mapping), exact copper, saved PCB and native DRC hashes. The capsule is bound
  back to its run manifest. No executable or filesystem paths come from JSON.
- Editor reconstructs router poses as movable seeds only when source, electrical
  dependency identity and physical projection match. Changed source/imports,
  footprints, rules or poses visibly stale the overlay and revoke connectivity
  credit. Source editing never retains copper in authoritative physical intent.
- Remaining native opens remain visible; missing native evidence gives explicit
  track/via-island ratsnest only. Matching zero-open native evidence can close
  presentation airwires, without claiming manufacturing signoff.
- Signal/supply, selected component/net, side and copper-layer filters, airwire
  labels and optional straight-line MST cost estimates are implemented.
- Affected suites: **116 passed, 6 skipped**. Includes 20 overlay regressions and
  a real KiCad filled-plane/NPTH exclusion test. Browser smoke passes layer
  visibility, stale-preview airwires, filters, labels/costs and discard recovery;
  zero page errors. Inspected screenshot: `build/editor-overlay-20261004/editor.png`.
- Fresh generic Make demo route: **zero native violations and zero opens**;
  bound reference generated with installed KiCad 10.0.6. Items 23–28 remain open.

### Native VS Code document host — 2026-10-04

- Optional custom text editor under `integrations/vscode`, staged with the same
  bundled web assets by `node integrations/vscode/build.cjs`. Local trusted
  workspaces only; compiler command is argv/no shell and an exact content digest
  is required. Source, imports and footprint resolution remain locked/offline.
- Bounded JSON-lines backend compiles the current unsaved TextDocument in memory.
  Prospective edits return reviewed minimal UTF-16 text spans, never filesystem
  source writes. Native WorkspaceEdit/version guards, Save, Undo/Redo, dirty-save
  failures, invalid-buffer recovery and source declaration links are supported.
- **19 document-host tests pass**; regression includes real subprocess protocol,
  stale review rejection, monotonic versions, Unicode/CRLF span handling and
  filesystem-write bypass prevention. Existing standalone editor regressions pass.
- Installed VS Code **1.140.0** Extension Development Host passes the actual
  custom editor, pinned compiler, rough placement worker, reviewed native save,
  native source Undo, invalid unsaved text recovery and declaration navigation.
  Test uses disposable `build/vscode-smoke-workspace`; no user workspace settings
  or sources are changed. Items 24–28 remain open.

### Keyboard access, exact spatial ratsnest and release assets — 2026-10-04

- Accessible footprint buttons with roving focus, selection and coordinate labels;
  keyboard arrows/Shift snaps, next allowed rotation, zoom/fit and Escape. Partial
  source locks restrict only their owned pose dimensions, not unrelated movement.
  Chrome keyboard smoke passes a legal 45-degree rotation with unchanged source.
- Exact spatial Boruvka MST uses bounding boxes and uniform-component pruning;
  no approximate-neighbour connectivity. Twenty randomized/tie-order cases match
  exhaustive Kruskal exactly, including shuffled input and coincident terminals.
  A 2,500-terminal grid uses **69,124** distance tests (two rounds, ~0.83 s on the
  verification host), versus **3,123,750** all-pairs tests. Worst-case work can
  still be quadratic; memory is linear. Unchanged status polls cache physical
  scene transforms/full legality rather than recomputing them.
- Deterministic, offline, whitelisted VSIX packaging passes reproducibility and
  exact shared-asset tests. Installed VS Code accepts the generated artifact in
  an isolated profile. Existing tag CI now includes it and the compiler fingerprint
  in inspection release assets; ordinary pushes preserve editor artifacts too.
  This is packaging configuration, not a claim that CI has run remotely.
- Broad source/editor/placement/profile/DRC/compiler/build regressions:
  **305 passed, 6 skipped**. Wheel builds with shared modules/assets and no new
  runtime dependencies. Items 25–28 remain open.
