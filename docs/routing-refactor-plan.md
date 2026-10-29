# Routing refactor plan

Status: accepted 2026-10-19 (decisions in section 6). Owner: andenore. This plan supersedes the
routing parts of `TODO.md` and `docs/routing-backlog.md` while it is active
(the former `docs/power-area-planning-plan.md` was removed with the legacy
router on 2026-10-26). Where they conflict,
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
| `cm4`, `full-vertical`, `nrf52` examples | 100 % completion, exact DRC clean, each ≤ 5 min |

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
| `placement*.py`, `clusters.py`, `incremental_placement.py`, `routeflow.py` (`optimize_placement_for_routing`, `detailed_failure_trials`) | Placement, plus the optional outer loop driven by opens. `detailed_failure_trials` serves the legacy path only once `congestion_feedback.py` drives the unified path |
| `critical_review.py`, `pair_geometry.py`, `route_path_geometry.py`, `return_paths.py` | Read-only reports |

### New (added by the refactor)

| Module | Role |
| --- | --- |
| `unified_router.py` | Negotiated-congestion (PathFinder) router on a numpy legality raster; exact commit through `routing_clearance` (P5) |
| `native/astar.c` + prebuilt `.so`/`.dll`, `tools/build_kernel.py` | Optional C A* kernel with identical results; pure Python fallback (`COPPER_NO_NATIVE=1`) (P6) |
| `bga_fanout.py` | Deterministic quadrant dogbones for full-grid ball arrays (P4) |
| `power_islands.py` | Automatic split-plane islands for decoupled rails on a spare plane layer (P3, opt-in `--power-islands`) |
| `route_summary.py`, `plane_estimate.py`, `bench.py`, `bench/ladder.json` | Pipeline-independent completion summary, raster plane estimate, benchmark ladder (P1) |
| `congestion_feedback.py` | Routability-driven placement feedback: per-tile congestion map from the unified router's contention (history, overflow, rejection points) combined with a RUDY-style demand estimate; parts on failed nets and hot tiles move down the demand gradient, candidates are screened by the estimate plus `route_global`, and only the best rerun the pipeline tail. Replaces `critical_feedback.py` and `routeflow.detailed_failure_trials` on the unified path (2026-10-24) |
| `routing_lattice.py`, `fine_regions.py` | Multi-resolution search lattice (option 2 of the 2026-10-25 router review): `routing_lattice.py` puts every node-id, offset and point/node computation of the unified router behind one `Lattice` object (`UniformLattice` today, `NestedLattice` with fine patches at base pitch ÷ k next); `fine_regions.py` plans the patches from footprints whose pad or ball pitch needs them, with the `choose_pitch` alignment rule applied per package instead of to the whole board. Base pitch stays 0.25 mm; its auto-calculation is a later change |
| `global_keepouts.py` | Polygon-aware coarse keepout geometry for the global router (`routing.route_global`): keepouts are tested as their real polygons rather than bounding boxes when capacity is assigned. Coarse only; exact copper gates are unchanged (added on main, 2026-10-25) |
| `global_reserved_access.py` | Feeds freshly verified, retained package ports and immutable prefix copper into the global router's capacity graph (`admit_fixed_prefix`, `reserved_pin_access_candidates`), so global guides start from the same exits the detailed router must use. Admission refuses any change to source contracts, immutable copper or required arrays (added on main, 2026-10-25) |
| `native_stub_cleanup.py` | Proposes trimming track dead ends that the native KiCad refill identifies exactly, for `plane_verify`: removes only copper outside declared same-net zone boundaries, never immutable copper or critical nets, and only a fresh native refill can accept the proposal. Part of the single native verification step, not a recovery loop (added on main, 2026-10-25) |

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
- Ladder: `valid`, CopperLedRing (pinned consumer commit), `nrf52`,
  `full-vertical`, `cm4`, AssetTracker (pinned commit), CopperProbe.
  Resolve CopperLedRing as an explicit consumer checkout with its locked
  dependencies; do not substitute the deprecated `round-led-ring` fixture or
  assume a sibling directory. Retain its successful pinned build as the
  comparison baseline and record the compiler revision under test separately.
  The consumer boards use their own Makefile route options as the baseline.
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

