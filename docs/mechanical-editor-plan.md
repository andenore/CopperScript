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

9. [ ] Add source spans for declarations/properties including comments/trivia,
   encoding and newline preservation. Tests for nested/imported/hierarchical files.
10. [ ] Build targeted patch operations for existing constraint properties and
    deterministic new declarations. Detect ambiguous/multiple/unowned rules.
    Initial slice implemented: immutable UTF-8/BOM snapshots, token-end spans,
    revision-bound candidate patches for direct board-owned fixed placements,
    comment/newline preservation and shared/duplicate ownership rejection.
    General declaration indexing and hierarchical target mapping remain open;
    these pure syntax-checked candidates are not exposed as a Save operation.
11. [ ] Add position-only, rotation-only, side and full-pose lock editing with
    explicit conflict resolution; move source-locked parts only in edit-lock mode.
12. [ ] Map resolved instance targets to valid source references. Do not edit
    imported library content or freeze all generated placements.
13. [ ] Compile/physicalize prospective edits without overwriting disk; validate
    mechanics, electrical equality and physical constraints. Review exact diff.
14. [ ] Atomic revision-checked save, source undo/redo and external-change reload
    conflict handling. Persist intended locks only; reset/recompute movable seeds.
15. [ ] Mark route/fill/manufacturing outputs stale after applicable source changes;
    rebuild from source, no incremental copper retention in the first version.

Acceptance: edit connector lock, save, reopen and route with unchanged electrical
connectivity and locked pose. Comments survive; stale/concurrent edits never win.

## C. Mechanical authoring

16. [ ] Circle/rectangle dimensions and polygon vertex tools; holes and polygonal
    cutouts with stable feature selection and exact source units.
17. [ ] Screw-head placement clearances, side-specific placement/copper keepouts
    with distinct previews. Plated holes remain component/net-owned.
18. [ ] Measurement and dimension snapping; reasons for illegal geometry/poses.
    Validate topology, material containment, courtyard/hole/copper clearance.
19. [ ] Undo/redo and safe source persistence for mechanical declarations. Native
    KiCad outline/drill/export regressions; full routed demo using generic code.

## D. Responsive jobs and editor integration

20. [ ] Asynchronous/cancellable auto-placement with revision-bound progress and
    resource budgets. Incremental incident-net ratsnest refresh and profiling.
21. [ ] Selected-net/component and side/power filters, labels, optional net costs;
    arbitrary copper-island terminals and validated filled-zone overlay extension.
22. [ ] Routed-copper overlay with source/physical digest, stale display and explicit
    remaining-connectivity evidence; never infer fill from an outline.
23. [ ] Shared browser core hosted by VS Code, document selection/source links,
    native undo/save, workspace trust and pinned compiler configuration.
24. [ ] Accessibility/keyboard editing, large-board performance and release assets.

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
