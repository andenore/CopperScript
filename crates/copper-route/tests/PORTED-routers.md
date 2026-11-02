# Ported general-router tests (session `rust-routing-tests-a1`)

Python behaviour tests of the general routers (read with
`git show d39067a:tests/test_NAME.py`) ported to Rust. Shared board builders:
`tests/support_routers/mod.rs` (integration tests; also included by
`src/test_support_routers.rs` for crate-private unit tests and by
`crates/copper/tests/replay_unified_cli.rs`).

Counts are Python test functions. "Ported" includes tests whose assertion was
adapted (noted). "Skipped" tests have no Rust counterpart to test.

| Python file | Tests | Ported | Skipped | Ignored | Rust tests |
|---|---|---|---|---|---|
| unified_router | 48 | 46 | 2 | 0 | `tests/unified_router.rs` (36), `src/unified_router/tests.rs` (9), `crates/copper/tests/replay_unified_cli.rs` (1) |
| escape_plan | 43 | 42 | 1 | 0 | `tests/escape_plan.rs` (41), `src/escape_plan.rs` `tests` (1) |
| escape_placement | 40 | 33 | 7 | 0 | `crates/copper-place/tests/escape_placement.rs` (32), `crates/copper-place/src/legalize.rs` `tests` (1) |
| routing | 19 | 18 | 1 | 0 | `tests/routing.rs` (17), `src/routing.rs` `tests` (1) |
| routing_clearance_pad_cache | 18 | 14 | 4 | 0 | `tests/routing_clearance_pad_cache.rs` (14) |
| congestion_feedback | 17 | 15 | 2 | 0 | `tests/congestion_feedback.rs` (15) |
| escape_assignment | 14 | 8 | 6 | 0 | `tests/escape_assignment.rs` (4), `src/escape_assignment.rs` `tests` (4) |
| unified_nested_routing | 13 | 13 | 0 | 0 | `tests/unified_nested_routing.rs` (12), `src/unified_router/tests.rs` (1) |
| route_path_geometry | 13 | 0 | 13 | 0 | none |
| corridor_reservation | 13 | 13 | 0 | 0 | `crates/copper-place/tests/corridor_reservation.rs` (12), `crates/copper/tests/corridor_cli.rs` (1) |
| unified_commit_index | 12 | 10 | 2 | 0 | `tests/unified_commit_index.rs` (10) |
| placement_global_progress | 11 | 3 | 8 | 0 | `tests/placement_global_progress.rs` (3) |
| routing_lattice | 9 | 9 | 0 | 0 | `tests/routing_lattice.rs` (9) |
| congestion_feedback_flow | 9 | 9 | 0 | 0 | `tests/congestion_feedback_flow.rs` (10) |
| unified_router_options | 8 | 4 | 4 | 0 | `tests/unified_router_options.rs` (3), `src/unified_router/tests.rs` (1) |
| unified_congestion_export | 8 | 8 | 0 | 0 | `tests/unified_congestion_export.rs` (8) |
| unified_search_costs | 7 | 6 | 1 | 0 | `tests/unified_search_costs.rs` (5), `src/unified_router/tests.rs` (1) |
| unified_review_constraints | 7 | 6 | 1 | 0 | `tests/unified_review_constraints.rs` (6) |
| routing_clearance_remove | 7 | 7 | 0 | 0 | `tests/routing_clearance_remove.rs` (4), `src/routing_clearance.rs` `tests` (3) |
| global_target_via_cost | 7 | 7 | 0 | 0 | `src/routing.rs` `tests` (7) |
| global_search_static_cache | 7 | 5 | 2 | 0 | `src/routing.rs` `tests` (5) |
| routing_acceptance | 6 | 6 | 0 | 0 | `tests/routing_acceptance.rs` (6) |
| global_reserved_access | 6 | 6 | 0 | 0 | `tests/global_reserved_access.rs` (6) |
| global_access_launch_diversity | 6 | 6 | 0 | 0 | `src/routing.rs` `tests` (6) |
| unified_span_guides | 5 | 4 | 1 | 0 | `tests/unified_span_guides.rs` (4) |
| routing_hotspot_caches | 5 | 4 | 1 | 0 | `tests/routing_hotspot_caches.rs` (2), copper-pcb `src/hard_macros.rs` `tests` (2) |
| region_blocker_repair | 5 | 4 | 1 | 0 | copper-place `src/legalize.rs` `tests` (4) |
| global_via_refinement | 5 | 5 | 0 | 0 | `src/routing.rs` `tests` (5) |
| global_edge_strip | 5 | 5 | 0 | 0 | `tests/global_edge_strip.rs` (5) |
| released_escape_revalidation | 4 | 0 | 4 | 0 | none |
| global_breakout_demand | 4 | 4 | 0 | 0 | `src/routing.rs` `tests` (4) |
| escape_capacity | 4 | 0 | 4 | 0 | none |
| reference_escape_geometry | 4 | 1 | 3 | 0 | `tests/routing_quality.rs` (1) |
| unified_via_budget_pressure | 3 | 3 | 0 | 0 | `src/unified_router/tests.rs` (3) |
| unified_pair_budget | 3 | 2 | 1 | 0 | `tests/unified_pair_budget.rs` (2) |
| unified_lead_conflicts | 3 | 3 | 0 | 0 | `src/unified_router/tests.rs` (3) |
| global_terminal_fallback_degree | 3 | 3 | 0 | 0 | `src/routing.rs` `tests` (3) |
| global_polygon_keepouts | 3 | 3 | 0 | 0 | `tests/global_polygon_keepouts.rs` (3) |
| global_access_diagnostics | 3 | 1 | 2 | 0 | `tests/global_polygon_keepouts.rs` (1) |
| unified_existing_access | 2 | 2 | 0 | 0 | `tests/unified_existing_access.rs` (2) |
| unified_direct_cli | 2 | 2 | 0 | 0 | `crates/copper/tests/unified_direct_cli.rs` (2) |
| surface_package_escape | 2 | 2 | 0 | 0 | `tests/surface_package_escape.rs` (2) |
| repair_search_reuse | 2 | 1 | 1 | 0 | `tests/repair_search_reuse.rs` (1) |
| multi_land_routing | 2 | 0 | 2 | 0 | none |
| global_terminal_capacity_priority | 2 | 2 | 0 | 0 | `src/routing.rs` `tests` (2) |
| exact_escape | 2 | 0 | 2 | 0 | none |
| escape_reservation_lifetime | 2 | 1 | 1 | 0 | `tests/escape_reservation_lifetime.rs` (1) |
| detailed_routing | 2 | 2 | 0 | 0 | `tests/detailed_routing.rs` (2) |
| routing_review_regressions | 1 | 1 | 0 | 0 | `tests/routing_review_regressions.rs` (1) |
| routing_hotpath_equivalence | 1 | 1 | 0 | 0 | `src/fanout.rs` `tests` (1) |
| routing_quality | 14 | 13 | 1 | 1 | `tests/routing_quality.rs` (11), `src/fanout.rs` `tests` (2), `src/escape_assignment.rs` `tests` (1) |

