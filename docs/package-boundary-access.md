# Ordinary package boundary access

## Decision and scope

CS-152 adds a **provisional local capacity gate** after ordinary dogbones,
specialized critical routing and selected early plane contacts. A legal via is
not automatically a usable exit from a dense package. All allocated ordinary
exits must also have mutually compatible paths beyond their package collars
before ordinary area routing starts.

[Ali et al. (2021)](https://pmc.ncbi.nlm.nih.gov/articles/PMC8056246/)
use boundary terminals as a hand-off between local escape and intermediate
routing. [TritonRoute's architecture](https://openroad.readthedocs.io/en/latest/main/src/drt/README.html)
also separates pin-access analysis from subsequent detailed routing. These are
architectural precedents, not drop-in PCB solvers: actual pads, through-vias,
mechanical geometry, hard macros and critical profiles need their own checks.

This covers **allocated ordinary fanout pins only**, not joint critical
differential launches or every power/GND contact. CS-152 analysis produces
provisional witnesses. CS-153 materializes their owned paths **only after the
entire package-access preflight passes**, then gives detailed routing an exact
layer-aware boundary anchor. Neither a port nor its stub proves remote-pad
connectivity; the area router must still create real end-to-end routes.

## Algorithm

`pcbir/boundary_access.py` provides immutable options, collars, ports, per-pin
analysis and allocation results. `analyze_boundary_access(board, fanout)`:

1. Rejects stale geometry/rules and missing fanout-owned track/via occurrences.
   Rechecks an exact connected pad-to-via launch, including internal pad groups.
   An arbitrary claimed point is not a terminal.
2. Bounds transformed courtyard (body size if absent) **and actual land shapes**,
   expanded by 0.25 mm. Back-side mirroring and 45-degree placement are included.
   World-axis collars are conservative hand-off regions, not new keepouts or
   electrical/schematic coordinates.
3. Samples ports beyond each edge using the exact anchor projection plus
   neighbours: eight coarse points per edge at 0.5 mm, with empty/conflicting
   domains refined to 32 points at 0.1 mm. The port capsule clears the collar,
   not just its center. All paths use exact straight/45-degree legs.
4. Uses allowed signal layers reached by the actual via span. Dedicated planes
   cannot carry foreign signals. Input vias/drills remain obstacles over their
   physical spans; no new via, overlap exemption or clearance is invented.
5. If refinement is insufficient, heading-aware octilinear A* can reach **any**
   collar edge instead of only sampled ports. Only straight/45-degree heading
   changes are generated. Each layer has a 12,000-state budget; search is confined
   to the collar/port envelope plus 2 mm. Collinear edges are merged and the
   emitted geometry is rechecked at acceptance. Results/cache are pin-local.
6. Allocates low-slack pins first, then uses the bounded exact conflict/CSP
   controller to repair mutually blocking selections. Distinct nets cannot share
   one copper channel on a layer; distinct permitted layers can carry paths at
   the same XY position. Independent per-pin success does not certify capacity.
7. Temporarily materializes the entire witness set and runs native DRC, excluding
   only expected open-net/unfinished-area findings. A rejected CSP proposal falls
   back to greedy; rejected greedy certifies no ports. The input board is unchanged.

The 2 mm option bounds cheap-family extra Manhattan distance and maze spatial
padding, not a hard trace-length limit. A failed finite domain/state budget means
**access unproven**, not physically impossible. Manufacturing rules never relax.

## Owned hand-off to area routing

`reserve_boundary_access(board, fanout, boundary)` is an immutable transaction.
It requires a complete unique port set matching the ordinary launch identities,
a matching physical-board digest, current transformed collars and actual port
clearance beyond the named collar edge, and the actual owned launch track/via
occurrences. It appends only witness tracks not already present, rechecks every
terminal-to-launch-to-port path and native DRC, and returns an extended
`FanoutResult`. A rejected reservation raises without modifying the input.
Existing same-net tracks may support a path but never acquire cleanup ownership.
Critical, hard-macro and selected-plane copper remains owned by its original stage.

`FanoutResult.accesses` retains the dogbone positions for pattern negotiation.
`boundary_accesses` contains `RoutingAccess(position, layer, launch_position,
path)` descriptors; `routing_accesses` merges these over ordinary via anchors.
The detailed router validates existing explicit geometry before using a port and
offers exactly **one selected-layer node**, not every layer of the launch via.
It rejects a missing/wrong-net path, disconnected terminal, prohibited layer or
insufficient actual via span rather than falling back to pad-center access.

Subset repairs retain the immutable reservation prefix and consume the same
typed anchors. Full-route cleanup removes only explicitly owned occurrences on
abandoned nets; it cannot delete input, macro, critical or ground copper.
A same-surface port may lose an unnecessary owned via while retaining its
terminal/path chain. An off-surface port still requires the actual layer transition.
These are physical routing descriptors, not electrical or schematic IR geometry.

## Pipeline, placement and reporting

`--fanout` automatically enables this gate. `--package-boundary-step-mm` controls
coarse port sampling; finer user spacing also reduces the refinement step.
API callers configure sampling/search/CSP bounds through
`PackageAccessOptions.boundary_options`. `maze_escapes=False` restricts domain
generation, not the mandatory capacity gate.

Boundary pending identities join ordinary/selected-plane pending pads, so existing
whole-unit placement feedback can move the blocked package within fixed poses,
allowed orientations, rigid units, keepouts and proximity rules. Acceptance
preserves previous boundary identities, ordinary launches and critical connectivity.
Incremental sparse-component moves with no committed boundary paths recompute
evidence. With nonempty owned boundary reservations, the incremental controller
conservatively requests **full preflight** rather than reclassifying or relabelling
their copper after a move. This limits the fast path, not legal placement repair.
Stale/missing evidence or a newly closed channel also triggers full fallback.

`package_access.boundary` reports provisional/reserved scope, `materialized`,
source digest, anchor count, tracks added at hand-off, readiness/native acceptance,
collars, pending identities, port layer/edge/position/path, candidate counts,
diagnostics, maze states/candidates and assignment budgets/trials. Progress phase
`package_boundary_access` separates analysis from critical and area routing;
`package_boundary_reservation` times the commit-time ownership/geometry checks.
Port paths describe the pre-area reservation; abandoned paths may be pruned
from the final board and must not be mistaken for completed-net evidence.
The normal profiled Make workflow needs no example-specific engine code.
A blocked gate still means zero ordinary area-search passes and no fabrication
readiness; uncommitted witness tracks do not appear in exported diagnostic copper.

## Verification

Tests cover legal enclosed dogbones, incompatible channel domains, independent
layers, plane/explicit-layer policy, narrow-window refinement, stale/missing
ownership, disconnected anchors, transformed collars and native rollback.
Pipeline tests check zero-pass blocking and placement-owner feedback when
dogbones succeed but boundary access is missing.
An installed-KiCad test independently checks a temporary witness board: only
expected unconnected/dangling items remain, not geometry/clearance violations.
Those findings deliberately prevent a passing full-board connectivity claim.

`tests/test_boundary_routing.py` additionally covers exact-layer source/target
nodes, fresh atomic materialization, unchanged input ownership, stale/invalid
path rejection, surface-via cleanup, subset repair and abandoned-net cleanup.
An installed-KiCad fixture verifies a complete boundary-to-area route with no
unconnected or geometry findings. Pipeline tests ensure that only a ready
preflight materializes paths and passes the typed anchors to detailed routing.
This small fixture is not evidence of full-vertical closure.

Matched full-vertical placement fingerprint:
`5d1b87bb622bd415def17b3c54bd23db266dee9b4a9a08d8c74632bc9551163e`.
All 76 dogbones pass. Straight/elbow allocation initially proves 75/76 boundary
paths; `U_MCU.23` has zero domain choices. The multi-bend fallback recovers that
pin, giving **76/76 compatible local witnesses with native geometry acceptance**.
Boundary time under cProfile increases from 8.713 to 20.555 seconds. Evidence is
in ignored `build/package-pattern-validation/boundary-20261005/` and
`boundary-maze-20261005/` respectively. Both experiments include macro reservations
but **exclude critical, selected-plane, ordinary area and filled-zone verification**.
This is not a complete-board routing pass.

A subsequent profiled **alternate-owner preflight** rebuilt critical routing
around a newly allocated ordinary pattern, selected 44 dense GND contacts and
then allocated **76/76 boundary witnesses against all those reservations**.
All six critical groups passed (four RF groups and both USB pairs), with no
pending contact, failed critical net or hard native finding. The boundary phase
took 20.664 seconds in its progress timing. The saved evidence is
`build/package-pattern-validation/boundary-owners-20261005/preflight-report.json`.
This validates the preflight, not ordinary area routing, filled-plane continuity
or production signoff. The one-off driver's `boundary_seconds` field measures
only post-preflight report assembly in this mode; use the recorded phase timing.

The heading-aware maze caches exact undirected edge predicates only within one
immutable layer/domain search. A matched rerun preserved identical port paths
and 22,270 total MCU fallback states while reducing maze `can_track` calls from
66,820 to 16,895. Observed profiled ordinary boundary time fell from 20.555 to
13.160 seconds (`boundary-cached-20261005/`). Wall times are measurements, not a
guaranteed production speedup; the predicate-count/geometry comparison isolates
the intended cache effect without relaxing any check.

CS-152 affected regression run: **192 passed, 10 skipped**. The skips are GNU Make
integration tests because Make is absent from PATH; portable build assertions
and installed-KiCad tests ran. This is the affected routing/CLI/build set, not a
claim that every repository test or the full-board manufacturing flow ran.

CS-153 final affected integration/build run: **220 passed, 10 skipped**; GNU Make
is still absent from PATH. All 29 focused boundary-handoff tests passed, including
the installed-KiCad complete-route check. A broader detailed/critical regression
run also passed (328 tests, 10 Make skips); the final collar/launch-width guards
and additional ownership cases were verified by the later 220-test run.
The profiled shared full-vertical workflow was started in
`build/full-vertical-boundary-20261005/`. At milestone commit it is still searching
the modem USB pair, before ordinary area routing. This in-progress run began
before the final guard/report refinements; it is diagnostic, not final-commit
manufacturing evidence. Saved fill, native DRC and per-layer review remain pending.

Remaining work:

- Negotiate dogbone choice and onward path together, beyond owner orderings.
- Jointly allocate specialized critical/power/GND access domains.
- Derive directional placement margins and escape order from bank/channel demand.
- Rerun the full board with critical/plane obstacles, independent refill/DRC,
  per-layer review and manufacturing gates before claiming closure.
