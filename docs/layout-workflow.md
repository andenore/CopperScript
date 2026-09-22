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

1. validation of polygonal board boundaries, footprint courtyards, placement
   regions, keepouts, fixed components, legal orientations, relative rules,
   and semantic groups;
2. hierarchy-aware clustering from module provenance, interfaces, explicit
   groups, and proximity constraints;
3. deterministic multi-seed analytical global placement using smooth,
   pad-aware wirelength gradients, density spreading, and group cohesion;
4. orientation selection followed by bounded grid/Hanan-style legalization,
   with recursive exact repair for small dense conflict tails;
5. hard relative-constraint repair and legal local moves, swaps, and rotations;
6. periodic coarse-routing feedback with per-layer capacity, crossing, via,
   and pin-escape estimates; and
7. Pareto filtering and deterministic ranking of retained candidates.

Wirelength is measured from transformed physical pads, not component centers.
The ranking vector combines hard-constraint penalty and margin, congestion,
crossings, estimated vias, pin-escape pressure, pad-aware HPWL, and semantic
group spread. Identical physical IR and options produce identical candidates,
KiCad output, and reports.

Run it with:

```console
python -m copperscript plan-layout examples/valid_board.copper \
  --allow-proxy-footprints \
  --candidates 3 \
  -o planned.kicad_pcb \
  --report layout-report.json
```

Proxy footprints are useful only for exercising the workflow. For meaningful
physical results, use resolved `.kicad_mod` footprints. In either case the
result remains non-fabrication-ready because no detailed routes or sign-off
checks have run.

The report schema is `copperscript-layout-report/v0.1`. It contains the four
gate states, actionable findings, the selected candidate, the retained Pareto
frontier, per-phase statistics, pad-aware HPWL, crossings, estimated vias,
pin-escape pressure, constraint penalty and margin, group spread, and
routing-bin capacity, overflow, and maximum estimated utilization.

## Deliberate limitations and next increments

The implemented legalizer accepts polygonal board and region outlines and uses
orthogonal component orientations. Courtyard polygons are used when present;
otherwise footprint body bounds are the conservative occupied shape. Routing
capacity is a coarse uniform-bin model. Crossings, vias, and pad escape are
placement proxies rather than geometrically valid routes, and the current model
does not yet understand differential-pair topology, controlled impedance, RF
keep-in geometry, thermal coupling, planes, or copper zones.

The next major increment belongs to Route: a global routing graph with
per-layer edge capacity and topology constraints, followed by a detailed router
with negotiated rip-up/reroute. Verify remains a collection of independently
reportable sign-off checks, with release blocked unless every required check
passes.

## Implemented global routing

`pcbir.routing.route_global()` builds a deterministic multilayer G-cell graph,
derives pad-access nodes, accounts for per-net width and clearance demand, and
grows multi-terminal trees with multi-source A*. Bounded PathFinder-style
negotiation adds present and historical congestion costs across reroute passes.
The result records corridors, layer assignments, proposed via transitions,
unreachable terminals, capacity overflow, contributors, and stable input/output
fingerprints.

Global-route segments are guides only. They never enter `PhysicalBoard.tracks`
or masquerade as routed copper. Exact geometry remains the responsibility of
the specialized and general detailed routers, followed by independent physical
DRC.

## Sources

- [Analytical PCB placement optimization with fine tuning (Integration, 2026)](https://www.sciencedirect.com/science/article/pii/S016792602500224X)
- [NS-Place: routability-driven PCB placement with legalization](https://cseweb.ucsd.edu/classes/fa23/cse248-a/papers/placement/PCBPlacement.pdf)
- [PathFinder negotiated-congestion routing](https://janders.eecg.utoronto.ca/1387_2015/readings/pathfinder.pdf)
- [FastRoute global routing](https://onlinelibrary.wiley.com/doi/10.1155/2012/608362)
- [OpenROAD global-router architecture](https://github.com/The-OpenROAD-Project/OpenROAD/blob/master/src/grt/README.md)
- [Constraint-graph-based PCB placement legalization (DAC 2025)](https://scholars.lib.ntu.edu.tw/entities/publication/ba17d07b-da5e-4ff4-b3f9-d13ef75303aa)
- [Negotiated-congestion and rip-up/reroute routing](https://engineering.lehigh.edu/sites/engineering.lehigh.edu/files/_DEPARTMENTS/ise/pdf/tech-papers/08/08t_003.pdf)
- [Altium PCB routing workflow and constraints](https://www.altium.com/documentation/altium-designer/pcb/routing)
- [Altium high-speed rules, xSignals, tuning, and return paths](https://www.altium.com/documentation/altium-designer/pcb/high-speed-design/xsignals/design-rule-support)
- [Altium batch and online design-rule checking](https://www.altium.com/documentation/altium-designer/pcb/drc)
- [KiCad PCB Editor reference](https://docs.kicad.org/8.0/en/pcbnew/pcbnew.pdf)
- [IPC-2231 DFX guidelines](https://www.ipc.org/TOC/IPC-2231-toc.pdf)
- [IPC-2581 Revision C manufacturing data standard](https://www.ipc.org/news-release/ipc-releases-ipc-2581-revision-c-generic-requirements-printed-board-assembly-products)
- [Ucamco Gerber format specifications](https://www.ucamco.com/en/gerber/downloads)
