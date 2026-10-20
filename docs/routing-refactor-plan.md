# Routing refactor plan

Status: accepted 2026-10-19 (decisions in section 6). Owner: andenore. This plan supersedes the
routing parts of `TODO.md`, `docs/routing-backlog.md` and
`docs/power-area-planning-plan.md` while it is active. Where they conflict,
this document wins.

## 1. Goal

A routing engine that is generic enough to route many kinds of boards, and that
can route CopperProbe as its hardest current case.

**Done means:**

| Board | Target |
| --- | --- |
| CopperProbe (6 layers, 169-ball 0.75 mm BGA, QFN ADC, USB 3 pairs) | 0 opens, 0 exact DRC, native KiCad DRC clean after refill, wall time ≤ 30 min |
| CopperAssetTracker (recorded 2026-10-11 candidate) | No regression in completion or DRC; time no worse than today |
| CopperLedRing (2 layers) | No regression in completion or DRC; time no worse than today |
| `cm4`, `full-vertical`, `nrf52`, `round-led-ring` examples | 100 % completion, exact DRC clean, each ≤ 5 min |

CopperAssetTracker and CopperLedRing must keep working at every merged phase:
their pinned Make targets either run unchanged or get a matching update in
their own repositories in the same change.

Electrical, SI, PDN and CAM qualification stay separate gates, as today. This
plan is about getting complete, legal copper quickly and repeatably.

## 2. Why the current design stalls

Measured on CopperProbe, run `20261016T071737786852Z`:

- The run took 13.4 h. About 99 % went to package access and critical search
  (`package_search_tier`, `package_access_trial`, `package_search_fallback`,
  `package_pattern_trial`, `package_critical_search`, `critical_group`). Global
  routing took about 2 minutes. Ordinary detailed routing never ran.
- Since then, every run has stopped at the power-preparation or package-access
  gate. The best whole-board state is about 183 opens.

There are five structural causes.

1. **Hard-gate cascade.** `flow.run_routing_pipeline` returns `FAIL` before
   `route_detailed` when `access.ready` is false or power preparation is not
   `native_connected`. One unsolved exit therefore hides every other result.
2. **Freezing instead of negotiation.**
   - Critical routes, bypass identities, accepted power ledgers and boundary
     reservations are all locked before ordinary routing starts.
   - Inside `route_detailed`, each routed net is added to `RoutingClearanceIndex`
     as a hard obstacle (`detailed.py` around line 599). The present and history
     costs therefore barely act: this is sequential routing with reordering, not
     PathFinder.
   - The index has no `remove`, so every rip-up rebuilds it.
3. **Search over candidate pools to satisfy the gates.**
   - `package_access` reruns the full critical router about 90 times in the
     worst case: 2 tiers × about 5 strategies × up to 9 placement trials.
   - Power preparation allocates over contact and area pools: 4,096 allocator
     states, then rejected.
4. **Accretion.**
   - About 22,700 lines sit in special stages (critical, package, power, joint,
     fanout, bypass, pair, escape), against about 4,400 in the core routers.
   - About 7,400 lines exist only to drive the gates.
   - `route-board` has about 90 CLI options.
5. **No fast feedback.** No benchmark ladder exists, and a CopperProbe run takes
   hours. Every change is judged by a bespoke evidence bundle; CopperProbe now
   has 280 `verification/` directories.

## 3. Target architecture

```
compile ─► place ─► fixed copper ─► planes ─► global guides ─► UNIFIED ROUTER ─► post ─► verify ─► report
                     (macros,        (layer     (route_global,   (all nets,         (tune,   (exact DRC,
                      via-in-pad,     roles,     soft costs)      one negotiation)   smooth,  native refill
                      lib fanouts)    taps)                                          pours)   once)
                         ▲                                                                       │
                         └──────────── optional bounded placement nudges driven by opens ◄───────┘
```

### Rules the architecture enforces

