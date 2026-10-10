# Agent guidance

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
here, reusable component contracts in CopperLib, board-specific bindings/evidence
in the consumer. Run the final-native audit/engineering/checkpoint commands;
missing tools, source facts, solver or bench evidence are incomplete. Never infer
production approval from generated files, raster agreement, ERC or native DRC.

## Adding or changing components and circuits

Follow [component integration guidance](docs/component-integration-guidelines.md)
when adding or replacing parts, reusable circuits, or example-board components.
These rules also apply to generated definitions and imported CopperLib parts.

- Generic components and reusable circuit modules, including reusable
  footprints and hard macros, belong in CopperLib, not project-local libraries
  or CopperScript examples. Check for an existing package first and import the
  shared definition. Keep board-specific composition, placement, mechanics and
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
