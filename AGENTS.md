# Agent guidance

Run `copper agent-guide` for the compact workflow bootstrap; retrieve individual
topics as needed. The guidance is bundled with the active CLI, including when a
consumer does not have this repository checked out. The rules below govern work
on CopperScript itself.

## Shared placement and routing

Use the standard `route-board` pipeline and bounded routing profiles for new
solver work. Board projects may wrap commands and supply electrical/mechanical
intent, macro assets and qualification data; do not add selected-net/pad repair
algorithms there. Algorithms belong in CopperScript and reusable component
contracts in library repositories (CopperLib is one instance). Follow [the autonomous routing plan](docs/autonomous-routing-plan.md)
and [native-feedback limits](docs/filled-copper-feedback.md).

Reproduce failures as small real-geometry cases with renamed nets/components.
Preserve successful escape identities, fixed/rigid placement intent, immutable
macro copper and required via-array multiplicity. Trial mutable dependencies
transactionally with bounded work, exact blockers, fresh validation and complete
rollback. Report unresolved constraints and missing tools without claiming
impossibility. Keep historical consumer helpers until equivalent shared behavior
is verified; do not extend their board-specific repair logic.

## Human-readable numeric output

Use ordinary fixed-point numbers and explicit engineering units in diagnostics,
IR examples, component labels and BOMs: `100 nF`, not `1E+2 nF`. Use the shared
quantity formatter, preserving the declared unit and exact fractional values;
never round a real fraction to an integer merely for display. SI `base_value`
fields remain exact fixed-point decimal strings. Simulator-specific numeric
syntax is a separate machine interface, not a template for human-facing output.

## Decoupling and package escape quality

Use capacitor `role = decoupling` with an explicit `decouples = "U.PIN"`
association in the owning circuit, not per-board guessed max-distance rules.
Inspect blocked direct paths and actual routed pad-to-capacitor geometry; do not
ban necessary ground-return vias or tag RF/load/bulk capacitors blindly. Respect
fixed placements, macro ownership and stronger source-backed constraints.
Follow [routing quality TODO](docs/routing-quality-todo.md); same-net electrical
legality alone is not a reason to create X-shaped package escapes.
Keep verified package exits reserved when an ordinary area route is partial;
later plane stitching must not consume the corridor needed by a repair. Clean
unused escape copper only after that net connects, retaining truthful open-net
status. Package-escape preferences do not yet eliminate all area-route crosses.

## CAM / RF / power qualification

Use [engineering qualification](docs/engineering-qualification.md) and keep the
[implementation checklist](docs/qualification-todo.md) accurate. Algorithms belong
here, reusable component contracts in library repositories, board-specific bindings/evidence
in the consumer. Run the final-native audit/engineering/checkpoint commands;
missing tools, source facts, solver or bench evidence are incomplete. Never infer
production approval from generated files, raster agreement, ERC or native DRC.

## Adding or changing components and circuits

Follow [component integration guidance](docs/component-integration-guidelines.md)
when adding or replacing parts, reusable circuits, or example-board components.
These rules also apply to generated definitions and imported CopperLib parts.

- Generic components and reusable circuit modules, including reusable
  footprints and hard macros, belong in an appropriate reusable library
  repository; CopperLib is one instance, not a required central catalogue.
  For the LLM workflow improvement work, do not add components to CopperLib;
  additions in other library repositories are permitted. Check the consuming
  project's library dependencies first and reuse existing definitions where available.
  Keep board-specific composition, placement, mechanics and
  configuration in the consuming project.
- Always check RF-capable components for required matching/support networks and
  inspect the exact manufacturer's reference schematic and layout. Document
  required, integrated or unresolved matching, and encode source-backed short
  distances, orientation, return paths and antenna keepouts in physical intent.
- For switching regulators, converters and switching chargers, prefer a
  generated, source-backed hard macro preserving critical placement and copper.
  Otherwise require strong, explicit placement and trace-width/current-path
  constraints, not merely proximity hints or default board widths.
- Record reference evidence and adaptations, verify the resulting geometry,
  and flag requirements the tool cannot enforce. ERC, legal placement and native
  DRC alone do not qualify RF performance, power integrity or thermal behavior.

## Compact library retrieval

Use `copper library search` against a consuming project's declared dependencies
before reading whole libraries. Inspect selected `package/path::Symbol` contracts
by section; use `--expect-digest` to bind inspection to a search result. Follow
pagination and retrieve requirements, source notes and necessary imported/device
contracts before integration. Search defaults to locked/offline; source-derived
metadata is not electrical verification. See [compact library context](docs/library-context.md).