- **Never block.** Every stage returns its best result, plus a list of
  findings. A stage failure becomes a report item, and the run continues to the
  end. Pass or fail is computed once, from the final board.
- **One negotiation.** All routed nets share one resource model and one
  rip-up-and-reroute loop. This covers ordinary signals, differential pairs,
  length-matched buses, wide power traces and plane taps.
  - Priority is a cost multiplier, never a lock.
  - Only *fixed copper* is immovable: hard macros (including library fanout
    macros), via-in-pad arrays, user-locked copper and source keepouts.
- **Planes are declared, not searched.** Plane and split-plane regions come from
  source. Plane nets are satisfied by short taps to the plane, not routed.
  Leftover rails are routed as wide traces and may be poured after signal
  routing.
- **Native KiCad runs at the end, not in loops.** One refill and DRC at
  verification. At most two bounded "fix reported opens" rounds re-enter the
  router.
- **Exact geometry stays the source of truth for legality.** The grid and
  rasters are only an acceleration structure. Every commit is checked exactly.

### Unified router (new module `pcbir/unified_router.py`)

**Grid.** One integer-id grid per board, built once.
- Default pitch 0.25 mm.
- Sparse extra nodes for pins, macro ports and via sites, as `_build_grid` does
  today.
- Per node: locked-obstacle mask per width/clearance class, `usage`, `history`
  and an owner set. Use numpy if accepted (decision D1), with a pure-Python
  fallback.

**Occupancy.** A committed path claims every node within its inflated envelope
(half width + clearance + the half width of the class being searched). Other
nets may still enter those nodes at present/history cost. That is the
negotiation.

**Iteration.**
1. Route every net.
2. Each later iteration rips up and reroutes nets that touch overflowed nodes,
   plus nets that failed exact commit.
3. Grow the present penalty and accumulate history.
4. Keep the best incumbent, ordered by (opens, overflow, exact violations,
   length, vias).
5. Stop at zero overflow with a clean exact check, or at the iteration cap.

**Net modes.**
- *Tree:* multi-terminal growth toward the nearest remaining terminal, lifted
  from `detailed._route_net`.
- *Pair:* coupled spine A* from `pair_search._search_steps`, with both mitered
  lanes charged to usage. Transitions come from the `pair_vias` geometry.
- *Bus:* a length term in the cost during routing. `critical_tuning.UnitTuner`
  and `critical_group_tuning.GroupTuner` run after convergence.
- *Power trace:* the width class comes from the net rule, using the same search.
- *Plane tap:* the target set is every legal via site inside the net's plane
  region within a radius of the pad. Cost includes the stub length. A
  decoupling capacitor's tap is drawn toward its associated pin.

**Costs.** Reuse the `_search_once` terms: length, layer and direction
preference, guide exposure, vias and bends. Add `TerminalPreferences` for
outward exits and `BreakoutRegions` for neck-down. Add a per-class
present-penalty multiplier, so critical nets win contests by price rather than
by lock.

**Commit check.** Use `RoutingClearanceIndex`, extended with `remove(owner)` and
binned holes. Run `run_physical_drc` only at final sign-off.

## 4. Reuse map

These are verdicts from a module-by-module review of the 2026-10-19 working
tree. **Keep** means use it as is. **Lift** means move the named functions into
the new structure and drop the rest. **Retire** means delete it once nothing on
the default path imports it (phase P7).

### Keep (core primitives)

