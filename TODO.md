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

- [ ] Add a separate power/ground copper-area planning stage shared by placement
  and routing. Plan rail partitions, ground continuity, local bypass/return vias,
  sensitive-node exclusions and reserved signal corridors before detailed tracks;
  feed infeasible access back to placement. Verify actual native-filled fragments
  and return paths, with bounded trials, exact ownership and complete rollback.
  A zone outline or same-net label must never count as connectivity or RF proof.
  See [the stage requirements](docs/routing-backlog.md#separate-powerground-copper-area-planning-stage).
  The earlier opt-in stage ran only with the legacy router and was removed with
  it on 2026-10-26; the unified router has no power-area stage yet.
- [ ] Add explicit source layer roles/dedicated-plane semantics beyond coverage
  inference, preserving existing zone/stitching/per-pad controls.
- [ ] Implement coherent whole-package fanout planning before mutable local
  feeds: geometry-derived dogbone patterns, joint signal/power/ground contacts,
  verified layer exits and coordination with support placement and copper areas.
  Reuse bounded local assignment and exact ownership; preserve fixed intent,
  critical macros and required via arrays. Joint escape side, layer and order
  per package follow [the escape planning plan](docs/escape-planning-plan.md).
  The earlier P1 proposals and power-preparation integration were legacy-router
  only and were removed on 2026-10-26.
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
