# Current backlog

Read only when planning unfinished work. Focused backlogs own acceptance criteria;
check current code and coverage before treating an item as an implementation gap.
Routing closure does not by itself close manufacturing or electrical gates.

## LLM efficiency

See [the future improvement plan](docs/llm-workflow-improvements.md): measured
token/intervention evaluation; reviewed reusable bundles, caching and transitive
version policy; remaining final-copper verifiers and broader acceptance.
Fabrication/prototype measurements remain deferred until a later request.

## Solver and qualification work

Choose only the relevant focused record for detailed acceptance criteria:

- [Escape planning plan](docs/escape-planning-plan.md): joint escape side, layer
  and order per package and package pair (steps E0–E6), with its status table.
- [Routing backlog](docs/routing-backlog.md): access/ownership, joint decoupling,
  ordinary topology, bounded quality and path extraction.
- [Routing quality contracts](docs/routing-quality-todo.md): implemented behavior
  that must survive future changes.
- [AssetTracker visual review](docs/assettracker-review-plan.md): preferred power
  width, pad-aware exits, short RF access and source-matched antenna corrections.
- [Routing optimization](docs/routing-optimization-todo.md): profiled bottlenecks
  and guarded comparisons; preserve output quality and evidence.
- [Qualification gaps](docs/qualification-todo.md): geometry adapters, derating/
  thermal extraction, native release integration and mandatory external evidence.
- [Other contributor work](docs/contributor-backlog.md): mechanical/editor,
  electrical-model, simulation and architecture gaps.

## Additional routing and asset scope

- [x] Add a separate power/ground copper-area planning stage shared by placement
  and routing. Plan rail partitions, ground continuity, local bypass/return vias,
  sensitive-node exclusions and reserved signal corridors before detailed tracks;
  feed infeasible access back to placement. Verify actual native-filled fragments
  and return paths, with bounded trials, exact ownership and complete rollback.
  A zone outline or same-net label must never count as connectivity or RF proof.
  See [the stage requirements](docs/routing-backlog.md#separate-powerground-copper-area-planning-stage).
  The opt-in stage, physical proposals and explicit-model DC extraction are now
  implemented.
- [ ] Complete expanded CopperProbe acceptance of the power-area stage: all
  FX10 domains with active USB routes, ADC/analog preservation and the gated
  normal full-board comparison. Record failures and measured work bounds in
  [the implementation plan](docs/power-area-planning-plan.md).
  Native return preparation and complete owner rollback are integrated. The
  retained pre-correction preparation connects all eleven selected FX10/ADC domains,
  all 63 bypass associations and all 112 selected return lands, including
  ADC ground pad 49 and FX10 N4/N6/N8. All fifteen critical nets remain joined;
  native refill has zero other violations and zero islands, with 183 remaining
  whole-board unconnected items. Source intent and fabrication rules remain
  unchanged. The scoped package handoff has no pending pads and 103 allocated
  boundary ports, with fresh native preservation. Handoff into global guides and
  the normal full-board comparison remain acceptance gates. Full ordinary
  preflight now identifies twelve pending exits; its unadopted proposal adds
  62 dangling vias and 24 dangling tracks and is rejected. Joint ordinary/power
  contact reservation and the three remaining global guide failures must be
  resolved before full detailed routing. Those eleven-domain figures belong to
  the earlier source: the 2026-10-18 package-support correction changes capacitor
  poses and invalidates that certificate for the new source. The corrected source
  now has fresh eleven-domain native preparation on all six layers, with all
  63 bypass associations and 112 selected returns connected, 183 whole-board
  opens, zero other native findings and zero islands. The 2026-10-19 complete
  run uses 50 candidates, 2320 allocator states and sixteen repairs; the final
  guarded controller replay produces identical accepted geometry. Complete
  85-contact geometry acceptance remains separate from real signal exits and
  the wider routing gates, which are still open.
  The fresh current-source ordinary handoff has thirteen pending pads: ten
  FX10 boundary exits plus ADC DFS/OE and converter EN fanout. Its unadopted
  proposal adds 78 dangling vias and sixteen dangling tracks; every accepted
  power/return join remains connected. Complete joint layer/boundary selection
  and source-backed control-terminal approaches before detailed routing.
- [ ] Add explicit source layer roles/dedicated-plane semantics beyond coverage
  inference, preserving existing zone/stitching/per-pad controls.
- [ ] Implement coherent whole-package fanout planning before mutable local
  feeds: geometry-derived dogbone patterns, joint signal/power/ground contacts,
  verified layer exits and coordination with support placement and copper areas.
  Reuse bounded local assignment and exact ownership; preserve fixed intent,
  critical macros and required via arrays. Follow work packages P0-P5 and the
  small-to-full consumer gates in
  [the implementation plan](docs/power-area-planning-plan.md#whole-package-fanout-planning-implementation-plan).
  P1 proposals, coupled support placement and source-bound power-preparation
  integration are implemented behind explicit bounded options. Moving 22 FX10
  capacitors clears all 85 eligible contacts in the full held consumer scene.
  Fresh authored-source eleven-domain preparation passes selected native
  supply/return checks; the previous backside-land blocker is resolved. Joint
  layer-exit selection, full ordinary handoff and full-board acceptance remain
  pending. Contact patterns are not complete routed escapes or electrical proof.
- [x] Add hierarchical module-to-macro frontend selection following the
  [source syntax plan](docs/physical-macro-syntax-plan.md).
- [x] Author named hard-macro definitions in Copper, following the
  [requirements and implementation plan](docs/macro-definition-plan.md).
- [ ] Extend movable-macro feedback with complete boundary/owner revalidation.
- [ ] Extend qualified mirrored/curved/local-zone macro variants only with exact
  serialization, fingerprints, native fill/export and independent CAM coverage.

Full-board acceptance requires real footprints, zero unintended opens/DRC,
complete required coverage and version-pinned independent CAM. Geometry changes
need cross-stage, deterministic and negative release-gate regressions.
Board-specific input budgets, antenna/USB/power qualification and hardware
measurements belong to the selected example/project, not this compiler backlog.