| Module | Role in new design |
| --- | --- |
| `physical.py`, `physicalize.py`, `compiler.py`, `parser.py`, ERC, backends | Unchanged: model, language and export |
| `geometry.py`, `routing_clearance.py` | Exact legality. Add `remove(owner)`, binned holes and per-net counters |
| `routing.py` (`route_global`), `layer_assignment.py`, `global_terminals.py` | Global guides as soft costs. Feed macro ports to `register_terminal_capacity` |
| `routing_costs.py`, `routing_guides.py`, `routing_layers.py`, `routing_vias.py` | Cost and layer helpers. `dedicated_plane_layers` gives a fallback plane inference |
| `pin_escape.py` (`RoutingAccess`, `checked_access_paths`), `pin_access.py` (`local_access_path`), `terminal_preferences.py`, `breakout.py`, `access_sites.py` | Terminal access, lead-ins, via-site proposals, neck-down |
| `hard_macros.py`, `macro_bindings.py`, `macro_definitions.py`, `pad_via_arrays.py`, `pad_stitch.py` | Fixed copper, including library fanout macros (extended in P4) |
| `zone_geometry.py` (`lower_zone_outline`, `deferred_zone_nets`, `ZoneRoutingReservations`) | Plane and split-plane regions from source. Reserved zones become obstacles or costs |
| `route_cleanup.py`, `route_style.py`, `route_smoothing.py`, `route_closure.py` (`close_detailed_lands`) | Post-processing |
| `drc.py`, `process_drc.py`, `copper_connectivity.py` | Final sign-off and opens report (not inside loops) |
| `plane_verify.py` (`verify_filled_planes`, `native_object_map`), `filled_connectivity.py`, `decoupling_fill.py` | The single native refill/DRC, islands, and capacitor supply/return checks |
| `power_network.py`, `power_extract.py`, `power_integrity.py`, `power_planning.py`, `power.py` | DC/IR screening and lowering. Advisory reports only |
| `routing_benchmark.py` (`summarize_run`, `compare_runs`), `routing_acceptance.py`, `routing_metrics.py`, `access_diagnostics.py` | Benchmark ladder (P1) |
| `placement*.py`, `clusters.py`, `incremental_placement.py`, `routeflow.py` (`optimize_placement_for_routing`, `detailed_failure_trials`) | Placement, plus the optional outer loop driven by opens |
| `critical_review.py`, `pair_geometry.py`, `route_path_geometry.py`, `return_paths.py` | Read-only reports |

### New (added by the refactor)

| Module | Role |
| --- | --- |
| `unified_router.py` | Negotiated-congestion (PathFinder) router on a numpy legality raster; exact commit through `routing_clearance` (P5) |
| `native/astar.c` + prebuilt `.so`/`.dll`, `tools/build_kernel.py` | Optional C A* kernel with identical results; pure Python fallback (`COPPER_NO_NATIVE=1`) (P6) |
| `bga_fanout.py` | Deterministic quadrant dogbones for full-grid ball arrays (P4) |
| `power_islands.py` | Automatic split-plane islands for decoupled rails on a spare plane layer (P3, opt-in `--power-islands`) |
| `route_summary.py`, `plane_estimate.py`, `bench.py`, `bench/ladder.json` | Pipeline-independent completion summary, raster plane estimate, benchmark ladder (P1) |

### Lift (useful parts inside large modules)