**Status: complete.** All 51 files are done: 441 Python tests, 363 ported (1 of
them `#[ignore]`d), 78 skipped.

Rust changes made for the ports: `router::index_holds` guard in
`Router::new` (bug fix, see unified_commit_index); `pub(crate)` on
`rejection_text`, `tune_bus_skew`, `congestion_map`, `open_count`, `turn_table`,
`route_net_budgeted`, `penalise`,
`Router::{escape_bias, ball_field_via_mask, zone_regions, plane_via_taps}`;
`#[cfg(test)]` modules in `routing.rs`, `routing_clearance.rs`, `fanout.rs`, copper-pcb `hard_macros.rs`, `escape_plan.rs`,
`escape_assignment.rs`, `unified_router/tests.rs` and copper-place
`legalize.rs`.

## Notes per file

### unified_router
- Skipped: `test_a_neck_only_node_is_left_inside_its_region_native` and
  `test_escape_bias_moves_a_net_off_the_layer_inside_its_area_native` repeat
  their pure-Python twins against the former native A* kernel; Rust has one
  search, ported once.
- Adapted: `test_native_kernel_matches_python_search` became
  `routing_is_deterministic` (equal inputs give equal copper).
- Adapted: `test_expired_repair_budget_does_not_start_another_escape_seed`
  patched `perf_counter`; the Rust test sleeps past a 2 s `time_budget_s` in the
  progress callback as the first unseeded repair ends (takes ~2 s).
