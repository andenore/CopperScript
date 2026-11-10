# Routing

How `copper route-board` turns a placed board into copper, the rules every
routing change keeps, how changes are measured, and the open algorithm work.
Specialized contracts have their own pages: [filled-copper feedback and routing
invariants](filled-copper-feedback.md), [escape planning](escape-planning.md)
(with its [plan](escape-planning-plan.md)), [placement](placement-internals.md),
[local critical routing](local-critical-routing.md),
[hard macros](hard-macro-routing.md) and [progress events](routing-progress.md).
Measured performance is in [routing performance](routing-performance.md).

Physical commands and the editor resolve the board's construction through
`copper-pcb::configuration` before placement. Source stackup and layer
requirements determine the layer count when `--layers` is omitted; an explicit
count must agree with a declared stackup. See [board configuration](board-configuration.md)
for the ownership of physical requirements, build preferences and invocation settings.

## Pipeline

```
compile ─► place ─► global guides ─► escapes and bypass ─► critical ─► fanout ─► UNIFIED ROUTER ─► post ─► verify ─► report
                    (negotiated       reservations          (pairs,     (BGA      (all ordinary     (land     (exact DRC,
                     congestion)                             clocks,     dogbones)  nets, one         closure,  native refill
                                                             RF, buses)             negotiation)      smoothing) and DRC)
            ▲                                                                                                   │
            └────────────────────── bounded congestion-driven placement trials ◄────────────────────────────────┘
```

A run's progress phases follow that order: `placement_global` and
`global_route`; `ordinary_escapes` and `bypass_reservation`; `critical_group`;
`bga_fanout`; `ordinary_area` and the `unified_*` phases (setup, iterations,
commit, pour re-check, congestion); `land_closure_native_drc`;
`placement_feedback`; `final_contacts_native_drc`; `independent_kicad` (the
plane check); `export`. `--stop-after placement` and `--checkpoint-dir` /
`--reuse-placement` cut a run at the placement boundary.

With `--progress`, a run also writes `<report>.timings.json` beside the report:
wall-clock seconds per stage (each placement-feedback trial separately) and
`total_s`. Stages nest, so a `global_route` entry contains its
`global_iteration_N` entries and the entries do not sum to the total. The
timings stay out of the report itself, whose digest is recorded in the summary.
The unified router's `seconds` on its progress events is the time since its
previous timed event. `copper bench` copies the timings into each board's
`phases`.

- **Fixed copper first.** Hard macros (including library fanout macros),
  required via-in-pad arrays, user-locked copper and source keepouts are
  materialized before routing and never move.
- **Global guides.** A negotiated-congestion router over tiles gives every net a
  guide and a layer plan, used as soft costs later.
- **Escapes and bypass reservations.** Package exits and the declared bypass
  links of decoupling capacitors are planned and reserved before critical
  routing; reservations are admitted read-only against the exact board
  (`bypass_reservations`). Escape planning can join exit side, layer and order
  per package (`--escape-plan`, `--bus-layers`, `--array-flow`, `--via-lattice`;
  opt-in, see its plan).
- **Critical stage.** Differential pairs, clocks, RF feeds and length-matched
  buses route first under their own rules: coupled pair search with bounded
  escalation and rip-up of committed pairs that share a package, local surface
  trees for short critical nets (`local_critical`), and post-route length
  tuning (the pair, unit and group tuners).
- **Fanout.** BGA dogbones and the cross-surface bypass links of decoupling
  capacitors.
- **Unified router.** One PathFinder negotiation over a shared multilayer lattice
  for every remaining net, with exact commit checks (below).
- **Post-processing.** Land closure, route smoothing, stub cleanup and
  single-layer via sweep; declared zones are stitched automatically.
- **Verification.** Exact physical DRC on the explicit copper, then a refill and
  DRC of the written board: CopperScript's own filler and DRC by default, or
  KiCad's with `--verify-plane-fill KICAD_CLI`. Opens the refill reports feed at
  most a bounded number of filled-copper repair trials.
- **Placement feedback.** Failed nets drive up to `--congestion-feedback-trials`
  placement proposals; each reruns the pipeline tail and is kept only if the
  whole-board result improves.

## Rules every stage keeps

- **Never block.** Every stage returns its best result and a list of findings. A
  stage failure becomes a report item and the run continues; pass or fail is
  computed once, from the final board.
- **One negotiation.** All ordinary nets share one resource model and one
  rip-up-and-reroute loop. Priority is a cost multiplier, never a lock. Only
  fixed copper is immovable.
- **Planes are declared, not searched.** Plane and split-plane regions come from
  source zones. Plane nets are satisfied by short taps; a zone outline or a
  same-net label never counts as connectivity.
- **Exact geometry decides legality.** The lattice and rasters only accelerate
  search; every commit is checked against the exact clearance index.
- **Native verification at the end.** One refill and DRC of the written board,
  plus bounded repair trials; no refill inside search loops.
