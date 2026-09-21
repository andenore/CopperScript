# Physical layout workflow

CopperScript exposes four physical-design gates: **Prepare**, **Place**,
**Route**, and **Verify**. This is intentionally smaller than the many-step
checklists used by individual EDA tools. The detailed algorithms still exist,
but they are internal substeps with one clear input and exit condition per
gate.

No generated board is fabrication-ready until all four gates pass. The first
implementation completes only a placement candidate. It marks Route as not run
and Verify as blocked.

## Research and consolidation

The original workflow listed thirteen stages. Research did not support keeping
each as a separate user-visible stage:

| Original concern | Current method | Gate |
|---|---|---|
| Footprints, board mechanics, stackup, fabrication limits, net classes | Validate and normalize exact inputs before optimization | Prepare |
| Functional partitioning, mechanical anchors, power/RF/thermal regions | Fixed and soft placement constraints | Prepare / Place |
| Global floorplan | Connectivity-aware analytical or partitioning-based placement | Place |
| Legalization | Constraint-graph, packing, or MILP repair of overlaps and boundaries | Place |
| Detailed placement | Local refinement with wirelength, density, alignment, and routability costs | Place |
| Routability prediction | Coarse global-routing demand/capacity estimate | Place |
| Layer/corridor planning | Global routing with capacity and topology constraints | Route |
| Critical and general routing | Constraint-driven detailed routing, normally A* or maze search with negotiated rip-up and reroute | Route |
| Length/skew tuning and return-path repair | Post-route constrained refinement | Route |
| Planes and copper zones | Connectivity-aware fill followed by another route/DRC pass | Route |
| Electrical, thermal, assembly, fabrication, and CAM checks | Domain-specific sign-off checks with release gating | Verify |

This removes two important overlaps. First, “floorplan,” “global placement,”
“legalization,” and “detailed placement” are one iterative placement problem,
not four independent deliverables. Second, routability estimation is feedback
to placement; actual global and detailed routing belong together in Route.

Modern PCB-placement work supports a multi-stage implementation internally:
analytical or connectivity-driven global placement, legalization, then detailed
refinement. Recent work also treats routability and local routing space as
first-class costs rather than relying on wirelength alone. NS-Place combines
routability optimization with MILP legalization; newer analytical work follows
the same global/legalize/fine-tune structure. Constraint-graph legalization is
a useful scalable direction when richer relative-placement constraints are
introduced.

For routing, the mature baseline remains global capacity estimation followed by
maze/A* detailed routing with negotiated congestion, rip-up, and reroute.
Interactive commercial flows add constraint-driven topology, width, layer,
via, impedance, length, skew, and return-path checks. CopperScript should use
the same separation: the global estimate guides placement, while a future
detailed router owns geometrically valid copper.

Verification is not one algorithm. It is a release gate aggregating geometric
DRC, connectivity, impedance and timing constraints, return paths, SI/PI,
thermal/RF checks where applicable, assembly/fabrication checks, and independent
CAM inspection. CopperScript must retain explicit not-run or failed states
instead of equating successful file generation with sign-off.

## Implemented placement planner

`pcbir.layout.plan_placement()` currently performs:

1. rectangular board-boundary validation;
2. preservation and validation of caller-selected fixed placements;
3. connectivity-weighted deterministic greedy global placement;
4. exact axis-aligned boundary and component-clearance legalization;
5. bounded deterministic local refinement; and
6. coarse bin-based routability estimation using Manhattan minimum spanning
   trees and demand-aware L routes.

The refinement objective combines half-perimeter wirelength with routing-bin
overflow. It is deliberately deterministic so identical physical IR produces
identical KiCad output and reports.

Run it with:

```console
python -m copperscript plan-layout examples/valid_board.copper \
  --allow-proxy-footprints \
  -o planned.kicad_pcb \
  --report layout-report.json
```

Proxy footprints are useful only for exercising the workflow. For meaningful
physical results, use resolved `.kicad_mod` footprints. In either case the
result remains non-fabrication-ready because no detailed routes or sign-off
checks have run.

The report schema is `copperscript-layout-report/v0.1`. It contains the four
gate states, actionable findings, HPWL, estimated connection count, routing-bin
capacity, overflow, and maximum estimated utilization.

## Deliberate limitations and next increments

The initial planner supports rectangular boards, orthogonally rotated component
body boxes, two-sided placements as ordinary occupied rectangles, default design
rules, and uniform routing-bin capacity. It does not yet model keepouts,
courtyards, connectors fixed by source constraints, RF antennas, thermal
regions, layer-specific capacity, via demand, differential pairs, planes, or
actual pad escape.

The next Place increment should add source-level physical constraints and
courtyard/keepout geometry before replacing the greedy global placer. A future
Route increment should first implement a global routing graph with per-layer
edge capacity and topology constraints, then a detailed router with negotiated
rip-up/reroute. Verify should be implemented as independently reportable
checks, with release blocked unless every required check passes.

## Sources

- [Analytical PCB placement optimization with fine tuning (Integration, 2026)](https://www.sciencedirect.com/science/article/pii/S016792602500224X)
- [NS-Place: routability-driven PCB placement with legalization](https://cseweb.ucsd.edu/classes/fa23/cse248-a/papers/placement/PCBPlacement.pdf)
- [Constraint-graph-based PCB placement legalization (DAC 2025)](https://scholars.lib.ntu.edu.tw/entities/publication/ba17d07b-da5e-4ff4-b3f9-d13ef75303aa)
- [Negotiated-congestion and rip-up/reroute routing](https://engineering.lehigh.edu/sites/engineering.lehigh.edu/files/_DEPARTMENTS/ise/pdf/tech-papers/08/08t_003.pdf)
- [Altium PCB routing workflow and constraints](https://www.altium.com/documentation/altium-designer/pcb/routing)
- [Altium high-speed rules, xSignals, tuning, and return paths](https://www.altium.com/documentation/altium-designer/pcb/high-speed-design/xsignals/design-rule-support)
- [Altium batch and online design-rule checking](https://www.altium.com/documentation/altium-designer/pcb/drc)
- [KiCad PCB Editor reference](https://docs.kicad.org/8.0/en/pcbnew/pcbnew.pdf)
- [IPC-2231 DFX guidelines](https://www.ipc.org/TOC/IPC-2231-toc.pdf)
- [IPC-2581 Revision C manufacturing data standard](https://www.ipc.org/news-release/ipc-releases-ipc-2581-revision-c-generic-requirements-printed-board-assembly-products)
- [Ucamco Gerber format specifications](https://www.ucamco.com/en/gerber/downloads)