- Adapted: `test_grid_window_refuses_invalid_bounds_and_excluded_targets`: the
  arity/bool/float checks cannot be expressed against the typed Rust option;
  ordering and excluded targets are tested.
- Adapted: `test_replay_rejects_unknown_and_excluded_net_selection` saves the
  narrowed selection with `only_nets` instead of editing the state with
  `pcbir.workbench`.
- Adapted: spies on `_search_unbiased` / `_route_net` became direct checks of
  the occupancy history after each `route_net` and of the route order after
  `negotiate`.
- Made `pub(crate)` for the unit tests: `rejection_text`, `tune_bus_skew`,
  `Router::{escape_bias, ball_field_via_mask, zone_regions, plane_via_taps}`.

### escape_plan
- Skipped: `test_replay_takes_the_saved_fanout_out_and_fans_out_again`:
  `replay-unified --refanout`/`--fanout-set` (BGA re-fanout on replay) is not
  ported; Rust reports it not implemented.
- Parametrized cases (`swap_sides`, `rotation`) loop inside one Rust test.
- `test_closed_tile_diagonals_...` calls the private `_array_flow`: unit test.

### escape_placement
- Owned by `copper-place`; its tests include `support_routers/boards.rs` (the
  builders that need no `copper-route`).
- Skipped (they replaced engine internals with monkeypatched fakes and
  counted calls; Rust has no injection seam for them):
  `test_nonimproving_escape_trials_do_not_repeat_complete_geometry`,
  `test_spreading_rejects_coarse_congestion_regression`,
  `test_exhausted_large_anchor_pack_retries_independent_close_pair`,
  `test_impossible_local_placement_bounds_illegal_probes`,
  `test_joint_search_retains_search_budget_when_lookahead_is_exhausted`,
  `test_local_domain_keeps_outer_candidates_within_the_probe_budget`,
  `test_invalid_local_domain_does_not_starve_a_legal_region_grid`.
- Adapted: cache-size and call-count assertions of
  `test_pair_cache_matches_fresh_models...` and
  `test_deficit_single_move_only_revisits_incident_pairs` (now
  `deficit_after_a_single_move_matches_full_geometry`) are dropped; the
  results are still compared with fresh models. The random trials use a
  fixed LCG instead of `random.Random(41)`. `_project_coordinate` (private)
  is not called by `region_sampling_...`; the profile immutability check is
  enforced by the type (`Rc`).

### routing
- Skipped: `test_legal_pad_exit_can_attach_to_region_without_center_stub`
  passed a duck-typed fake clearance index; `pin_access_candidates` takes the
  concrete `RoutingClearanceIndex`.
- `_route_net` is private: unit test. Rule validation that Python ran in
  `replace(board, ...)` is `copper_pcb::physicalize::revalidate`.

### routing_clearance_pad_cache
- Python shared an LRU pad template between indexes; Rust builds it per
  index. Kept: every behavioural assertion (fresh answers after each source
  change, index independence, pad/NC/hole geometry, breakout owners against a
  linear oracle, validation errors). Dropped: cache hits, object identity and
  call counts.
- Skipped: `test_bounded_geometry_work_across_copper_snapshots`,
  `test_cache_bounds_and_strong_original_identity`,
  `test_owner_geometry_work_is_once_per_complete_source_epoch` (cache work
  only), `test_required_array_occurrences_remain_current_original_vias`
  (built on the decoupling cross-layer fixture of another test file).
- Adapted: `test_materialized_macro_prefix_is_indexed_fresh_after_template_hit`
  became `existing_copper_is_indexed_in_source_order_as_locked_additions` on a
  plain board (the macro fixture lived in `test_hard_macros.py`).

### congestion_feedback
- The Rust module keeps only what the pipeline uses. Dropped parts of tests:
  numpy read-only arrays and constructor shape checks, `value_at`,
  `tile_bounds` IndexError, `gradient_at(radius_tiles=3)` (the corner case
  runs at the fixed radius 2), `estimate_demand(nets=)`,
  `propose_moves(rotations=, limit=)` (the fixed limit of 24 is checked) and
  the private `rank` (group and step order are read from `reason`).
