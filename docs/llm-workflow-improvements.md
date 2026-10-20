# Future LLM workflow improvements

Current usage and implemented limits are in [LLM workflow](llm-workflow.md) and
[library context](library-context.md). This plan records unfinished work only.
CopperScript owns generic mechanisms; independently versioned libraries own
reusable contracts; consumer projects own bindings and board measurements.

## 1. Measure the LLM workflow

Evaluation remains future work; do not claim measured token savings before it runs.

- Freeze a task corpus: simple sensor board, constrained two-layer controller,
  four-layer mixed-interface board, unfamiliar part integration, replacement
  part, connector relocation, and a changed operating envelope.
- Compare CopperScript and direct KiCad automation with the same model, initial
  specifications, part evidence, tools, compute budgets and acceptance criteria.
  Include both a cold start and reuse of already reviewed library assemblies.
- Record input/output tokens including documentation retrieval, unsuccessful
  tool calls and retries; human interventions; elapsed time; search limits;
  completed connections; engineering findings; and unresolved requirements.
- Separate one-time library authoring cost from marginal board construction cost.
  Publish both rather than hiding preparation in one comparison arm.
- Use multiple attempts per task. Publish distributions, failures and exact
  input/tool identities; a selected successful run is not a success rate.
- Have independent reviewers assess functionality requirements and layout quality.
  Hardware-function results remain pending until item 6 becomes available.
- Deliver a machine-readable evaluation manifest, replay instructions and a
  concise result table. No numerical token-saving claim before this evidence.

## 2. Reviewed reusable contracts across repositories

Selective source-derived search/inspection is implemented. Remaining work:

- Publish versioned reviewed contract bundles with exact manufacturer/package
  identities, source revisions/locators, support topology, operating limits,
  variants, pin-associated bypasses and host integration obligations.
- Carry assumptions, unresolved facts and qualification requirements with each
  package. Preserve source-backed RF/power macros, immutable copper, ports,
  keepouts and adaptation limits; board measurements stay with consumers.
- Validate pad maps, hierarchy, procurement identities, contract failures,
  macro transforms/ports and deterministic regeneration before publication.
- Extend provider-independent retrieval with persistent aggregate caching and an
  explicit transitive version policy. Pin module revision, asset inventories,
  footprint providers and content identity; do not assume CopperLib ownership
  or silently alter project locks. Indexes must invalidate when inputs change.
- Preserve the distinction between indexed declarations and reviewed facts;
  never promote an entire catalogue through a blanket qualification flag.

Add definitions when a project needs them and after searching its dependencies.
For this improvement work, do not expand CopperLib; additions in other library
repositories are permitted. Improve mechanisms independently of catalogue size.

## 3–5. Remaining reporting, verification and acceptance extensions

Compact reports, per-declaration verification and bounded routing acceptance
are implemented. Further work includes measured shortest bypass paths, complete
pair/return/stub/tuning verifiers, supported external evidence adapters, larger
dense real-board acceptance and statistical success rates. Unsupported checks
remain explicit blockers until their verifier and evidence are available.

## 6. Deferred: fabrication and prototype measurements

Wait for a later request. No fabrication or measurement campaign is initiated
by this plan. Future acceptance should cover rail startup, transient loads,
charging/temperature behavior, firmware interfaces and RF tuning/OTA behavior
in the intended enclosure, bound to the actual built revision and components.

## 7. Source ergonomics and feedback (proposed)

Implemented: explicit `no_connect` with `UNMARKED_PIN`, located `check`
diagnostics with hints and `--json` fixes, `copper explain` and `copper query`;
for routing iterations `copper issues` (grouped, located failures), `copper view`
(per-layer windows with reference overlays) and `pcbir.workbench` state files.
Proposed next, each to be measured with item 1 before claiming savings:

- Arrays, loops and pin groups (`component LED[1..12]`, `U1.VSS*`) and
  placement patterns for repeated parts.
- Board `defaults {}`, footprint aliases and inline two-terminal connections
  (`between = (DEC1, GND)`).
- Parameterized modules; typed SPI/UART/SWD/USB interfaces that generate nets;
  solver-backed pin assignment from the existing mux model, written back to source.
- Typed firmware obligations and pin-map export (devicetree overlay, C header).
- Semantic `copper diff` at netlist/intent level; labelled per-layer renders for
  vision models; relative placement as the default style in examples.
- An MCP server over check/query/library/route/summarize; datasheet-to-part
  drafting with per-fact locators; guide entries mined from evaluation failures.
- Per-property source locations so `CMP018` can point at, and fix, the property.
- Routing iterations: solver decision traces with `copper explain-route` (located
  blockers instead of monkeypatched internals); a structured reference diff to
  accompany the overlay (via counts/positions, layer use, support placement);
  `copper verify-native` cached by board digest; `route-board --resume` with a
  narrowed net scope.
