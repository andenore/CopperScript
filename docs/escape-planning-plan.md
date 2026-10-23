# Escape planning: research and integration plan

Status: accepted plan, in progress (E0–E5 implemented, E7 started; 2026-10-22). Background research lives in the
Copper project notes (SOTA review Tier 2.1, D3-A fix plan); this file is the
working copy for contributors and agents in this repository.

## Status

| Step | Branch / PR | Result |
|---|---|---|
| E0 | #24 | Diagnostics only; D3-A routing identical (93 clean). D3-B added: 7 of 10 clean. Remaining D3 failures by cause: two-end crossings 6 of 17 unrouted nets (every failing signal net on 0048, 0050, 0051), channel overflow 1 (0024). CopperProbe: 12 of 28 open nets are the ADC bus bundle (U_ADC–U_USB, 72 crossing pairs). So E3 came before E2 |
| E1 | #25 (re-landed in `claude/plan-escape-e1-e3`) | Model only; routing identical |
| E2 | `claude/plan-escape-e2-array-flow` | Opt-in (`--array-flow`). CopperProbe, unified router: 25 → 21 open nets, 522 → 496 vias. The 7 FX10 balls without a via site stay blocked (see E7) |
| E3 | #26 (re-landed in `claude/plan-escape-e1-e3`) | Opt-in (`--escape-plan`). Off: routing identical. On (CI, bias 20): no clean board lost; 0050 3 → 2 and 0051 3 → 1 unrouted; +33 vias on boards already clean |
| E4 | `claude/plan-escape-e4-router-integration` | Unified router and `route-board --escape-plan`; plan stays opt-in. Ladder: no completion change; vias both ways (cm4 84 → 69, led-ring 36 → 52). Global access bias not done |
| E5 | `claude/plan-escape-e5-bus-layers` | Layer moves opt-in (`--bus-layers`). Buses first and bus skew tuning on by default for the unified router: boards without length-match groups route identical copper (D3, small ladder); CopperProbe 24 → 21 open connections, every ADC data line routes; with a declared 1 mm amplitude the bus meets its 5 mm skew (22 open) |
| E6 | | |
| E7 | `claude/plan-escape-e7-bga-lattice` | In progress. `--via-lattice` (opt-in): FX10 58 balls sited, none blocked (7 before); the GND/supply balls N4–N10, M4–M10 beside the USB macro get no site. FX10 fast case, median open nets over five perturbed runs: base 17, lattice 20, exact raster 17, lattice + exact raster 15 — single runs vary by ±4 nets, so every comparison uses the five-run ensemble. Always open with the lattice: ACQ_EN, FX_RESET, the FX_V33/FX_VDDD/GND supplies (E11), ADC_OE, ADC_OVR and the CC nets (sealed exits, E9) |
| E8 | `claude/plan-escape-e7-bga-lattice` | Partial. `ball_field_exact` (exact raster around ball grids, opt-in) and tolerant ball-grid detection (`package_patterns.ball_grid`: 2 µm rounding, depopulated grids, extra pads; D3 copper identical). Exact raster everywhere (`raster_margin_nm=0`) is not safe: D3 loses 0024, 0035, 0376; full-vertical and led-ring lose a net. Access points per ball (TritonRoute PAO) not done |
| E9 | `claude/plan-escape-e7-bga-lattice` | Tried, no gain. Exit corridors (`exit_reservation_cost` 5, 20), bus bundles, rejection marker cost and retrying failed nets every iteration all stay within ±1.5 nets of the FX10 five-run mean (16.0–16.4); retrying gives 15.2 with the lattice but 17.8 without |
| E10 | | |
| E11 | `claude/plan-escape-e7-bga-lattice` | Islands from the ball matrix (`plan_ball_grid_islands`, via `power_islands_ball_grid_only`): each via-lattice cell goes to the nearest rail, touching same-rail cells form islands, several per rail allowed, other nets' vias become holes (as the kit's PWR1). Two router defects kept every island unusable and are fixed, now on by default: zone-net plane regions use the board minimum width (`plane_fill_width`; FX_V33's 0.4 mm rule width hid every plane under the BGA) and a via the fill touches is a plane tap (`reuse_plane_vias`). Those two alone: FX10 fast case 16.0 → 12.0 (lattice) and 16.4 → 13.8 open nets; D3 identical; small ladder complete with fewer vias; full CopperProbe 22 → 11 open connections, 17 → 10 signal nets, 508 → 370 vias, completion 0.907 → 0.954. Islands on top cost about 2 nets in the what-if (FX10 rails allowed on In4): they stop at the array, so a rail's ferrite and bulk capacitor still route in by track; the kit's islands reach them. CopperProbe's rules keep In4 as ground (a consumer decision); the via lattice with these defaults gave 26 open connections in one full route Islands extended out of the array to each rail's ferrite, bulk and decoupling capacitors (corridors through the host plane): FX_USB18 covers 13 of 16 non-ball pads, FX_V33 reaches its ferrite and bulk capacitor. That is reference-like but worse: FX10 what-if mean 20.6 (12.0 without islands, 13.8 under the array only). Full CopperProbe with In4 allowed for the FX10 rails (committed locally in CopperProbe): no islands 11/237 open, identical copper; with islands 16/168, 14 signal nets (10 before), completion 0.954 → 0.905. FX_V33 then routes, but CC1/CC2, XTALIN/XTALOUT, FX_RESET and ADC_D6 fail: islands take In4 area and add tap vias in the crowded field |
| E12 | | |
| Exact | `claude/plan-escape-e7-bga-lattice` | Analytical track. `escape_capacity` (certificate): CopperProbe FX10 73/73 signal balls escapable, no cut above 60 %; kit FX10 121/121, SIG1 cut 2 at 50/51; global route without overflow, so the open nets are a detailed-routing failure. `exact_escape` (CP-SAT, extra `exact`): all 62 (then 31 non-zone) FX10 signal balls escape on the real board state (OPTIMAL in about 1 min); `exact_escape_copper` makes 26 of 31 legal copper (lanes, exact local maze, lazy cuts). Fixed as copper they make the FX10 fast case worse (mean 17 against 12): the model does not yet carry net rules (width beyond breakout length: 0.15 mm ADC lines do not fit between lattice vias; via budgets) nor the ADC bus order at the array edge |

Update this table when a step's PR merges: the D3 clean count (before → after),
boards gained, full-vertical fingerprint outcome, and the ladder table if it changed.

## Summary

Every strong published PCB routing result comes from planning the escapes of a package's pins **jointly**: which side, which layer and in which order each pin leaves, chosen so the two ends of a group of nets do not cross. Routing graphs and search come second. The algorithms that do this are polynomial network-flow and ordering methods. They are deterministic, so they fit CopperScript's contract. ILP and SAT solvers are needed only for minimising layer count and for ordered buses.

CopperScript has the pieces around such a planner but not the planner. Today:
- fanout picks each pin's escape greedily, then repairs conflicts locally;
- the global router chooses pin access per pin, independently;
- the D3 step 3.8 repair finds crossing conflicts only after detailed routing, by trial and error.

**Proposal:** a new `pcbir/escape_plan.py` stage, run after placement and before global routing, in six PR-sized steps (E0–E5) plus a later placement-feedback step (E6). It is advisory: every escape it suggests is still built and checked by the existing exact geometry and DRC.

**First targets:**
- D3-A boards 0048–0051, two-ended crossings at a radio header;
- the CopperProbe ADC bus at the FX10 (about 11 of its 30 open nets);
- the D3-B tier, which CI does not run yet.

## Why escape planning

- **D3-A, 92/99 clean.** Four of the seven remaining boards fail the same way. On the MySRaspiGW boards, the nets between module U1's pin column and the 2×4 header P1 cross. Each gap between header pins holds exactly one track: 2.54 mm pitch minus a 1.727 mm pad leaves 0.813 mm, and a 0.254 mm track with 0.254 mm clearance on each side needs 0.762 mm. Routing the nets one at a time lets the first ones take the gaps the later ones need.
- **What the original designer did.** The board's own routing in PCBench escapes CE and MISO through vias, so the crossing nets move to the other layer. That is a layer assignment of crossing nets, exactly what simultaneous escape planning computes up front.
- **D3 plan step 3.8 (`_enclosure_repair` in `pcbir/detailed.py`) is the after-the-fact version.** Its enclosure repair rips up and reroutes nets in a window. It fixed 0049 but costs minutes per board, so it is limited to boards with at most two open nets.
- **CopperProbe** ([routing refactor handoff](routing-refactor-handoff.md)): the ADC data lines route alone, but lose the corridor south of the FX10 to other nets. The handoff names "an escape plan or corridor reservation for the bus at the FX10 south edge" as the next lever.

## State of the art

All results are as reported by the authors. Most use private industrial boards, so they are not directly comparable with each other.

| Problem | Best methods | Reported results | Fit for CopperScript |
|---|---|---|---|
| **Unordered escape**: every pin of an array to the boundary, any order | Max-flow on a pin-grid channel graph. Yan & Wong give the first model that handles 45° diagonal capacity correctly, and so the first optimal algorithm ([DAC 2009](https://doi.org/10.1145/1629911.1630001); [TCAD 2012](https://scholars.hkbu.edu.hk/en/publications/correctly-modeling-the-diagonal-capacity-in-escape-routing/)). Staggered arrays: [Ho et al.](https://ieeexplore.ieee.org/document/6105346/) | Optimal within the flow model; polynomial | **Core of E2.** Integer, exact, deterministic |
| **Layer minimisation**, multi-layer | SAT with max-flow theory: Bayless, Hoos, Hu ([ICCAD 2016](https://www.cs.ubc.ca/labs/isd/Projects/monosat/monosat_bga_iccad2016.pdf)). Three ILPs (bus direction, bus layer cubes, multilayer flow): **MORE-Router+** ([ISQED 2024](https://numbda.cs.tsinghua.edu.cn/papers/isqed24_2.pdf)) | Bayless: 22 commercial BGAs with 382–1,755 pins, never more layers than greedy (a TI TMS320C went from 5 to 3 layers). MORE-Router+: 9 cases with 222–1,602 nets and 3–12 layers, all in ≤266 s, where the previous multilayer ILP timed out at 3,600 s on 7 of 9 | Offline check or ordered buses only (E5). Too slow and solver-dependent for the default path |
| **Ordered escape**: a given order at the boundary | SAT (Luo & Wong 2008); min-cost multi-commodity flow ([Jiao & Dong, ASP-DAC 2016](https://ieeexplore.ieee.org/document/7428042)); multi-capacity MC-MCF (Gao 2023); LP relaxation plus heuristic path selection ([Chen et al., Micromachines 2025](https://pmc.ncbi.nlm.nih.gov/articles/PMC12734948)) | Chen 2025: about twice the problem scale and 17% less time than the previous multi-capacity method | E5, for buses and length-matched groups |
| **Simultaneous escape (SER)**: pins of two components escape so the orders at both ends agree, leaving the area between them crossing-free | Maximal planar subset per layer, exact and polynomial on one layer, plus a randomised scalable variant ([Ozdal & Wong, TCAD 2006](https://scholars.hkbu.edu.hk/en/publications/algorithms-for-simultaneous-escape-routing-and-layer-assignment-o/)). Optimal joint pin assignment and escape in polynomial time ([Kong, Yan, Wong, ASP-DAC 2010](https://scholars.hkbu.edu.hk/en/publications/optimal-simultaneous-pin-assignment-and-escape-routing-for-dense-/)). Boundary routing with dynamic net order and backtracking: **B-Escape** ([ISPD 2010](https://ispd.cc/ispd2026/slides/2010/2_05.pdf)). Negotiated congestion: NCER ([ISQED 2010](https://www.isqed.org/English/Archives/2010/Technical_Sessions/123.html)) | B-Escape solved 14/14 industrial cases against Allegro's 7/14, in 0.2–691 s. NCER solved 7/14, but different ones from Allegro (11/14 together). Kong: 80 buses and over 7,000 nets in under 5 minutes | **Core of E3.** The planar-subset layer assignment is what the MySRaspiGW header needs |
| **Complete escape-to-area flows** with variable rules, diff pairs and length | Simultaneous escape, then refinement, then gridless area routing ([Lin et al., DAC 2021](https://scholar.nycu.edu.tw/en/publications/a-complete-pcb-routing-methodology-with-concurrent-hierarchical-r/)). Pins projected to the boundary on a channel projection graph, matching-based hierarchical sequencing, then rip-up and length-aware detailed routing ([Chen et al., DAC 2023 / TCAD 2025](https://scholars.lib.ntu.edu.tw/handle/123456789/732753)) | Lin: 7/7 commercial boards that the commercial tool could not complete (0/7). Chen: 100% routability with no violations where Freerouting and Allegro "cannot complete escape routing". Abstracts only; no code or data | The architecture to aim for. Chen's projection to the boundary is how to handle non-grid packages (QFN, SOIC, headers) |
| **Learned fanout** | PPO with CNN and attention: FanoutNet ([AAAI 2023](https://ojs.aaai.org/index.php/AAAI/article/view/26030)) | 100% routability on industrial cases; 6.8% less wire length | Not adopted: training data and determinism |
| **Tools** | Freerouting reintroduced a `BatchFanout` pre-pass in v2.3 with multi-layer escalation and neck-down fallback ([releases](https://github.com/freerouting/freerouting/releases)) | n/a | Confirms the pre-pass pattern |

**Takeaways:**
1. Joint planning beats per-pin greedy everywhere it is measured: B-Escape against Allegro, Lin's 7/7 against 0/7, Chen against Freerouting and Allegro.
2. Agreement of order between the two ends, and assigning layers to crossing nets, is what decides completion between two parts. This is SER, not BGA fanout alone.
3. The exact core (max-flow, planar-subset dynamic programming) is polynomial and integer. Solvers are only needed for layer minimisation and ordered buses.
4. The literature is mostly about grid arrays. For general packages, Chen 2025 projects pins to the package boundary and plans on that projection. That matches CopperScript's existing `boundary_access` collar.

## What CopperScript has today

From a code survey of `main` (2026-10-21, after PR #18):

- **`fanout.py`** covers any dense SMD package, not only BGAs.
  - Each pin gets candidates in a cascade (interstitial vias, rays, two-leg, maze, surface).
  - Pins are reserved greedily, constrained pins first; `escape_assignment` then runs a local constraint solver (MRV with forward checking) on the exact conflict graph; then a DRC gate.
  - **There is no global objective, no flow and no channel model.**
- **`boundary_access.py`** has ports on a package's collar edges, with an edge and layer per port. This is the only per-pin record of escape direction.
- **`placement_escape.py`** has `EscapeSide`, `EscapeDemand` and `EscapeChannel`, a soft channel-capacity estimate, but it is used only by placement.
- **`package_patterns.circular_array`** detects only complete square-pitch circular arrays. `bga_fanout.py` places fixed quadrant dogbones with no search.
- **`critical_bundles.py`** orders diff pairs between the same two components physically (`_physical_order`). This is the closest existing idea to SER ordering, but only for critical pairs.
- **`layer_assignment.py`** relabels the layers of global runs after routing to reduce crossings, but never moves runs touching a pin access.
- **Global and detailed access are per pin** (`_pin_access_candidates`, `_access_candidates`). Net order is by pad count and span.
- **The unified router** ignores `fanout_accesses`. Pre-planned stubs reach it only as fixed copper (as the BGA dogbones do) or as reservation keepouts (`flow._plan_ordinary_escapes`).

**The gap:** nothing decides side, order and layer for all pins of a cluster together, or checks that the two ends of a set of nets agree.

## Design

`pcbir/escape_plan.py`, with options on `EscapePlanOptions`, off by default until a step shows no losses.

**Placement in the flow.** It runs after placement and before global routing. It produces an `EscapePlan`: for each planned pin, the escape side or exit port, the layer, the order index along the collar, an optional via site, and diagnostics. The diagnostics cover each channel's demand against its capacity, crossings that cannot be avoided, and pins the plan could not place.

**[AGENTS.md](../AGENTS.md) compliance.** It is a forward planning stage that informs routing. It is not a gate and not a recovery module; it never blocks routing.

**Model (E1):**
- **Clusters.** A cluster is the pads of one component whose neighbour spacing is under a threshold, generalising `circular_array` to:
  - grids (BGA, 2×N headers);
  - perimeter rows (QFN, QFP, SOIC);
  - irregular sets (projected to the collar, as in Chen 2025).
  A "cluster pair" is two clusters joined by at least k nets: module and header, MCU and connector.
- **Channel graph.**
  - Nodes are the gaps between neighbouring pins (rows, columns, diagonals), collar ports (from `boundary_access`) and candidate via sites (from `access_sites`).
  - Capacity per layer is ⌊(gap − s)/(w + s)⌋, using each net's own width and clearance (Chen's variable rules) and the neck-down widths from `breakout`.
  - Diagonals follow Yan & Wong's model.

**Solvers:**
- **E2, unordered escape:** integer min-cost max-flow per cluster across layers, with via costs. It is our own deterministic implementation: successive shortest paths with integer costs and sorted tie-breaks. Graphs are small (about 20 nodes for a 2×4 header, a few thousand for a 400-ball BGA). It maximises escaped pins, then minimises vias, then length.
- **E3, two-end ordering and layer assignment:**
  - The desired collar order of each cluster comes from the far end's position (the global guide direction or the target pad).
  - Each layer gets the largest non-crossing subset of nets (Ozdal & Wong). For two clusters this is a longest-increasing-subsequence problem; in general it is a dynamic program on the circle graph.
  - Remaining nets go to the next layer through a planned via, with the flow re-solved under that assignment.
- **E5, ordered buses:** multi-commodity flow for match groups. An optional exact solver may be used only offline or behind an extra, single-threaded with a fixed seed.

**Materialisation (E4).** The plan biases the routers; it does not replace their checks:
- **Global router:** planned side and layer as cost preferences on `PinAccess` candidates, not hard filters at first.
- **Legacy fanout:** plan-consistent candidates first. `EscapeConflicts` and the DRC gate stay.
- **Unified path:** planned escapes become reservation keepouts like `_plan_ordinary_escapes`. For BGAs, `bga_fanout` takes its via assignment from the plan.
- **Detailed router:** nets of a cluster pair are routed in plan order, outermost first (as `critical_bundles._physical_order` does), with the planned layer preferred.
- **Step 3.8:** stays as the last resort.

## Steps

| Step | Content | Target | Size |
|---|---|---|---|
| E0 | **Diagnostics and coverage.** Report each cluster's channel demand against capacity and its crossing count in route results and `copper issues`. Classify every remaining D3 failure and the CopperProbe open nets. Add the 10-board **D3-B** tier to the CI benchmark (Freerouting 0.78 there). | Every open net has a cause class; a D3-B baseline exists | S |
| E1 | **Cluster and channel model** (`escape_plan.py`), with no routing change. | Capacities match hand calculations, e.g. 1 track per MySRaspiGW header gap; Yan & Wong's diagonal counter-example handled | M |
| E2 | **Min-cost-flow escape for arrays**, feeding fanout candidate ranking and `bga_fanout` via choice. | Fewer fanout pending pads; CopperProbe FX10 opens; cm4 | M |
| E3 | **Two-end ordering and planar layer assignment** for cluster pairs. | D3-A 0048–0051; fewer 3.8 windows; time per board | M |
| E4 | **Router integration**: global access bias, detailed net order, unified reservations; then default on. | Benchmark ladder: asset-tracker, nrf52, full-vertical, cm4, led-ring, copper-probe | M |
| E5 | **Ordered and bus escape** for match groups, length-aware. | CopperProbe ADC bus (about 11 of 30 open nets) | L |
| E6 | **Placement feedback.** Channel overflow from the plan feeds `placement_escape` spacing. | Fewer placement repair trials | M |
| E7 | **Package via-site lattice, reserved first.** For every ball grid, exact-checked candidate via sites per connected ball (its four diagonals at the rule-derived offset); one site per ball chosen jointly, preferring the quadrant-to-corner pattern and keeping the outer band and the central cross free of signal vias; sites reserved for their ball before ordinary escapes, bypass reservation and critical routing. Supply and ground bypass paths use their ball's site. | FX10: no ball without a site (7 today); vias aligned with the kit pattern; FX10 fast case and CopperProbe | M |
| E8 | **Exact access near packages.** Router nodes at the package's exact via sites, land exits and channel centrelines, checked when built, instead of the bounded exact maze per trapped pin. | FX10 fast case with default options in minutes (the maze is about 85% of its time today); no exact-commit rejections at the FX10 | M–L |
| E9 | **Hard escape corridors.** A planned exit (surface track or via site and its first channel) is closed to other nets for the first negotiation rounds, then costly; a sealed planned net rips up its blockers, not itself. Replaces E4's soft bias for package exits. | No sealed planned exits (FX10 E1 ADC_OVR) | M |
| E10 | **Ring capacity and layer assignment.** Per layer, the capacity of each via-ring cut (gaps × tracks per gap minus blocked gaps) against the signal balls inside; balls assigned to layers by ring (outer two on the surface, then one ring per inner signal layer), checked by the E2 flow per layer with existing copper as removed capacity; overflow reported as a located issue. | Inner-layer share under the FX10 (half of F.Cu/B.Cu today); kit ring-to-layer map | M |
| E11 | **Bypass parts and supplies around the lattice.** Decoupling parts under an array only in via-free channels or on shared supply vias (at most two balls per via); supply tracks never cross a via-site column on a layer that uses it as an escape channel. | No cap pad on a via site; FX10 supply nets routed | M |
| E12 | **Fanout before critical routing.** Critical pairs at a package route through the reserved plan (paired boundary escape) instead of before it. | USB pair rules still met; FX10 blocked balls 0 with critical routes present | M |

E7–E12 come from the FX10 comparison and the BGA research of 2026-10-22: the
Infineon KIT_FX10_FMC_001 layout reserves one dogbone via per connected ball
in a quadrant-to-corner lattice before anything else, keeps rings 0–1 and a
central cross via-free (bypass parts sit there) and escapes rings 0–1 on the
top layer, ring 3 on the first inner signal layer and rings 4–5 on the second.
Every vendor flow found fans out before general routing (Altium Situs, Siemens
Xpedition: power/ground first); PAO plans exact pin access points jointly
before routing. Escape-flow models (Yan & Wong) take existing copper only as
removed capacity, so the site choice of E7 comes first.

**Open BGA benchmark (2026-10-23).** ULX3S (caBGA-381, 0.8 mm, 4 layers), Antmicro sdi-mipi-bridge (0.65 and 1.0 mm grids) and lpddr4-testbed (0.8 x 0.65 mm), prepared from their official repositories with routing stripped; the originals are the reference layouts. Every reference reserves one dogbone per connected ball in the outer rings, escapes rings 0–1 on the top layer and puts decoupling parts on the bottom inside the array. ULX3S routes 122 of 287 nets with the unified router; its failures are board-wide (power nets that cannot reach their planes, no terminal access), not only the BGA. lpddr4 and sdi-mipi-bridge exceed 35 minutes. OrangeCrab does not import (thermal_bridge_angle 90 pads).

**FX10 fast case.** `route-board --unified-save-inputs STATE` (also saves the
ball-grid fanout, `STATE.bga.json`) on CopperProbe, then
`copper replay-unified STATE --component U_USB [--refanout] [--set ...]`
routes only the FX10's nets (53) against the saved board. With
`--set local_escape_radius_nm=0` it takes about 4 minutes (17 open nets,
the same count as with the exact maze, which takes 26 minutes); E8 makes
the default fast. One run is not a measurement: a 0.05 change of
`present_factor` or `history_increment` moves the open count between 12
and 20, so each variant runs five times (`present_factor` 0.55, 0.6,
0.65; `history_increment` 0.75, 0.85) and is compared by median and
spread, and nets that fail in every run are the structural targets.
Report per variant: open nets, balls without a site,
sealed planned exits, exact-commit rejections near the package, seconds.

**Verification for every step** follows the D3 fix-plan pattern (see "How to carry out a step" below):
- unit tests that fail with the change removed;
- D3-A, and from E0 also D3-B, compared board by board against the baseline, with no clean board lost;
- the full-vertical comparison, with fingerprints identical while the option is off and every change explained when it turns on;
- the benchmark ladder from E2 on;
- run time per board;
- docs in a new `docs/escape-planning.md` describing the implemented contract (this plan stays the plan).

## How to carry out a step

1. Branch from current `main` as `claude/plan-escape-<step>-<topic>` (the
   `claude/plan-*` prefix also triggers the full-vertical comparison workflow;
   any `claude/**` push that touches `pcbir/**` triggers the D3 benchmark).
2. Read only what the step needs: this plan, the modules it names, and
   [routing-benchmark.md](routing-benchmark.md) for the benchmark tooling.
3. Add the option to `EscapePlanOptions` (off by default) and unit tests that
   fail with the change removed (mutation-check them).
4. Before spending CI time, reroute the target boards locally with
   `python -m pcbir.bench route <board.kicad_pcb> --out <dir>` and compare
   unrouted nets, vias, length and seconds with `main`.
5. Push and read the D3 result: the job fails if a clean board is lost; the
   comparison with `.github/routing-benchmark-baseline.json` is published as
   the "Benchmark comparison" annotation. Read the full-vertical comparison's
   fingerprint table.
6. From E2 on, run the benchmark ladder (`copper bench bench/ladder.json`, see
   the [routing refactor handoff](routing-refactor-handoff.md)) and record it.
7. Open a PR with the numbers. After merge, refresh the D3 baseline
   (`python -m pcbir.bench baseline <record> --from-record --source ...`,
   update the clean count in `tests/test_bench.py`) and this file's status table.

## Risks and open questions

- **Model against real geometry.** Rotated parts, octilinear routes and irregular pads may not match the channel graph. The plan is advisory, and every stub still passes `checked_access_path` and DRC. Unplannable pins fall back to today's per-pin path and are reported.
- **Over-constraining the routers.** Start with cost biases and measure before any hard reservation.
- **Defining cluster pairs.** Module plus header is clear; a pin cluster with nets spread to many parts is not. E0's classification decides the threshold.
- **Unified router per-land gap.** Closed on 2026-10-22: the unified router routes every separate land of a pad number, as the legacy router does. MySRaspiGW (D3-A 0049) is still open with the unified router: its header gaps fit one track only on the exact pad midline (±0.025 mm), which the 0.25 mm grid never reaches, so it needs E1's channel model.
- **Solver dependency.** E2 and E3 need none. E5 may want an exact solver; keep it optional and offline-checkable.
- **Expected gain is unmeasured** on CopperScript's own boards. E0 decides the order of E2 and E3: E3 first if two-end crossings dominate, as the D3 evidence suggests.

## Sources

- [Yan & Wong, "A correct network flow model for escape routing", DAC 2009](https://doi.org/10.1145/1629911.1630001)
- [Yan & Wong, "Correctly modeling the diagonal capacity in escape routing", IEEE TCAD 2012](https://scholars.hkbu.edu.hk/en/publications/correctly-modeling-the-diagonal-capacity-in-escape-routing/)
- [Ho et al., "Escape routing for staggered-pin-array PCBs", ICCAD 2011](https://ieeexplore.ieee.org/document/6105346/)
- [Bayless, Hoos, Hu, SAT-based multi-layer escape routing, ICCAD 2016](https://www.cs.ubc.ca/labs/isd/Projects/monosat/monosat_bga_iccad2016.pdf)
- [Gao, Dong, Cheng, Yu, "MORE-Router+", ISQED 2024](https://numbda.cs.tsinghua.edu.cn/papers/isqed24_2.pdf)
- [Jiao & Dong, ordered escape routing by min-cost multi-commodity flow, ASP-DAC 2016](https://ieeexplore.ieee.org/document/7428042)
- [Chen, Lin, Liu, Lin, "A High-Performance Ordered Routing Algorithm for Large-Scale WLCSP with Multi-Capacity", Micromachines 2025](https://pmc.ncbi.nlm.nih.gov/articles/PMC12734948)
- [Ozdal & Wong, "Algorithms for simultaneous escape routing and layer assignment of dense PCBs", IEEE TCAD 2006](https://scholars.hkbu.edu.hk/en/publications/algorithms-for-simultaneous-escape-routing-and-layer-assignment-o/)
- [Kong, Yan, Wong, "Optimal simultaneous pin assignment and escape routing for dense PCBs", ASP-DAC 2010](https://scholars.hkbu.edu.hk/en/publications/optimal-simultaneous-pin-assignment-and-escape-routing-for-dense-/)
- [Luo, Yan, Ma, Wong, Shibuya, "B-Escape", ISPD 2010](https://ispd.cc/ispd2026/slides/2010/2_05.pdf) ([abstract](https://scholars.hkbu.edu.hk/en/publications/b-escape-a-simultaneous-escape-routing-algorithm-based-on-boundar/))
- [Ma, Yan, Wong, "A Negotiated Congestion based Router for Simultaneous Escape Routing", ISQED 2010](https://www.isqed.org/English/Archives/2010/Technical_Sessions/123.html)
- [Lin, Wang, Kuo, Chen, Li, "A Complete PCB Routing Methodology with Concurrent Hierarchical Routing", DAC 2021](https://scholar.nycu.edu.tw/en/publications/a-complete-pcb-routing-methodology-with-concurrent-hierarchical-r/)
- [Chen et al., "A Matching-Based Escape Routing Algorithm With Variable Design Rules and Multiple Constraints", DAC 2023 / IEEE TCAD 2025](https://scholars.lib.ntu.edu.tw/handle/123456789/732753)
- [Li, Zhang, Xu, Liu, "FanoutNet", AAAI 2023](https://ojs.aaai.org/index.php/AAAI/article/view/26030)
- [Freerouting release notes (v2.3 BatchFanout)](https://github.com/freerouting/freerouting/releases)
- [Kahng, Wang, Xu, "The Tao of PAO: Anatomy of a Pin Access Oracle", DAC 2020](https://vlsicad.ucsd.edu/Publications/Conferences/377/c377.pdf)
- [Xu et al., "PARR: Pin-Access Planning and Regular Routing", TODAES 2016](https://www.cerc.utexas.edu/utda/publications/C178.pdf)
- [Yan, Wu, Ma, Wong, "On the escape routing of differential pairs", ICCAD 2010](https://scholars.hkbu.edu.hk/en/publications/on-the-escape-routing-of-differential-pairs/)
- [Ho, Shih, Chang, Cheng, "Layer minimization in escape routing for staggered-pin-array PCBs", ASP-DAC 2013](https://www.aspdac.com/aspdac2013/archive/program/2D_abst.html)
- [TI SPRABV2, "PCB design guidelines for BGA packages" (via sharing, decoupling under the array), 2019](https://ti.com/lit/pdf/SPRABV2)
- [AMD UG1099, BGA device design rules (layer count per ring)](https://docs.amd.com/r/en-US/ug1099-bga-device-design-rules/Layer-Count-Optimization)
- [Altium, fanout control (quadrant diagonals, away from centre)](https://www.altium.com/documentation/cstu/fanout-control?version=5)
- [Siemens, layout automation part 3 (fanout order)](https://blogs.sw.siemens.com/electronic-systems-design/2019/10/08/layout-automation-using-advanced-pcb-design-techniques-part-3/)
- Infineon KIT_FX10_FMC_001 PCB design data, measured in CopperProbeParts `evidence/fx10-kit-escape/`