Status 2026-10-26: the legacy router (`--router legacy`, `route_detailed`) and
every stage reachable only through it are deleted with their tests and docs:
the package-access, power-preparation/area-planning, zone-escape,
critical-placement-feedback, joint-preparation, incremental-placement and
contact-regeneration modules, `shove.py`, `any_angle.py`, `copper_branches.py`
and most of `copper_ownership.py`. Modules on the retire list that the unified
pipeline still calls (`bypass_reservations.py`,
`fanout_bypass_completion.py`, `critical_preflight.py`, `critical_reuse.py`,
`local_critical.py`, `pair_search_resume.py`, `pair_tuning_spans.py`) remain,
with unreachable definitions removed; `verified_bypass_links` moved to
`bypass_links.py`. The removed `route-board` flags were accepted and ignored
until the consumers dropped them (2026-10-27); they are now refused. The single-ended critical
fallback for RF-feed, power and general critical kinds, which still called
`route_detailed` on the default path, now uses the unified router (see the
2026-10-26 progress row). The option-profile cut and the docs collapse below
are still open.

2026-10-29 option cleanup (Python and Rust): remove nine dedicated unified
solver switches in favour of the existing `--unified-set OPTION=VALUE`, the two
old placement-feedback budget aliases in favour of `--congestion-feedback-trials`,
and the redundant zone-stitch request. Rename the global demand estimator's
pitch to `--local-demand-pitch-mm`; physical routing retains
`--unified-pitch-mm`. Effective defaults stay unchanged. Removed spellings fail
with migration instructions; current consumer wrappers migrate with the CLI.
This reduces the option count by twelve; folding the remaining live controls
into profiles is still open. See the [migration table](cli-workflow.md).

Example build entry points now select the installed Cargo Rust executable. The
Python orchestration runner accepts an explicit compiler path, records its hash,
and preserves unsupported/nonzero exits and native completion gates. Python
remains for helper APIs, provenance and tests; it is not a compiler fallback.
The installed binary is byte-identical to the binary used for the ladder above;
a native Rust-backed build-runner fixture also passes refill and DRC.

- Delete the retire list and its tests, and `--legacy-pipeline`.
- Cut `route-board` options to a short profile set: `quick`, `balanced`,
  `thorough`, plus time budget, layers and fab profile.
- Collapse `routing-backlog.md` and the
  routing TODO items into a single `docs/routing.md` that describes the
  shipped design.
- **Accept when:** the ladder is unchanged and the test suite is green.

Rough total: 6–8 weeks of focused work. P2 alone should produce the first
whole-board CopperProbe result within the first week.

### After the refactor · Escape planning

Joint planning of escape side, layer and order per package and package pair,
before global routing: [escape-planning-plan.md](escape-planning-plan.md)
(steps E0–E6). It builds on P4's fanout generator and feeds P5's unified
router through reservations; it is a planning stage, not a recovery gate.

## 5a. Progress (branch `refactor/routing`)

Handoff, open items and the plan for merging to `main`:
[routing-refactor-handoff.md](routing-refactor-handoff.md) (2026-10-21).

