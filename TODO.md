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
- [Routing optimization](docs/routing-optimization-todo.md): profiled bottlenecks
  and guarded comparisons; preserve output quality and evidence.
- [Qualification gaps](docs/qualification-todo.md): geometry adapters, derating/
  thermal extraction, native release integration and mandatory external evidence.
- [Other contributor work](docs/contributor-backlog.md): mechanical/editor,
  electrical-model, simulation and architecture gaps.

## Additional routing and asset scope

- [ ] Add explicit source layer roles/dedicated-plane semantics beyond coverage
  inference, preserving existing zone/stitching/per-pad controls.
- [ ] Add hierarchical module-to-macro frontend selection and movable-macro
  feedback with complete boundary/owner revalidation.
- [ ] Extend qualified mirrored/curved/local-zone macro variants only with exact
  serialization, fingerprints, native fill/export and independent CAM coverage.

Full-board acceptance requires real footprints, zero unintended opens/DRC,
complete required coverage and version-pinned independent CAM. Geometry changes
need cross-stage, deterministic and negative release-gate regressions.
Board-specific input budgets, antenna/USB/power qualification and hardware
measurements belong to the selected example/project, not this compiler backlog.
