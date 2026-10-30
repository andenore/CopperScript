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

## Remaining files (in order)

congestion_feedback, routing_quality, escape_assignment,
unified_nested_routing, route_path_geometry, corridor_reservation,
unified_commit_index, placement_global_progress, routing_lattice,
congestion_feedback_flow, unified_router_options, unified_congestion_export,
unified_search_costs, unified_review_constraints, routing_clearance_remove,
global_target_via_cost, global_search_static_cache, routing_acceptance,
global_reserved_access, global_access_launch_diversity, unified_span_guides,
routing_hotspot_caches, region_blocker_repair, global_via_refinement,
global_edge_strip, released_escape_revalidation, reference_escape_geometry,
global_breakout_demand, escape_capacity, unified_via_budget_pressure,
unified_pair_budget, unified_lead_conflicts, global_terminal_fallback_degree,
global_polygon_keepouts, global_access_diagnostics, unified_existing_access,
unified_direct_cli, surface_package_escape, repair_search_reuse,
multi_land_routing, global_terminal_capacity_priority, exact_escape,
escape_reservation_lifetime, detailed_routing, routing_review_regressions,
routing_hotpath_equivalence.
