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
- [Routing](docs/routing.md#open-work): access/ownership, route quality, power
  and package planning, and assessment.
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

Routing algorithm work, including the power/ground copper-area stage, layer
roles, whole-package fanout, arched-corner follow-ups and macro variants, is in
[routing open work](docs/routing.md#open-work).

Full-board acceptance requires real footprints, zero unintended opens/DRC,
complete required coverage and version-pinned independent CAM. Geometry changes
need cross-stage, deterministic and negative release-gate regressions.
Board-specific input budgets, antenna/USB/power qualification and hardware
measurements belong to the selected example/project, not this compiler backlog.
