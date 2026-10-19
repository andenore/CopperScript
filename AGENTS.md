# Agent guidance

**Routing refactor in progress.** Routing, placement-for-routing and power
work follow [the routing refactor plan](docs/routing-refactor-plan.md). Where
that plan conflicts with the rules below, the plan wins:
- Measure every change with the benchmark ladder.
- Don't create new evidence bundles.
- Don't add gate-driven recovery modules.
- Keep CopperAssetTracker, CopperLedRing and the examples working.

Start with `copper agent-guide` in the active CLI environment. Request individual
topics and selected library contracts as needed. For contributor work, use the
[task index](docs/README.md) to choose the relevant reference; consult
[TODO.md](TODO.md) only when planning unfinished work. Do not bulk-read `docs/`.
Accepted architectural rationale is opt-in; follow current task references.

- Generic algorithms belong in CopperScript; reusable parts/circuits belong in
  independently versioned library repositories; board composition and evidence
  belong in consumers. For this LLM improvement work, do not expand CopperLib.
- Use the standard bounded `route-board` pipeline. Preserve fixed/rigid intent,
  immutable macros, required via-array multiplicity and verified package exits
  while a net is partial. Mutable dependency trials need exact ownership,
  fresh validation and complete rollback. Do not add board-specific repair
  selectors. See [routing invariants](docs/filled-copper-feedback.md).
- For every blocked package exit or package-escape fault, compare the failing
  geometry with the applicable manufacturer reference layout before choosing a
  repair. Align package/pin coordinates and inspect relevant layers, vias,
  support placement and power/ground topology. Explain the differences, then
  improve generic placement/routing algorithms to reproduce the reference's
  applicable structure under the actual design rules. Validate the improvement
  on a bounded subset and retain before/after evidence. Missing references and
  unsupported adaptations remain explicit. See [integration guidance](docs/component-integration-guidelines.md).
- For component/circuit changes, follow [integration guidance](docs/component-integration-guidelines.md):
  exact source facts, pin-associated decoupling, RF matching/reference layout and
  source-backed switched-power macros or strong physical constraints. Retrieve
  existing dependency contracts before adding definitions; unknown facts remain
  unresolved. Unsupported checks and missing tools must stay visible.
- Use ordinary fixed-point engineering units through the shared quantity
  formatter (`100 nF`); preserve exact fractions and fixed-point `base_value`.
  Simulator syntax is a separate interface.
- ERC, legal geometry and native DRC do not establish electrical/RF/power,
  assembly, CAM or prototype qualification. Follow [engineering qualification](docs/engineering-qualification.md)
  when changing evidence or release gates; changed inputs invalidate old evidence.