| From | Lift | Drop |
| --- | --- | --- |
| `detailed.py` | `_access_candidates`, `_surface_accesses`, `_escape_terminals`, `_access_path`, tree growth in `_route_net`, the `_search_once` cost formula and `_neighbors`, `_erase_path_loops`, `_compact_path`, `_merge_collinear_tracks`, `_prune_fanout_copper` | Pass loop, `_repair_from_passes`, `_SearchReuse`, `_BranchCheckpoint`, neutral-fallback rerun |
| `pair_search.py`, `pair_vias.py`, `pair_terminals.py`, `pair_refine.py` | `_search_steps` spine A*, `_lane_paths`, `_miter`, `_ports`, `_legal`, `_goal_paths`; `transition_spacing`, paired via and return-via construction; `prepare_pair_terminals`; `paired_shortcuts` | Search drivers and budget slicing |
| `critical.py`, `critical_bundles.py` | `_route_pair` copper assembly, `_pair_pin_stubs`, `_paired_via_transitions`, `CriticalNetResult` reporting; `plan_bundles` as ordering and layer hints | `route_critical_nets` sequential freeze, `_bundle_repair`, `_validate_candidate` (two full DRC runs per candidate) |
| `critical_tuning.py`, `critical_group_tuning.py` | `UnitTuner`, `GroupTuner`, `serpentine_path`, `bump_path`, as post-convergence tuning | Coupling to frozen critical stage |
| `package_patterns.py` | `circular_array`, `_direction`, `FAMILIES`, near/centre offset math, `PackagePattern`/`PatternContact`. Generate them in the **footprint-local frame** | Board-scene evaluation (via `fanout._legal_choices`), frontier sweep, budgets |
| `fanout.py` | `_candidates` rays, `_two_leg_candidates`, `_OutlineSites`, `_surface_layer`, as a deterministic QFN/perimeter stub generator | Coarse→maze→surface cascade, `improve_escape_assignment`, bypass coupling |
| `escape_assignment.py` | `EscapeConflicts`, to self-validate generated macros | `_finite_assignment` CSP |
| `boundary_access.py` | `package_collar`, as a congestion region | `analyze_boundary_access`, `reserve_boundary_access` gate |
| `plane.py` | `stitch_zone_pads`/`_stitch_land_choices` core: pad → short stub → via inside the plane region, reusing nearby same-net vias. This becomes the plane-tap seed generator | `CopperUse` ownership records (from `copper_ownership`), hard-coded `"GND"` default, `propose_plane_contacts` pool |
| `decoupling.py`, `decoupling_paths.py` | `improve_decoupling_placement`, `_surface_candidates`; `verify_crosslayer_decoupling` as a post-route check | Admission ledger in `route_decouplers`, `propose_crosslayer_*` search |
| `power_geometry.py` | `power_obligations`, `_layer_zones` (routed width → solid pour polygon), `_AreaClearance`, `_zone_geometry` | `plan_power_areas`, tree growth, `apply/reselect_power_area_plan`, reservation refresh (about 1,000 lines) |
| `power_config.py` | Electrical-model JSON and `protected_corridors` parsing (`ExternalPowerBoundary` lives in `power_geometry`; move it out) | Area-search knobs |
| `filled_feedback.py` | `_open_nets`, `_finding_proposals`: native opens → new router connections | Own bridge search, strict-monotone accept, `package_access_blocked` exit |
| `package_support.py`, `support_patterns.py` | `package_contact_intents`, `support_pattern_candidates` (bypass-bank placement) | `plan_package_support`, embedded fanout and power-proof loops |
| `flow.py` | Stage wiring, `_detailed_score` | Gate cascade (rewritten in P2) |

### Retire (about 7,400 lines, plus their tests)

`package_access.py`, `package_placement_repair.py`, `package_preparation.py`,
`escape_feedback.py`, `released_exit_retry.py`, `power_preparation.py`,
`power_areas.py`, `power_contact_feedback.py`, `power_contacts.py`,
`power_contact_normalization.py`, `power_transition_repair.py`, `power_feed.py`,
`plane_bypass_transfer.py`, `bypass_reservations.py`, `priority_bypass.py`,
`surface_bypass_coexistence.py`, `fanout_bypass_completion.py`,
`joint_preparation.py`, `joint_access.py`, `contact_owner_regeneration.py`,
`critical_feedback.py`, `critical_preflight.py`, `critical_reuse.py`,
`local_critical.py`, `pair_search_resume.py`, `pair_tuning_spans.py`,
`shove.py`, `any_angle.py`.

`copper_ownership.py`, `copper_branches.py` and `copper_reservations.py` serve
power preparation only. Keep them until P7, then retire whatever is unused.
`_verified_bypass_links` from `package_access` moves into a post-route check
before that file is deleted.

## 5. Phases

