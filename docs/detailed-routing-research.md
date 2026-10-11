# Detailed routing contracts

Global routes estimate capacity and provide guides; they are not copper or DRC
evidence. Detailed routing uses deterministic bounded walk-around search, with
transactional repair. If no accepted route is found, retain the safe incumbent
and report an incomplete result. A bounded failure is not proof of infeasibility.

Spatial indexes and grids are broad phases. Exact integer copper/shape, drill,
material, keepout and full via-span checks accept emitted geometry. Courtyards
do not substitute for copper obstacles; unconnected pads remain copper. Immutable
macros, critical pairs and required arrays retain occurrence-aware ownership.

Multi-terminal routes preserve real junctions and every required land. Pad-centre
grids, physical access candidates and declared breakout widths feed searches;
compressed pieces and shortcuts receive exact rechecks. Cleanup protects branches,
contacts and owner copper. Failed attempts never publish tentative tracks or
private branch checkpoints.

Ordinary and critical transitions use through-vias unless an explicitly declared
technology has matching dimensions and span. Through-vias occupy every spanned
layer, not only the chosen search transition. Via-in-pad needs pad-scoped process
permission; native connectivity acceptance alone does not grant it.

Declared plane intent is not a connection. Detailed routes, actual physical lands,
native filled connectivity and independent DRC must agree before closure. Generated
KiCad projects carry selected board rules; final qualification remains separate.

Use [package access](package-access-first.md), [filled-copper feedback](filled-copper-feedback.md),
[stub cleanup](route-stub-cleanup.md) and [search policy](routing-search-policy.md)
for specialized contracts; unresolved algorithm work is in [routing backlog](routing-backlog.md).

Related algorithm references: [KiCad router](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html),
[TritonRoute](https://vlsicad.ucsd.edu/Publications/Journals/j133.pdf),
[Freerouting architecture](https://github.com/freerouting/freerouting/blob/master/docs/architecture.md)
and [PathFinder](https://janders.eecg.utoronto.ca/1387_2015/readings/pathfinder.pdf).