- Adapted: `test_combined_requires_same_lattice_and_keeps_own_capacity`
  tests the lattice/capacity/failure-point contract on `combine_normalised`
  (Python's raw `combined` is not ported).
- Skipped: `test_to_json_round_trips_the_lattice_and_maps_compare_by_value`
  (no `to_json`), `test_screen_orders_by_global_score_and_keep_bounds_the_result`
  (monkeypatched `route_global`).

### routing_quality
- Ignored: `default_planner_consumes_decoupling_role_without_distance_rules`:
  copper-place's decoupling-access phase reports NotImplemented for a
  movable pin-associated decoupler (Rust gap, not a test problem).
- Skipped: `test_nested_module_decoupling_endpoint_is_qualified` exercised the
  private elaboration helpers `_expand_one`/`_Template` of the language.
- The prototype board is `compile_design_source` + `physicalize(Proxy)` at
  Python's `PrototypePhysicalOptions` defaults.

### escape_assignment
- Skipped (they replaced `fanout._candidates`, `_two_leg_candidates` or
  `run_physical_drc` with monkeypatched fakes to build a trap; Rust has no
  injection seam): `test_joint_assignment_repairs_equal_slack_trap_without_losing_incumbent`,
  `test_competing_pin_with_legal_radial_choice_gets_off_ray_alternative`,
  `test_budget_exhaustion_preserves_greedy_geometry`,
  `test_locked_subset_copper_is_not_a_replaceable_escape`,
  `test_joint_domain_can_mix_surface_and_via_without_misclassifying_ownership`,
  `test_exact_whole_board_gate_rolls_failed_assignment_back_to_greedy`.
- `EscapeConflicts::compatible` is private: unit tests. Trials are compared by
  their `Debug` text (no `PartialEq`).

### unified_nested_routing
- `test_bga_routes_completely_on_the_nested_lattice` and
  `test_uniform_fine_pitch_reaches_the_same_completion` share one Rust test
  (Python shared a module fixture). The congestion test is split: the routed
  map is checked in the integration test, the empty-board capacities against
  the private `congestion_map` (now `pub(crate)`) in a unit test.
- The memory fixture (100 x 80 mm, six layers) runs in about 0.5 s in
  release, so it is not ignored (Python skipped it without the native kernel).
- The state rewrite of the replay test uses `copper_state::content_digest`.

### route_path_geometry
- Skipped (all 13): `routed_path_inventory` was the routed-path screening of
  `pcbir.engineering_qualification`, which has no Rust implementation.

### corridor_reservation
- Owned by `copper-place`/`copper-drc`. Python's `reserved_corridors(...).skipped`
  is `copper_drc::legality::corridor_reservation`'s second value.

### unified_commit_index
- **Rust bug fixed:** `Router::new` accepted any clearance index. Python's
  `_Router` refused a shared index that does not hold exactly its board's
  copper (`_index_holds`); the Rust router now checks the same
  (`router::index_holds`, `same_but_copper`).
- Skipped: `test_hold_rips_nets_out_and_answers_as_a_fresh_index` and
  `test_shared_index_ends_holding_the_returned_copper` test the private
  `_SharedIndex` (`Shared` in `route.rs`) directly / through a patched
  subclass; the shared-vs-fresh result tests cover its behaviour.
- Adapted: the index build/removal counters of
  `test_repair_on_the_shared_index_commits_the_same_copper` are dropped.

### placement_global_progress
- Skipped (monkeypatched routing internals to record call order or force
  outcomes): `test_iteration_limit_improvement_and_layer_assignment_do_not_add_work`,
  `test_interrupt_never_invents_a_finished_stage`,
  `test_callback_exception_propagates_before_unstarted_work`,
  `test_routeflow_reports_existing_seed_failure_without_extra_attempts`,
  `test_fatal_fixed_source_placement_keeps_original_error_without_retry_or_finish`,
  `test_routeflow_keeps_candidate_trials_and_final_certification_in_order`,
  `test_zero_global_feedback_budget_skips_proposals_but_retains_failed_certification`;
  `test_nested_progress_aggregation_keeps_one_pipeline_count_and_inclusive_spans`
  needs `routing_benchmark.summarize_run`, which is not ported.
- Adapted: the call-order half of
  `test_global_real_result_and_work_order_are_identical_with_callback` is dropped.

### routing_lattice
- The random node round-trip uses a fixed LCG instead of `random.Random(7)`;
  the cached-offsets identity check is dropped (no cache in Rust).

### congestion_feedback_flow
- Adapted: Python forced rejected trials by monkeypatching
  `flow._feedback_score` (the `accept=False` case) and `run_placement_feedback`;
  the Rust tests starve the detailed search (`maximum_expansions: 1`) so no
  move of B lets SIG connect while global routing still succeeds.
- Adapted: `test_feedback_results_round_trip_with_congestion`: Rust has no
  decoder for saved results, so the encoded result is checked to carry the
  congestion map and a replay of the saved inputs to export one. Python's
  `initial_congestion` is recomputed with `feedback_map` (Rust keeps only the
  final map).
- `flow._feedback_score` is private; the step-halving test scores with the
  router metrics (any deterministic score works when reruns never change).

### unified_router_options
- Skipped (scripted `_route_net`/`_conflicts`/`_route_net_once` fakes; no Rust
  seam): `test_failed_nets_are_retried_every_n_iterations`,
  `test_failed_net_gets_one_retry_before_a_conflict_free_exit`,
  `test_unconnected_via_budget_retry_keeps_the_shortfall_diagnostic`,
  `test_connected_via_budget_retry_within_budget_is_kept`.
- Validation runs through `with_overrides` with JSON values (as `bench --set`);
  the tuple/hash check has no counterpart. `open_count` is now `pub(crate)`
  for its unit test; `stitched_pad_board` moved to `support_routers/boards.rs`.
- `RoutingAccess` replaces Python's bare fanout point.

### unified_congestion_export
- `value_at` is not ported (lookups via `tile_of`); `off == on` is dropped
  (no `PartialEq` on the result; copper and net outcomes are compared).

### unified_search_costs
- One search in Rust: the `native` parametrizations run once; skipped
  `test_native_kernel_matches_python_search_with_cost_terms`; adapted
  `test_native_and_python_routes_agree_with_both_cost_terms` to a clean,
  deterministic route with both cost terms. `turn_table` is now `pub(crate)`
  (unit test). Random history uses the shared `Lcg` (support module).

### unified_review_constraints
- Skipped: `test_native_fill_confirms_the_plane_lead_in_connection` (needs a
  native `kicad-cli` refill).
- Adapted: `test_expired_deadline_stops_before_starting_nets` keeps only the
  `unified_setup` case: the Rust router buffers negotiation progress events
  and forwards them after `negotiate` returns, so an observer cannot expire
  the budget at `unified_iteration` start (a progress-timing difference, not
  fixed). The patched clock is a 0.5 s budget the observer sleeps past.
### routing_clearance_remove
- The fixture (board, deterministic copper, every public answer) is
  `support_routers/clearance_remove.rs`; answers are compared as `Debug`
  text. The compaction (`_slots`, `_compact`), unchanged-bins and binned
  drill (`_hole_clear`) tests read private state: unit tests in
  `routing_clearance.rs` (no visibility change).

### global_target_via_cost, global_access_launch_diversity
- They drive the private global router (`route_net`, `terminal_via_cost`,
  `measure_attempt`, `bounded_access_choices`) on hand-built `Graph`s: unit
  tests in `routing.rs`. Dropped: the monkeypatched `_build_graph` /
  `RoutingClearanceIndex` tripwires and `repr(board)` checks (the board is
  borrowed immutably); "same object" checks became equality.

### global_search_static_cache
- The original A* is kept as `reference_search` in the `routing.rs` tests and
  compared with the cached-step `search` (the random trials use `Lcg`).
- Adapted: the patched `heapq` push/pop event comparison (the path is
  compared instead), the patched `length_cost`/`_preference_cost` warm-cache
  check (a warm cache must still answer as the reference) and the point
  identity/IndexError checks.
- Skipped: `test_snapshot_defensively_copies_axes_resources_sites_and_layer_preferences`
  (Rust `Graph` owns its data; `Graph::with` is checked for a fresh cache) and
  `test_complete_route_artifact_and_fingerprints_match_frozen_search`
  (monkeypatched `_search` inside `route_global`).

### routing_acceptance
- `route_case` is private to the `copper` binary; the test repeats its
  pipeline options.

### global_reserved_access
- `FanoutResult.routing_accesses` is built in the test from `accesses` and
  `surface_accesses`. The hard-macro asset of `test_hard_macros.py::fixture`
  and `_array_board` of `test_pad_via_arrays.py` are rebuilt locally (the test
  registers the via-in-pad array materializer as the binary does).
- Adapted: the checked-corner test patched `local_access_path` and used a
  hand-built two-node graph; it uses the real graph and checks the result.

### unified_span_guides
- `technology`/`four_layer` moved to `support_routers/boards.rs`.
- Skipped: `test_guides_survive_every_repair_router` (subclassed `_Router` to
  observe every repair router).

### routing_hotspot_caches
- Rust keeps no SMD-land, courtyard or local-shape caches, so the tests check
  fresh answers after each change. The via-permission cases call the private
  `validate_macro_via_permissions`: unit tests in copper-pcb `hard_macros.rs`
  (which includes `support_routers/boards.rs`; copper-pcb gained a test-only
  `extern crate self as copper_pcb`).
- Skipped: `test_local_shape_translation_matches_direct_geometry_at_arbitrary_angles`
  (`_local_land_shapes` has no Rust counterpart).

### region_blocker_repair
- `exact_repair` is private: unit tests in copper-place `legalize.rs`.
- Skipped: `test_supplemental_raw_blocker_probes_respect_existing_limit`
  (counted calls of a monkeypatched `_legal`).
- Adapted: `test_spread_same_probe_count_detects_right_boundary_blocker_missed_by_prefix`
  captured the references passed to a patched `_pack_local_components`; the
  domain facts are checked and the repair must succeed with the tail kept.

### global_via_refinement
- Unit tests in `routing.rs`. Dropped: the sampler call counts. Adapted:
  `test_global_refinement_fixes_sampling_overflow_and_preserves_passing_routes`
  patched the refiner out to get the coarse result; the refined result is
  checked to pass deterministically without overflow or hotspots.

### released_escape_revalidation
- Skipped (all 4): `flow._revalidate_released_escapes` is not ported (the
  Rust pipeline reports it not implemented).

### global_breakout_demand
- Unit tests in `routing.rs` on the hand-built graph.

### escape_capacity
- Skipped (all 4): `pcbir.escape_capacity` (the escape certificate) has no
  Rust implementation.

### reference_escape_geometry
- Skipped: the three `_interstitial_sites` tests (Rust's fanout has no
  interstitial launch sites). The decoupler end-face test lives in
  `tests/routing_quality.rs` beside its `bypass_board`.

### unified_via_budget_pressure, unified_lead_conflicts
- Unit tests in `unified_router/tests.rs` (`route_net_budgeted` and
  `penalise` are now `pub(crate)`); one search, so `native` runs once.

### unified_pair_budget
- Skipped: `test_pipeline_forwards_the_pair_cap` (spied on
  `flow.route_critical_nets`). `True`/`2.5` caps are JSON overrides.

### global_terminal_fallback_degree, global_terminal_capacity_priority
- Unit tests in `routing.rs` on hand-built graphs. Dropped: the
  `local_access_path` call counts and tripwire; `check_hole_copper` has no
  Rust parameter (the default check is used).

### global_access_diagnostics
- Skipped (monkeypatched `register_terminal_capacity` /
  `_global_pin_access_candidates`): the two failure-cause report tests.

### unified_direct_cli
- Runs the `copper` binary. Dropped: the spies proving the legacy routers
  are not called; the report's work limits, the saved inputs and the final
  board's summary are checked.

### surface_package_escape
- Dropped: the `pin_analysis` assertion (not modelled) and the patched
  `_surface_layer` comparison (a determinism check instead).

### repair_search_reuse
- Skipped: `test_benchmark_counts_reused_attempts_and_treats_the_bound_as_an_intervention`
  (`routing_benchmark.summarize_run` is not ported).

### multi_land_routing, exact_escape
- Skipped: `bench.route_board_file` with the KiCad board importer, and the
  OR-Tools `solve_exact_escape`, have no Rust implementation.

### escape_reservation_lifetime
- Adapted: the `stitch_zone_pads` spy is dropped; the end-to-end result is
  checked (no escape masks survive, no critical retry). Skipped:
  `test_failed_critical_retry_converts_whole_endangered_escape_group` (forced
  a critical miss by patching `route_critical_nets`).

### routing_hotpath_equivalence
- `OutlineSites` is crate-private: a unit test in `fanout.rs`.