Each phase ends with a benchmark-ladder run (P1) and a commit. A phase is
time-boxed. If it overruns by 50 %, stop and write a one-page status, not a new
evidence bundle.

### P0 · Stabilise (½ day)

- CopperScript: commit the current working tree on a branch,
  `snapshot/pre-refactor-20261019`.
  - Commit the line-ending normalisation from `.gitattributes` separately, so
    real diffs stay readable.
  - The work branches from `main` plus that snapshot.
- CopperProbe:
  - `git init` and commit. There are no commits today.
  - Choose one source as `board.copper`. The candidate is
    `design-candidates/hybrid-reference-inward-escape/board.copper`, which has
    the ferrite-filtered FX10 domains and the GCT connector.
  - Move the other candidates and `verification/` to an `archive/` folder, or
    leave them untracked.
- Add a short "refactor in progress" section to both `AGENTS.md` files pointing
  here. Add the working rules in section 7.

### P1 · Benchmark ladder (2–3 days)

- Add `copper bench <ladder.json>`, or `scripts/bench.py`.
  - It runs `route-board` on each ladder board under a fixed time cap.
  - It reads results with `routing_benchmark.summarize_run`.
  - It writes one JSON plus a Markdown table.
  - Fields: connections total and routed, opens, exact DRC count by code,
    native DRC (when KiCad is present), vias, track length, wall time per stage,
    peak memory.
- Ladder: `valid`, `round-led-ring`, `nrf52`, `full-vertical`, `cm4`,
  CopperLedRing, CopperAssetTracker (pinned commits), CopperProbe. The consumer
  boards run with their own Makefile route options as the baseline.
- Record baselines from the current pipeline under `bench/baselines/`, and use
  `compare_runs` for regressions.
- Optional oracle: `backends/specctra.py`, which exports DSN and imports SES, so
  Freerouting can route any ladder board.
  - This separates "the placement is unroutable" from "our router is weak".
  - It never becomes a dependency (decision D2).
- **Accept when:** one command reproduces the ladder table, and the small boards
  finish in under 10 minutes in total.

### P2 · Never-block pipeline (2–3 days)

- Add `run_routing_pipeline_v2`, initially wired to the *existing* routers:
  `route_global` → `route_critical_nets` → `route_detailed`.
- Remove the early returns.
  - Package access becomes optional and off by default.
  - Power preparation is off on the v2 path.
  - Stage findings go into the report.
- Keep the old pipeline behind `--legacy-pipeline` until P7.
- Add a `--time-budget` that every stage respects. On expiry, the stage returns
  its incumbent.
- **Accept when:** CopperProbe produces a complete best-effort board, opens
  counted, in ≤ 60 min. This gives the first honest whole-board number. All
  other ladder boards are no worse than baseline.

### P3 · Planes-first power (≈1 week)

- **Language.**
  - Add explicit layer roles in `stackup`: `role = signal | plane(NET) | mixed`.
    Inference by `dedicated_plane_layers` stays as a fallback. This closes the
    existing TODO item on dedicated-plane semantics.
  - Split planes are authored as `copper_zone` regions with `priority`.
    `CopperZone` already supports priority and `reserve_routing`.
- **Plane taps.** A generic generator, lifted from `plane.stitch_zone_pads`,
  seeds one tap per plane-net pad: stub plus via, reusing nearby same-net vias.
  - Taps are **soft**: the unified router (P5) may move or regenerate them.
  - Until P5, `route_detailed` treats them as ordinary fixed seed copper.
- **Decoupling.** `improve_decoupling_placement`, then a capacitor-pin surface
  stub (`_surface_candidates`), then a capacitor-side plane tap. Check
  `assess_decoupling_fill` after refill.
- **Rails not on a plane** are ordinary nets with width rules. After routing,
  `_layer_zones` can turn them into pours in free space (opt-in).