- **Determinism.** Output does not depend on hash order or thread timing.

The [routing invariants](filled-copper-feedback.md) add the package-exit,
macro, via-array and rollback rules a mutable trial must keep.

## Unified router

`crates/copper-route/src/unified_router/`. One integer-id lattice per board
(base pitch 0.25 mm, `--unified-pitch-mm`), with sparse extra nodes at pins,
macro ports and via sites and finer patches over ball arrays
(`fine_regions=arrays`). Per node it keeps locked-obstacle rasters per
width/clearance class, usage, history and owners in flat arrays.

- **Occupancy.** A committed path claims every node within its inflated
  envelope; other nets may enter at present and history cost.
- **Iteration.** Route every net, then rip up and reroute nets on overflowed
  nodes or failed exact commits, growing the present penalty and history. Keep
  the best incumbent by (opens, overflow, exact violations, length, vias); stop
  at zero overflow with a clean exact check or at the iteration cap.
- **Net modes.** Multi-terminal trees, coupled pairs, buses with a length term,
  wide power traces by rule width, and plane taps to any legal via site inside
  the net's plane region.
- **Costs.** Length, layer and direction preference, guide exposure, vias,
  bends (`bend_cost` 0.25) and breakout necks (`neck_cost` 1.0).
- **Commit.** `RoutingClearanceIndex` with `remove(owner)`; one shared index
  across commit, repair and pour re-check.

Its 78 options are listed with types and defaults by
`copper route-board --list-unified-options` and set with
`--unified-set KEY=VALUE`.

## Options and profiles

`--routing-profile quick|balanced|thorough` sets the filled-copper and
decoupling work budgets; layers (`--layers`), fabrication rules
(`--fab-profile`), board size and the time budget (`--unified-time-budget-s`)
are separate. `copper route-board --help` lists every option with its default;
the [CLI workflow](cli-workflow.md) records removed options and what replaced
them. Solver controls no caller set were removed on 2026-11-09 and keep their
former defaults.

## Measuring a change

The metric is the benchmark ladder (`copper bench bench/ladder.json`): the CI
[Rust workflow](../.github/workflows/rust.yml) routes the example boards and the
consumer boards (CopperLedRing, CopperAssetTracker, and CopperProbe where the
token allows) and compares completion, opens, DRC, native connectivity,
routing-intent findings and time against `bench/ladder-baseline.json`. Locally,
set `COPPER_BOARDS` to a folder holding the consumer checkouts.

- A change is good if completion goes up or time goes down without other
  regressions. Evidence is the ladder result, the diff and the tests.
- A change that should not alter copper is checked by comparing each board's
  written `board.kicad_pcb` before and after.
- No new `verification/` bundles unless the owner asks; changed inputs
  invalidate earlier evidence.
- No board-shaped heuristics: a fix that only suits one board's geometry goes in
  that board's source (constraints, zones, macros).
- Iterate on failures with `copper issues RUN` and `copper view RUN --issue ID`
  (`copper agent-guide iteration`), not one-off scripts.

The DRC column counts hard physical errors only, so a board can show DRC 0 and
still miss its signal-integrity intent. Two columns carry that intent, read from
the route-board report:

- Intent is "critical unrouted / skew / length-match": the critical groups the
  router left unconnected (`CRITICAL-UNROUTED`, from `critical.nets`), and the
  differential pairs over their skew limit (`DRC-DIFF-SKEW`) and length-match
  groups over theirs (`DRC-LENGTH-MATCH`), both from `summary.drc.budget_warnings_by_code`.
  The ladder reports a regression when any of the three rises against the
  baseline. A baseline written before a field existed is treated as unknown for
  that field and is not flagged.
- Crossings is the number of net pairs that cross on one layer in the escape
  model (`escape.crossing_pairs`, the `ESCAPE-CROSSING` findings). It is advisory
  and mostly a property of the placement, so it is shown but not guarded.

Status (ladder of 2026-11-09, native plane check, two boards at a time on a
shared 4-core container):

| Board | Completion | Opens | DRC | Intent | Crossings | Time |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| browser-routing-demo | 1.0 | 0 / 2 | 0 | 0 / 0 / 0 | 0 | 0.0 s |
| browser-routing-weave | 1.0 | 0 / 8 | 0 | 0 / 0 / 0 | 4 | 0.3 s |
| round-led-ring | 0.980 | 1 / 51 | 0 | 0 / 0 / 0 | 0 | 9 s |
| nrf52 | 1.0 | 0 / 35 | 0 | not yet measured | not yet measured | 5 s |
| full-vertical | 1.0 | 0 / 86 | 0 | not yet measured | not yet measured | 21 s |
| cm4 | 1.0 | 0 / 21 | 0 | not yet measured | not yet measured | 4 s |
| fx10-bridge | 0.953 | 4 / 85 | 0 | 1 / 4 / 1 | 50 | 222 s |
| led-ring | 1.0 | 0 / 51 | 0 | not yet measured | not yet measured | 3 s |
| asset-tracker | 1.0 | 0 / 124 | 0 | not yet measured | not yet measured | 39 s |
| copper-probe | 0.983 | 4 / 237 | 0 | not yet measured | not yet measured | 4995 s |

