# Routing-to-manufacturing research and implementation recommendations

Status: implemented baseline, researched 2026-09-22.

The manufacturing path was exercised end to end with installed KiCad 10.0.6:
KiCad DRC passed and the independent verifier accepted seven Gerber files, two
separated drill files, and one IPC-D-356 netlist before atomic publication.

This document records six separate research tasks for workflow steps 4–9. The
recommendations form one sequence: each stage consumes a typed, immutable result
from the prior stage, and no planning proxy can satisfy a manufacturing gate.

## 4. Congestion estimator and global router

**Research task.** Identify a deterministic method that predicts whether a
placed PCB is routable, assigns layers and corridors, handles capacity, and
does not pretend to create manufacturable copper.

**Findings.** FastRoute-style G-cell graphs are the established separation
between placement and exact routing. PathFinder's present-cost plus historical-
cost negotiation robustly resolves shared-resource congestion over successive
reroutes. OpenROAD similarly treats global routes as guides for a separate
detailed router and permits deliberate capacity derating.

**Recommendation and implementation.** Use a multilayer G-cell capacity graph,
multi-source A* for multi-terminal trees, and bounded PathFinder-style
negotiation. Represent width plus clearance as physical demand. Emit corridors,
proposed layer changes, overflow and stable fingerprints, never tracks or vias.
Implemented in `pcbir.routing`.

## 5. Placement–routing feedback loop

**Research task.** Determine how real routing results should improve placement
without destabilizing hard constraints or accepting a locally better but
incomplete route.

**Findings.** Routability-driven placement works well as an iterative two-stage
system: placement proposes geometry and routing supplies non-differentiable
congestion evidence. NS-Place explicitly optimizes net separation and then
legalizes. A router in the loop provides stronger evidence than another smooth
wirelength proxy.

**Recommendation and implementation.** Route every retained legal candidate;
select by a lexicographic vector led by unrouted nets and overflow; make bounded
hotspot-driven legal moves; reroute; accept only strict improvements; otherwise
roll back atomically and reduce movement. Stop on stagnation and certify the
winner with a fresh full global route. Implemented in `pcbir.routeflow`.

## 6. Specialized critical-net routing

**Research task.** Decide which nets must route before ordinary signals and
which claims a geometric router can safely make without a field solver or
electrical simulation.

**Findings.** High-speed guidance consistently treats differential geometry,
layer continuity, via count, uncoupled length, return paths, skew and impedance
as explicit design constraints. Protocol values cannot safely be hard-coded by
the router because stackup, device generation and context determine the actual
requirement. Geometric compliance is not electrical qualification.

**Recommendation and implementation.** Route only explicitly profiled nets as
critical. Order them by declared priority; route differential/CAN nets as
coupled bundles; handle clock, RF-feed, power and other constrained single nets;
materialize their exact copper first and lock it. Check representable geometric
budgets and retain explicit external impedance/SI/RF/current/thermal assumptions.
Implemented in `pcbir.critical`.

## 7. General detailed router

**Research task.** Select a practical first algorithm that converts global
guides into exact tracks and vias while respecting locked critical copper and
leaving a path toward richer PCB geometry.

**Findings.** TritonRoute's architecture separates pin access, track assignment,
initial routing, search-and-repair, and integrated DRC. Guide-aware maze/A*
routing plus deterministic rip-up/reroute is a stronger baseline than greedy
point-to-point traces. Pin access must be a first-class input.

**Recommendation and implementation.** Build legal pin-access nodes on a fine
multilayer grid, treat critical copper and component bodies as hard obstacles,
prefer but permit bounded deviation from global guides, grow multi-terminal
trees with deterministic A*, and repeat with present and historical resource
costs. Materialize exact `TrackSegment` and `Via` objects and fail partially
rather than claiming success with conflicts. Implemented in `pcbir.detailed`.

The baseline is orthogonal and grid based. It does not yet provide push-and-
shove, arcs, arbitrary-angle traces, plane fills, neck-down escape shapes or
post-route length tuning; these are later improvements behind the same IR.

## 8. Physical DRC and signoff