- **Automatic partitioner (D3).** For each `mixed`/`plane(*)` layer, assign
  rails to regions. Seed one region per rail from its pins' convex hull,
  inflate it, then resolve overlaps by priority and pin ownership (a
  Voronoi-style split clipped to the board, keepouts and reserved corridors).
  The result is ordinary `CopperZone`s with priorities, checked by the same
  refill. Rails that get no usable region fall back to wide traces. Lift
  `power_obligations` and `_AreaClearance` from `power_geometry`.
  - When the partitioner can't separate rails (a dense BGA field), the fallback
    is an authored region set. That can be source zones or a power-island hard
    macro owned by the part.
- **CopperProbe.**
  - L2 stays solid GND, with the analog opening.
  - L5 becomes split power islands for USB18, VDDD, FX_V33 and the rest of the
    FX10 rails. They are generated by the partitioner where possible, and
    checked against the Infineon KIT_FX10_FMC_001 PWR1 layer.
  - Other rails become wide traces.
- **Accept when:**
  - CopperProbe's native refill has 0 islands.
  - Every plane-net pad and every `DecouplingLink` is connected.
  - The power stage takes ≤ 2 min.
  - Power preparation is no longer on the default path.

### P4 · Library fanout macros (≈1 week, can run in parallel with P3)

- Add `copper macro fanout <part> --fab-profile ...`.
  - It runs the `package_patterns` kernel in the footprint-local frame against
    footprint-only obstacles, plus the `fanout.py` stub generator for
    perimeter/QFN packages.
  - It self-checks with `EscapeConflicts` and emits a v0.5 macro asset: tracks,
    vias, one port per ball on its via, with the protected region on F.Cu only.
- **Hard-macro extensions.**
  - Optional roles and auto-binding by pin name, so unused balls and balls that
    share a net bind.
  - Via size and drill parameterised per fab class.
  - Board readiness decided by DRC/native results rather than "contains a
    macro". Today `production_publishable` must be false and
    `fabrication_ready` is forced to false.
  - Mirroring (bottom-side packages) later.
- **FX10.** Generate the macro and compare it with the Infineon reference
  layout. Where the reference uses a structure that the generator can't make
  (via size, channel use), record that once in the macro's README. Publish it
  in CopperProbeParts as a part-level asset.
- **Accept when:** the FX10 macro materialises in CopperProbe with 0 exact
  DRC; every signal ball is exposed as a port; generation takes seconds.

### P5 · Unified router (2–3 weeks, in slices)

| Slice | Content | Accept when |
| --- | --- | --- |
| 5a | Grid, occupancy, real negotiation, tree mode; `RoutingClearanceIndex.remove`; exact commit | Small ladder boards ≥ `route_detailed` completion, faster |
| 5b | Pair mode (spine A* with lane costs, paired transitions, return vias) | CopperProbe USB and ADC clock pairs route inside the same loop; skew/gap rules pass exact DRC |
| 5c | Bus length term, then post-convergence `UnitTuner`/`GroupTuner`; power-trace widths; plane-tap mode | ADC bus length group within `max_skew`; taps regenerated by the router |
| 5d | Post-processing: cleanup, chamfer, smoothing, land closure; native refill; opens → re-entry (≤ 2 rounds) | CopperProbe meets the section 1 targets. AssetTracker and examples are no worse than baseline |

`run_routing_pipeline_v2` then calls `unified_router.route_board` in place of
`route_critical_nets` and `route_detailed`.

### P6 · Performance (alongside P5)

- Profile with the ladder.
- Expected wins:
  - a numpy obstacle raster built once per board, replacing per-net
    `_blocked_nodes`;
  - integer node ids with array-backed `best`/`prev`/`usage`, instead of
    dataclass nodes and f-string resource keys;
  - binned holes;
  - a bbox sweep instead of O(n²) pairs in `drc._check_copper_spacing`.
- Optional: a small compiled A* kernel with a pure-Python fallback (decision D1).

### P7 · Retire and simplify (≈1 week)