The Intent and Crossings columns were measured on 2026-11-10 for the four boards
that ran (browser-routing-demo, browser-routing-weave, round-led-ring,
fx10-bridge); the other rows keep the numbers of the 2026-11-09 ladder and have
no intent measurement yet. fx10-bridge has one critical group unrouted, four
differential pairs over their skew limit and one length-match group over its
limit while its DRC column reads 0.

The goals the router was rebuilt for: CopperProbe (6 layers, 169-ball 0.75 mm
BGA, QFN ADC, USB 3 pairs) with 0 opens, 0 exact DRC errors, native DRC clean
after refill and at most 30 minutes; the example boards at 100 % completion,
exact DRC clean and at most 5 minutes each; no regression on CopperAssetTracker
and CopperLedRing. Met: exact and native DRC are clean on every board,
CopperAssetTracker and CopperLedRing route completely, and the small examples
finish in seconds. Not met: CopperProbe still has 4 opens and takes about
83 minutes; round-led-ring has 1 open and fx10-bridge has 4. Those opens, and
CopperProbe's time, are the measure for the open work below and for the
[closure plan](rust-routing-closure-plan.md).

## Open work

Verify a gap against current code and a targeted case before starting it.

### Access and incremental ownership

- [ ] Retain unaffected ordinary escape prefixes during the initial critical
  probe (the partial rebuild after a disposable probe is distinct). Rebase
  occurrence-counted ownership and keep macro/required-array rollback.
- [ ] Extend joint domain allocation beyond alternate owner ordering to
  specialized paired/single-ended critical and dense power/GND access, without
  breaking pair coupling or anchors.
- [ ] Rank complete access patterns with onward guide/layer congestion and allow
  bounded transactional replacement.
- [ ] Extend directional placement estimates to exact nearby-obstacle/access
  demand where the pad-bank model is insufficient.
- [ ] Extend incremental repair to moved crowded packages and macros only with
  complete pad-level ownership and fresh joint access validation.

### Route quality and bounded search

- [ ] Improve feasible incumbents with bounded wider guides and branch-safe
  octilinear cleanup; preserve junctions, pads, vias and owner scope.
- [ ] Extend exact same-net crossing preferences from package escapes to
  ordinary area topology, allowing intentional shared trunks and T junctions.
- [ ] Port joint placement of mutually blocked decouplers
  (`support_pattern_trials`): the Python planner had it, the Rust planner
  reports it as not implemented. Then demonstrate it on an authored mutable
  CopperProbe bank with onward D18 access.
- [ ] Localize neutral-cost fallback and use clearance-aware demand/failure
  pressure, keeping explicit layer/reference restrictions.
- [ ] Reserve whole-bundle corner-wrap corridors, not only each pair's terminal
  band.
- [ ] Extend endpoint-only path extraction to real track-interior junctions.
- [ ] Arched corners: concentric arcs for `tuning_group` lanes beyond a pair,
  re-arching nets the filled-copper repair reroutes, and arcs in the browser
  routing lab.

### Power, planes and packages

- [ ] A power/ground copper-area planning stage shared by placement and
  routing. After coarse placement and before package access, plan rail
  partitions, ground continuity, local bypass/return vias, sensitive-node
  exclusions and reserved signal corridors; feed infeasible access back to
  placement. Validate continuity on fresh native-filled fragments, with bounded
  trials, exact ownership and complete rollback. Start from a consumer-owned
  single-domain baseline, and include partial-zone fixtures: a source outside a
  local supply island, disconnected same-net islands, and a filtered downstream
  domain. (An earlier stage ran only with the legacy router and was removed
  with it on 2026-10-26.)
- [ ] Explicit source layer roles and dedicated-plane semantics beyond coverage
  inference, keeping zone, stitching and per-pad controls.
- [ ] Coherent whole-package fanout planning before mutable local feeds:
  geometry-derived dogbone patterns, joint signal/power/ground contacts,
  verified layer exits, and coordination with support placement and copper
  areas. Joint escape side, layer and order follow the escape planning plan.
- [ ] Evaluate automatic multi-rail partitioning with layer/corridor,
  neck/current, fill and reference-plane checks.
- [ ] Extend movable-macro feedback with complete boundary/owner revalidation;
  extend mirrored/curved/local-zone macro variants only with exact
  serialization, fingerprints, native fill/export and independent CAM coverage.

### Assessment

- [ ] Evaluate whole-board Pareto feedback and connection-level complete-blocker
  ripping, separately from failing-tree branch checkpoints.
- [ ] Extend geometry-bound final-copper return/impedance/current checks and
  native acceptance cases. Routing closure never substitutes for CAM or
  hardware.