| Date | Phase | Ladder (completion, unified router) |
| --- | --- | --- |
| 2026-10-19 | P0–P2 done; P5 prototype | legacy: round-led-ring 0.57, nrf52 1.0, full-vertical 0.88, cm4 0.76 |
| 2026-10-20 | P5/P6 working, P4 dogbones, P3 islands (opt-in) | round-led-ring 0.98, nrf52 1.0, full-vertical 0.99–1.0, cm4 1.0, led-ring 1.0, asset-tracker 0.95, CopperProbe ≈0.82; all DRC-clean |
| 2026-10-20 (pm) | Escape and bypass reservations before critical routing; exact pin escapes; full-footprint negotiation | asset-tracker 1.0 (120/120), led-ring 1.0, nrf52 1.0, cm4 1.0, full-vertical 1.0, round-led-ring 0.98; all DRC-clean |
| 2026-10-20 (eve) | Automatic diff-pair corridors, satellite seeding, placement and plane-estimate diagnostics in `copper issues` | asset-tracker 1.0, led-ring 1.0, nrf52 1.0, cm4 1.0, full-vertical 0.99, round-led-ring 0.98; all DRC-clean. CopperProbe (7 parts unfixed) 0.844: USB_DM/DP now route |
| 2026-10-20 (late) | Pour re-check in the unified router (walled-off plane pads get explicit copper) | asset-tracker 1.0 and 0 estimated plane opens (was 1: U_CC.3), others unchanged; all DRC-clean |
| 2026-10-20 (night) | Neck exits stay inside the breakout region; refinement swaps ignore escape differences under 0.1 mm | full-vertical 1.0 (was 0.988), asset-tracker 1.0, led-ring 1.0, nrf52 1.0, cm4 1.0, round-led-ring 0.98; all DRC-clean, 0 estimated plane opens |
| 2026-10-21 | CopperProbe repo: candidate is `board.copper` (21 parts released), others archived; first Windows run, native refill | CopperProbe 0.878 (29/237 opens), 2 exact DRC (max length: ADC_D8, SENSE), 34 native unconnected, 3 est. plane opens, 67 min |
| 2026-10-21 (pm, Windows) | Merged origin/main (3.2, 3.4); D3-A 89→91 clean, 0 lost; ladder unchanged. BGA escape strips (`--unified-escape-strips`, off) after comparing the FX10 kit layout | CopperProbe baseline 0.869 (31/237), DRC 1; with strips 0.886 (27/237), DRC 1, but USB_TX1*_IC lost (moved bypass copper takes its via site), so strips stay off |
| 2026-10-21 (eve, Windows) | CopperProbe `c1d3639` releases x/y of C_FX10_G3, C_FX_E3, C_FX_L3 (placer puts them 0.8-1.5 mm from their balls, kit-like) | released 0.848 (36 opens), released+strips 0.873 (30 opens, TX1_IC lost); both worse than unreleased (0.869 / strips 0.886); bypass pending 6/10/9/12 |
| 2026-10-21 (late) | Owner decision: escape strips dropped from the branch (reverted; they cost the TX1_IC pair) and the cap release reverted in CopperProbe (`09d8c0d`) | CopperProbe back to 0.869 (31/237) |
| 2026-10-22 | Bends: unified has no turn cost in its grid search (legacy charges bend_cost 12 per 45 degrees) and never runs route_smoothing, so equal-length staircases of 45-degree jogs survive. `UnifiedRouterOptions.route_smoothing` (off) straightens its own new copper with the legacy pass | PCBWorld D3-A first 20 boards, local KiCad 10, 19 clean in all runs: bends legacy 411, unified 739, unified+smoothing 505; 90-degree-or-sharper bends 122 / 24 / 3; length 1715 / 1574 / 1566 mm; completion, DRC and vias unchanged by smoothing |
| 2026-10-22 | Unified lost boards: per-land terminals (plan 3.4); grid obstacles keep their own net's rule clearance; claims assume a neighbour at least as wide as the net | PCBWorld D3-A locally (KiCad 10): unified 80 -> 95 clean, none lost (legacy 93); completion 0.953 -> 0.988; vias, length and bends unchanged. Open: 0049 (header gaps on the exact midline, escape plan E1) |
| 2026-10-22 | `UnifiedRouterOptions.route_smoothing` on by default; `route-board --no-route-smoothing` turns it off for either router | D3-A 99 boards locally: bends 7025 -> 4382, 90-degree-or-sharper 550 -> 117, length -0.8%; clean (95), completion, DRC, vias unchanged; no board worse. Ladder (unified): completion, opens, DRC, vias identical, length shorter on all six |
| 2026-10-24 | Review fixes (`review/unified-router-plan`): failed-net retry schedule and tunables as options; `RoutingClearanceIndex.remove(owner)` with one shared exact index across commit, repair and pour re-check; `physical_via_span` vias; `bend_cost` and `guide_cost` search terms in Python and the C kernel (both default 0: bend 0.25 cut bends 27–40 % on cm4, nrf52, full-vertical but cost round-led-ring 3–14 vias; guide cost was worse on every board); `route-board` reports legacy flags that `--router unified` ignores | Ladder (unified, no KiCad) identical to `6f7f652`: round-led-ring 51/51, 40 vias, 582.844 mm; nrf52 35/35, 44, 350.563; full-vertical 86/86, 193, 1602.883; cm4 21/21, 61, 587.68; 0 DRC on all. Legacy on the same tree: round-led-ring 30/51, cm4 21/21 |
| 2026-10-24 | Default router is unified: `route-board`, `pcbir.bench` and the D3 CI pin default to `--router unified`; `--router legacy` keeps the pre-refactor pipeline | Ladder (default, no `--router`, no KiCad): round-led-ring 51/51, 40 vias, 582.844 mm; nrf52 35/35, 44, 350.563; full-vertical 86/86, 193, 1602.883; cm4 21/21, 61, 587.68; 0 DRC on all. `--router legacy`: cm4 21/21, 84 vias, 646.078 mm, 0 DRC |
| 2026-10-24 | Owner decisions: `bend_cost` default 0.25 (adopted); legacy-only stages stay behind `--router legacy` for now; direct-mode pair verification deferred | Ladder (unified, bend 0.25, no KiCad): round-led-ring 51/51, 48 vias, 604.833 mm; nrf52 35/35, 42, 345.115; full-vertical 86/86, 189, 1597.398; cm4 21/21, 62, 588.979; 0 DRC on all |
| 2026-10-24 | Congestion-driven placement feedback (B): one loop on the unified path (`--congestion-feedback-trials N`; `--detailed/--critical-feedback-trials` map onto it, `improve_critical_placement` no longer runs there); router map plus RUDY estimate, each normalised to its 95th percentile; screen to 2, rerun the shared pipeline tail, accept on the whole-board tuple | Ladder (default, no KiCad), with and without `--congestion-feedback-trials 4` (loop idle, no failures): round-led-ring 51/51, 48 vias, 604.833 mm; nrf52 35/35, 42, 345.115; full-vertical 86/86, 189, 1597.398; cm4 21/21, 62, 588.979; 0 DRC; boards byte-identical to `3770a2a`; total 352 s / 333 s (base 372 s). Fixture: one boxed-in net connected by one accepted move |
| 2026-10-25 | Multi-resolution lattice S4: `--fine-regions arrays` default (fine patches over full ball arrays only); `dense` (also dense non-array packages) for measurement; `off` uniform; `choose_pitch` judges every placed ball | Ladder (default, no KiCad): round-led-ring 51/51, 48 vias, 604.833 mm; nrf52 35/35, 42, 345.115; full-vertical 86/86, 189, 1597.398; cm4 21/21, 62, 588.979; 0 DRC; no patches, boards byte-identical to `--fine-regions off` and to `53c5f6c`. `dense`: round-led-ring 51/51, 50 vias, 616.451 mm, 73.7 s (U1 k=5); nrf52 35/35, 43, 333.132, 21.6 s (U_NRF k=5); full-vertical 86/86, 187, 1596.424, 186.7 s (5 patches); cm4 21/21, 61, 583.967, 14.8 s (2 connectors k=2); 0 DRC. BGA fixture (100 x 80 mm, 6 layers, 5 x 5 at 0.75 mm): `arrays` 0.79 M nodes, 148 MB, 4.1 s; `off` (0.125 mm uniform) 3.09 M, 348 MB, 4.9 s; 25/25, 0 DRC both |
| 2026-10-26 | P7 retire: legacy router (`--router legacy`, `route_detailed`) and every stage only it reached deleted, with tests and docs; removed `route-board` flags accepted and ignored. The single-ended critical fallback for kinds other than clock/critical (RF feed, power, general) now uses the unified router; on `main` it still called `route_detailed` | Ladder (default, no KiCad), board SHA-256 against `main` `35dc73b`: round-led-ring, nrf52, full-vertical, cm4 and CopperLedRing byte-identical (examples with the removed flags dropped from their Make arguments, CopperLedRing with its pinned consumer flags). CopperAssetTracker differs only through CELL_RF (RF feed): `unified_single_net`, 5 tracks, 17.72 mm on F.Cu instead of `local_surface_tree`, 7 tracks, 16.91 mm; ordinary routing 60/62 both (GND, V3V3 open), vias 51 -> 54, length 1389.2 -> 1391.9 mm, same DRC findings; 1596 s against 3885 s. Owner accepted the CELL_RF change as is |
| 2026-10-29 | P7 input cleanup, mirrored in Python and Rust: 12 fewer top-level options; expert solver controls share `--unified-set`, placement-feedback aliases retire, demand pitch gets an explicit name, and declared zones stitch automatically. Current example/consumer callers migrate. Effective direct-mode limits govern native processing as well as reports. | Rust before/after against `dfc2745`: PCB bytes and route summaries identical on round-led-ring, nrf52, full-vertical, cm4, CopperLedRing and CopperAssetTracker (their existing exit 1 results remain). CopperProbe exits 3 in both with the identical unsupported nested-lattice diagnostic. Source/manifest/lock inputs match; timings include dependency-cache warming. Both CLIs pass a direct-mode native KiCad fixture with zero repair trials. 210 focused Python tests pass, 1 skips; 11 Rust CLI tests, formatting and strict CLI clippy checks pass. The existing unknown-part error discrepancy remains visible. |

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
- **(Resolved 2026-10-21: the candidate is now CopperProbe's `board.copper`.)**
  **The benchmarked CopperProbe is a design candidate, not the base board.**
  `copper-probe` is `design-candidates/hybrid-reference-inward-escape` (166
  fixed placements, made in earlier agent sessions); the owner's top-level
  board.copper has 109 and fixes none of the parts below. `copper issues`
  flagged 16 of the candidate's fixed parts (REMOTE_FIXED_PART), e.g. R_CC_DET
  65.7 mm and C_CLKP 40 mm from their nearest partner. On 2026-10-21 the owner
  approved releasing 21 (side kept) in that candidate; a copy with the same
  change gave 0.857 (34 opens) with every critical net routed.
- **FX10 south edge vs the Infineon kit (2026-10-21).** Aligned on all 169 balls, the kit carries its
  Port0 bus out of that edge on TOP and SIG1 with 2 vias in the strip below the outer rows; ours had 11
  (outward GND dogbones of grounded P0D13-15 and south-bank bypass vias). The kit's supply vias are
  in-field diagonal dogbones and its VDDIO_P0 caps sit under the outer rows about 1.1 mm from their balls.
  Escape strips (via keepouts outside a signal edge while supply vias are placed) emptied the strip and
  routed 4 more ADC lines, but the re-planned bypass copper took the TX1_IC pair's via site; releasing
  the south-bank caps made routing worse. Both were dropped on 2026-10-21; the comparison stands as the
  reference for a future FX10 fanout (P4).
- **CopperProbe ADC bus (11 nets) at the FX10 south edge.** max_vias=2 and a
  length limit; F.Cu just south of the package is full of other copper, so
  the bus drops to inner layers and runs out of vias or length.
- **Satellite parts under a BGA.** The FX10 kit puts the RESREF resistor on
  B.Cu under the package, 1.2 mm from its ball. The placer never changes a
  part's side, so R_RESREF lands about 7 mm away on F.Cu. (Crystal caps were
  swapped 12 mm away for 10 um of escape width; fixed, now about 3.3 mm.)
- **Freerouting comparison (D2)** is blocked here: the sandbox can't download
  the jar.
- **Windows DLL** verified 2026-10-20: it loads, matches the Python search, and the
  small/medium ladder reproduces the Linux numbers (round-led-ring 0.98, nrf52 1.0,
  full-vertical 0.99, cm4 1.0; 0 DRC).

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
