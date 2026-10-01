# Pass 17: compatible ordinary package-escape assignment

This extends pass 16's two-leg representation with a bounded assignment owner.
It changes newly proposed ordinary fanout only: no source connectivity, pose,
critical/GND reservation, routing layer rule or fabrication limit is changed.
The last full-board routing result remains pass 13, not this partial checkpoint.

## Algorithm and acceptance

1. Generate immutable legal radial/empty-domain two-leg choices, then retain the
   existing deterministic low-slack greedy selection as an incumbent.
2. For each pending pin with a legal domain, identify newly selected escapes
   actually conflicting with its alternatives. Build the local candidate-conflict
   graph lazily, using the original exact track/via/drill predicates. Conservative
   bounding boxes avoid unrelated exact queries; cached conflicts are scoped to
   the unchanged candidate geometry and physical rule/stackup snapshot.
3. Expand affected pins with off-ray alternatives even when they already have a
   legal radial choice. Preserve both legal diagonal/straight orders, not only
   the first. Existing alternatives/indices remain stable; additions are appended
   and deduplicated. Expansion occurs once per affected pin.
4. Filter alternatives against fixed outside selections. Solve the remaining
   local CSP with minimum remaining values, forward checking and backtracking,
   preferring existing selections where compatible. If a complete solution fails
   because outside selections consume alternatives, grow the group by one proven
   blocker, ranked by consumed-domain pressure and deterministic pin order.
5. Accept only a complete assignment for the pending pin **and every previously
   selected group member**. Outside selections stay fixed. Failed, unsatisfiable
   or over-budget groups keep the incumbent; partial search assignments never
   become copper. The final selection is independently materialized with exact
   incremental checks and full native hard DRC. A failed improvement falls back
   to greedy; a failing greedy proposal still rolls back to immutable input.

Defaults are eight pending-root trials, at most 12 pins per local group,
20,000 attempted assignments per root (including group growth), and 200,000
distinct pair queries for the whole allocation call. `joint_escapes=False` is
an experimental greedy control. `EscapeAssignmentOptions` configures these
finite budgets; no external solver dependency is required. Two-leg endpoint
generation keeps its original 256-site cap and 3 mm radius. Input copper is
never a replaceable graph node; optional net subsets cannot move other nets.

`FanoutResult.assignment` and CLI `fanout.assignment` report affected pads,
local groups, assignment states, pair queries, solution-found flags/reasons and
the final `native_accepted` gate. A solution-found flag is provisional when that
gate is false. Per-pin candidate counts now include any immutable domain
expansion, not just the initial radial/fallback pass. These observations do not
prove onward area connectivity, optimal wirelength or fabrication readiness.
Explicit track/via ownership and detailed anchor/cleanup verification from
pass 16 remain mandatory.

The separation of pin access, assignment, area routing and search/repair follows
[OpenROAD's documented detailed-routing stages](https://openroad.readthedocs.io/en/latest/main/src/drt/README.html).
The bounded local CSP/pressure heuristic above is our implementation choice,
not a claim to reproduce TritonRoute's optimizer, obtain optimal PCB escapes,
or include the still-pending onward-route cost/feedback owner.

## Matched evidence

Use exactly the pass-16 checkpoint: all 51 pass-13 poses, identical recomputed
global guides, and all critical/GND tracks/vias including the late ground
contact. Ordinary area copper is absent in this disposable experiment.

| Measurement | Pass-16 greedy | Joint default |
| --- | ---: | ---: |
| Escaped crowded pins | 72 | 74 |
| Pending crowded pins | 4 | 2 |
| Added escape tracks | 75 | 79 |
| Added through-vias | 72 | 74 |

U_MCU.61 (MCU_MODEM_RX) and U_MCU.39 (USER_LED_2) now escape. No previously
escaped identity is lost. The algorithm replaces the conflicting choices for
U_MCU.60/.38; it does not manually edit these references or special-case them.
Every pose and prior critical/GND copper prefix remains identical. Default
allocation performs 10,628 pair queries and four root trials, using respectively
2, 30, 2 and 51 search states. Neither search-state nor group-size budget is
exhausted. Runtime comparisons during simultaneous pytest are not benchmarks.

Fresh KiCad 10.0.6 refill reports eight existing library findings, 74 dangling
fanout vias and 94 unconnected items; no shorts, clearance, drill-spacing or
other hard copper findings. These are expected partial-fanout artifacts,
**not** full-board closure or a regression/improvement against pass 13's
34 open items. All three full-board ordinary open nets remain unclosed in the
last verified complete routing attempt.

U_MCU.62/.64 remain pending. Group expansion includes the implicated .60/.61/.63
escapes, but the generated domains still have no complete compatible assignment.
An endpoint-coverage control raises the enumeration cap to 1,024, covering all
528 off-ray lattice sites within the same radius. It still produces 74 exits
and the same two pending pins, with the same unsuccessful local state counts;
the default cap is therefore retained. This is not proof of geometric
unroutability beyond those domains.

A diagnostic-only call to the existing 0.25 mm octilinear surface search with
3 mm radius/12,000-state limit also finds no escape for either pending pin while
all 74 selected escapes remain locked. This does not exclude a multi-bend
alternative **combined with replacement of the competing selected escape**, nor
does it authorize moving input/critical/GND copper. Merely enlarging the area
search budget or routing-layer preference is not the next experiment.

## Tests and remaining work

Twelve new checks cover equal-slack traps, expansion of already-legal pins,
search/group/pair/trial limits, deterministic output, locked subset preservation,
whole-native rollback, layer/through-via/copper/drill conflicts, clearance
overrides, three-pin conflict-driven growth and installed-KiCad routed acceptance.
Existing fanout, exact access, CLI and full-routing script checks remain exercised.
Full suite: 503 passed, 459 upstream dependency warnings, in 201.18 seconds.

Next acceptance is a bounded constrained local escape search for both sides of
the remaining conflicts, with multi-leg anchor verification/ownership, or legal
whole-unit placement/critical-reservation feedback if no legal pattern exists.
Compare a compatible complete group; do not insert a lone escape and assume
its displaced neighbor can reconnect. Once the MCU exits survive allocation,
use `scripts/route_full_vertical.py` for the complete rerun, independent filled
zones and all-layer geometry/congestion review. That rerun is not performed by
this pass. Area-route escape replacement, onward-route cost, R5/R6b/R7/R9 and
RF/stackup/library/CAM qualification remain open.

Task evidence: `routing-review-pass17-joint-escapes/compare.py`, `comparison.json`,
`radius_sweep.py`, `radius-comparison.json`, matched `global.json`, native/KiCad
artifacts and `pressure-drc.json`; `local_paths.py` / `local-path-diagnosis.json`
records the fully locked local-search control. Original full-board artifacts
and user draft files remain untouched.
