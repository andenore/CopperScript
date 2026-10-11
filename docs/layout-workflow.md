# Physical layout workflow

The electrical source owns connectivity; resolved physical IR owns pads,
placements, rules and geometry. Use [CLI workflow](cli-workflow.md) and
[Make builds](make-builds.md) for commands. This reference covers the planner
and global guide contracts; [detailed routing](detailed-routing-research.md)
accepts actual copper.

## Placement planner

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
   dense packages (32 or more pads) also receive a soft 8-10 mm escape-channel
   objective and bounded legal translations with their close-placement
   companions, so decoupling rules stay intact;
6. periodic coarse-routing feedback with per-layer capacity, crossing, via,
   and pin-escape estimates; and
7. Pareto filtering and deterministic ranking of retained candidates.

Wirelength is measured from transformed physical pads, not component centers.
The ranking vector combines hard-constraint penalty and margin, congestion,
crossings, estimated vias, pin-escape pressure, dense-package spacing, pad-aware HPWL, and semantic
group spread. Identical physical IR and options produce identical candidates,
KiCad output, and reports.

Run it with:

```console
python -m copperscript plan-layout examples/valid_board/board.copper \
  --allow-proxy-footprints \
  --candidates 3 \
  -o planned.kicad_pcb \
  --report layout-report.json
```

Proxy footprints are useful only for exercising the workflow. For meaningful
physical results, use resolved `.kicad_mod` footprints. In either case the
result remains non-fabrication-ready because no detailed routes or sign-off
checks have run.

For a routing-capacity experiment, `--layers 6 --fab-profile jlcpcb-six-layer`
is also accepted by the PCB export, placement, global-route, and board-route
commands. It uses F.Cu/In1.Cu/In2.Cu/In3.Cu/In4.Cu/B.Cu with the same
provisional 0.09 mm clearance and 0.20 mm default track width as the four-layer
profile. This is not a controlled-impedance stackup model or a six-layer
manufacturing qualification; a routed draft still requires plane-fill,
independent KiCad DRC, electrical review, and CAM signoff.

The report schema is `copperscript-layout-report/v0.1`. It contains the four
gate states, actionable findings, the selected candidate, the retained Pareto
frontier, per-phase statistics, pad-aware HPWL, crossings, estimated vias,
pin-escape pressure, dense-package spacing penalty, constraint penalty and margin, group spread, and
routing-bin capacity, overflow, and maximum estimated utilization.



## Global routing

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

`pcbir.routeflow.optimize_placement_for_routing()` wraps placement and global
routing in a transactional feedback loop. It globally routes every retained
placement candidate, keeps the best lexicographic checkpoint, and attempts
bounded legal moves for components contributing to hotspots. A move is accepted
only when a complete route improves; failed trials leave no history or geometry
behind. Movement is reduced after rejection, repeated non-improvement stops the
loop, and the accepted placement is always certified by a fresh full reroute.
A feedback `pass` therefore means global feasibility, not detailed routing or
physical signoff.

`pcbir.critical.route_critical_nets()` consumes those guides first for nets
with explicit non-general routing profiles. It materializes locked exact tracks
and vias, routes differential/CAN members as coupled bundles, checks geometric
length, skew, and via budgets, and reports external assumptions for impedance,
RF, current, and thermal qualification. Coupled fanout is always rechecked by
the authoritative physical DRC; a geometric proxy never constitutes SI/RF or
thermal signoff.

`pcbir.detailed.route_detailed()` routes the remaining ordinary nets on a fine
multilayer grid. It derives legal pin-access points, prefers the global-route
corridors, preserves critical tracks and vias as locked obstacles, and uses
deterministic A* with PathFinder-style present and historical costs across
bounded rip-up/reroute passes. Its output is exact `TrackSegment` and `Via`
geometry in physical IR. A partial result remains explicitly non-fabrication-
ready, and even a successful result must pass the separate physical DRC gate.

`pcbir.drc.run_physical_drc()` is that gate for the geometry CopperScript can
currently represent. It checks completed routing, exact net connectivity,
cross-net shorts and clearance, track/routing-profile widths, layer/length/via
budgets, via structure, and copper-to-board-edge clearance. The report keeps
coverage separate from violations: required checks that cannot execute make
the result incomplete and fail it, while analyses outside the present model
(zones, final artwork, creepage, SI and thermal qualification) remain explicit
non-required unsupported entries. Waivers identify one stable finding
fingerprint and retain reason and approver. A SHA-256 signoff token binds the
decision to exact board geometry, routing rules, policy, and report so any
subsequent copper change invalidates it.

`pcbir.flow.run_routing_pipeline()` composes placement feedback, global guides,
critical routing, general routing, and physical DRC without translating through
a backend format. `pcbir.manufacturing.build_manufacturing_release()` accepts
only the resulting exact board and matching complete pass token. It invokes a
qualified KiCad CLI for a second DRC plus explicit Gerber X2, metric Excellon,
and IPC-D-356 exports; a separate structural CAM parser checks the generated
files before an atomic publish. The release includes a provenance manifest and
SHA-256 checksum inventory. See `docs/routing-manufacturing-research.md` for the
individual research tasks, recommendations, limitations, and sources.



## Limits and acceptance

Planning scores, bins and global capacity are heuristics, not routability or
physical-clearance certificates. The placer supports source-declared permitted
angles and fixed/rigid intent; it cannot independently move macro members.
[Directional access/domain/inline placement](placement-internals.md) supplements
connectivity scores. Package, critical and detailed stages retain exact gates.

Actual filled references, RF/USB impedance, thermal/current capacity and final
CAM require separate evidence. Finite search failure never permits weaker source
rules. Open algorithm scope is in [routing backlog](routing-backlog.md).