**Research task.** Define a release gate that distinguishes a violation-free
run from a complete run, supports controlled waivers, and becomes invalid after
any geometry change.

**Findings.** Production DRC must check both connectivity and pairwise physical
constraints. KiCad checks copper objects including pads, tracks and vias, but
tool success alone does not prove that every domain-specific analysis ran.
Safety standards and electrical simulation have scopes beyond geometric DRC.
Coverage therefore has to be reported independently from findings.

**Recommendation and implementation.** Run fail-closed checks over exact
physical IR for placement legality, opens, cross-net shorts/clearance among
pads/tracks/vias, copper edge spacing, track profile constraints, layer/length/
via budgets, and via structure. List unsupported analyses explicitly. A required
check that cannot run makes coverage incomplete and the decision fail. Bind
waivers to a stable finding fingerprint, and bind the signoff token to canonical
footprint, placement, copper, board, rule, policy and report digests. Implemented
in `pcbir.drc`; KiCad DRC runs again during export as a second engine.

The initial geometric engine conservatively bounds non-circular pads by their
maximum radius. This may reject legal dense geometry, but it cannot silently
relax clearance. Final solder-mask, zone, creepage, SI and thermal checks remain
visible unsupported coverage until their models exist.

## 9. Manufacturing export and independent CAM verification

**Research task.** Choose the authoritative export path and package checks that
minimize the risk of a valid IR producing incomplete or ambiguous factory data.

**Findings.** Gerber X2 remains a standard image transfer format; Excellon/XNC
remains common for drill data; IPC-D-356 carries an independent electrical test
netlist. KiCad 10 exposes non-interactive DRC and export commands. Writing a new
Gerber generator would add substantial format and qualification risk, while an
intelligent format such as IPC-2581 can be added later as another profile.

**Recommendation and implementation.** Treat the generated KiCad PCB as an
intermediate export artifact and pin a qualified KiCad major version. Require a
matching complete passing CopperScript signoff token; reject proxy footprints;
run KiCad DRC with violation exit codes; generate explicit-layer Gerber X2,
metric separated drill files, and IPC-D-356; parse outputs independently for
format, units, layer function/polarity, termination and expected file counts;
write SHA-256 checksums and a provenance manifest. Build in a sibling staging
directory and rename it into place only after all gates pass. Implemented in
`pcbir.manufacturing`.

## Compatibility contract and sequence

`pcbir.flow.run_routing_pipeline()` composes steps 4–8:

1. placement/global-route feedback produces one certified placement and global
   guide fingerprint;
2. critical routing consumes those guides and adds locked exact copper;
3. detailed routing preserves that copper and routes ordinary nets;
4. physical DRC digests the exact routed board and produces its signoff token;
5. manufacturing export accepts only that same board/token pair.

Failures and partial results remain inspectable but cannot advance the release
gate. Manufacturing output is never written into its requested final directory
until both KiCad and the independent CAM parser succeed.

## Primary sources

- [PathFinder negotiated-congestion router](https://janders.eecg.utoronto.ca/1387_2015/readings/pathfinder.pdf)
- [OpenROAD global router](https://github.com/The-OpenROAD-Project/OpenROAD/blob/master/src/grt/README.md)
- [OpenROAD detailed router / TritonRoute](https://github.com/The-OpenROAD-Project/OpenROAD/blob/master/src/drt/README.md)
- [NS-Place: net-separation-oriented PCB placement](https://arxiv.org/abs/2210.14259)
- [USB-IF USB 2.0 specification](https://www.usb.org/document-library/usb-20-specification)
- [TI high-speed differential layout guidance](https://www.ti.com/document-viewer/lit/html/SLLA653/GUID-A4F1CD83-9D39-45B1-B7A5-0E03429E4305)
- [KiCad 10 PCB Editor and DRC documentation](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html)
- [KiCad 10 command-line interface](https://docs.kicad.org/10.0/en/cli/cli.html)
- [KiCad board file format](https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/)
- [Ucamco Gerber and XNC specifications](https://www.ucamco.com/en/guest/downloads/gerber-format)
- [IPC-2581C scope](https://www.ipc.org/TOC/IPC-2581C-toc.pdf)
