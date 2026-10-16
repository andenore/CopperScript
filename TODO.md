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
  and routing; see [the stage requirements](docs/routing-backlog.md#separate-powerground-copper-area-planning-stage).

- [ ] Add explicit source layer roles/dedicated-plane semantics beyond coverage
  inference, preserving existing zone/stitching/per-pad controls.
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