- Delete the retire list and its tests, and `--legacy-pipeline`.
- Cut `route-board` options to a short profile set: `quick`, `balanced`,
  `thorough`, plus time budget, layers and fab profile.
- Collapse `docs/power-area-planning-plan.md`, `routing-backlog.md` and the
  routing TODO items into a single `docs/routing.md` that describes the
  shipped design.
- **Accept when:** the ladder is unchanged and the test suite is green.

Rough total: 6–8 weeks of focused work. P2 alone should produce the first
whole-board CopperProbe result within the first week.

## 5a. Progress (branch `refactor/routing`)

| Date | Phase | Ladder (completion, unified router) |
| --- | --- | --- |
| 2026-10-19 | P0–P2 done; P5 prototype | legacy: round-led-ring 0.57, nrf52 1.0, full-vertical 0.88, cm4 0.76 |
| 2026-10-20 | P5/P6 working, P4 dogbones, P3 islands (opt-in) | round-led-ring 0.98, nrf52 1.0, full-vertical 0.99–1.0, cm4 1.0, led-ring 1.0, asset-tracker 0.95, CopperProbe ≈0.82; all DRC-clean |

Open items:

- **CopperProbe** stops at about 15 conflicted plus a few unroutable nets.
  Diagnosis on 2026-10-20:
  - It is not layer capacity. Turning In4 into a signal layer gives the same
    result.
  - The conflicts are a few hundred grid cells, mostly F.Cu within 0.5 mm of
    FX10 balls and next to the crystal caps (C_XIN, C_XOUT) and the
    C_FX_L3/C_FX_A3 corner. This is terminal access in a dense neighbourhood.
  - Some nets fail even alone, because of board rules: VBUS has 0.4 mm width
    with no breakout rule on a 0.25 mm QFN pin, and FB is F.Cu-only with no
    vias.
  - Next levers are placement feedback driven by these hotspots (the
    `routeflow` outer loop) and a part-owned FX10 fanout macro (P4).
- **Freerouting comparison (D2)** is blocked here: the sandbox can't download
  the jar.
- **Windows DLL** is built but not yet run on Windows.

## 6. Decisions (owner, 2026-10-19)

- **D1 Dependencies.** numpy is allowed as a runtime dependency. A compiled A*
  core is allowed later if profiling justifies it; keep a pure-Python/numpy
  fallback.
- **D2 Freerouting.** Use it as an optional comparison in the benchmark ladder
  only where it finishes in reasonable time. It is never required.
- **D3 Power.** Automatic power design is the goal. P3 therefore includes an
  automatic plane/split-plane partitioner: it assigns rails to plane layers and
  regions from pin locations, decoupling links and the declared layer roles.
  Manual authoring is the fallback when the partitioner gets stuck. The manual
  form may be source zones or a hard macro (for example an FX10 power-island
  macro that ships with the part).
- **D4 Retired code.** Delete it. Git history keeps it, and
  `snapshot/pre-refactor-20261019` holds the last full working tree. The
  consumer boards (CopperAssetTracker, CopperLedRing) and the CopperScript
  examples must keep working.

## 7. Working rules during the refactor (for agents)

- **The metric is the ladder table.** A change is good if completion goes up or
  time goes down without other regressions.
  - Per-change evidence is: the ladder JSON, the diff and the test result.
  - No new `verification/*` bundles unless the owner asks.
- **No board-shaped heuristics.** A fix that only helps one board's specific
  geometry goes in that board's source (constraints, zones, macros), not in the
  engine.
- **No new module** outside this plan without updating section 4 first.
- **README status** is a short table (date, phase, ladder numbers). Not a
  narrative log.
- **Commit small and often** on a feature branch. CopperProbe is version
  controlled from P0 onward.
- **Escalate when blocked.** If a phase misses its acceptance after its time box,
  stop and summarise for the owner instead of adding recovery layers.
