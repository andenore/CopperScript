# Ported critical-routing and differential-pair tests

Python behaviour specifications (d39067a `tests/test_*.py`) ported to Rust.
Crate-private ports live in `src/critical_tests/` (helpers in
`src/test_support_critical.rs`); public-API ports live in `tests/`.

"Adapted" notes a port whose Python test observed internals through a
monkeypatch: the Rust test checks the same specified outcome without the hook.

| Python file | Tests | Ported | Skipped | Ignored | Rust test file |
|---|---|---|---|---|---|
| test_local_critical.py | 26 | 26 | 0 | 0 | `src/critical_tests/local_critical.rs` |
| test_critical_routing.py | 21 | 21 | 0 | 0 | `src/critical_tests/critical_routing.rs`; the three `_tune_pair` cases in `tests/pair_tuning.rs` |
| test_planned_crossings.py | 17 | 17 | 0 | 0 | `src/critical_tests/planned_crossings.rs` |
| test_dphy_routing_intent.py | 14 | 14 | 0 | 0 | `crates/copper-drc/tests/dphy_routing_intent.rs` |
| test_pair_vias.py | 12 | 12 | 0 | 0 | `src/critical_tests/pair_vias.rs` |
| test_critical_replay.py | 12 | 8 | 4 | 0 | `src/critical_tests/critical_replay.rs`; the CLI case in `crates/copper/tests/critical_replay_cli.rs` |
| test_critical_diagnostics.py | 12 | 9 | 3 | 0 | `src/critical_tests/critical_diagnostics.rs` |
| test_critical_reuse.py | 11 | 0 | 11 | 0 | none: `pcbir.critical_reuse` was not ported |
| test_pair_tuning_spans.py | 10 | 10 | 0 | 0 | `src/critical_tests/pair_tuning_spans.rs` |
| test_pair_return_boundaries.py | 9 | 9 | 0 | 0 | `src/critical_tests/pair_return_boundaries.rs` |

## Notes

### test_local_critical.py

Parametrized Python tests are one Rust test looping over the parameters.
Adapted (monkeypatch-only observations dropped, outcome kept):
- `local_clock_maze_can_enter_a_legal_channel_on_the_other_terminal_grid`:
  the per-phase state budgets were counted through a patched
  `surface_path_between`; the legal path through the off-grid channel is checked.
- `reserved_reverse_phase_still_runs_after_forward_budget_exhaustion`: the
  forward phase was forced to fail; the split-phase maze must reach the
  off-grid end exactly.
- `short_surface_clock_uses_existing_local_maze_before_board_wide_search`:
  Python failed on any `route_unified` call; the `local_surface_maze` strategy
  shows the early return.
- `a_shorter_exact_tree_replaces_a_multi_terminal_guide_tree`: local
  candidates were patched away; `route_single_exact` is called directly.
- `native_rejection_preserves_incumbent_even_if_builder_proposes_shorter_copper`:
  the defective builder's open candidate goes straight to `validate_candidate`.
- `rejected_any_angle_cheap_path_does_not_suppress_bounded_octilinear_search`:
  the any-angle `surface_path` substitute cannot be injected; the maze path is
  checked legal and octilinear.
- `progress_is_observational_and_does_not_change_route_identity`: Python's
  `(event, nets, result)` callback is the `critical_group` progress phase.

### test_critical_routing.py

- `tuning_bumps_leave_breakout_regions_to_the_escaping_lanes`: Python's
  stand-in region object (`x > 14 mm`) is a real `BreakoutRegions` with one
  land whose reach covers the east piece only.
- `joint_search_does_not_invent_copper_behind_a_tapered_port`: Python's
  `_search` generator is driven through a `Continuation` over the escaped board.
- The SHA-256 evidence digest is a fixed 64-hex string rather than a hash.

### test_planned_crossings.py

Rust `route_critical_nets` has no `plan_crossings` switch (it always plans
crossings and nested exits); `plan_bundles` keeps the switch.
- `crossing_pair_routes_with_one_planned_paired_swap_and_return_vias`: the
  unplanned (`plan_crossings=False`) surface-first route is not reachable; the
  planned route and its transition evidence are checked.
- `bundles_without_crossings_route_byte_identically`: `plan_bundles` with and
  without crossing planning must agree, and no bundle document has `crossings`.
- `preflight_prints_each_planned_crossing`: the preflight CLI ran on a patched
  router; the printed line is `crossing_line`, checked on the routed crossing.

### test_dphy_routing_intent.py

Compiler, physicalization and DRC behaviour, so the port lives in
`copper-drc` (which depends on `copper-lang` and `copper-pcb`). Sub-assertions
dropped because the Python API has no Rust counterpart:
`effective_impedance_tolerance_percent` (the 10 % default property) and
`normalize_constraints` (constraint ownership report). Python's
`NetMatchGroup.__post_init__` one-net refusal is checked at the compiler
(`CMP111`) instead.

### test_pair_vias.py

Adapted (monkeypatched engines):
- `critical_owner_accepts_via_pair_without_surface_refinement` and
  `easy_via_pair_is_still_exactly_accepted_with_initial_cap`: the surface
  engine was patched away; the full-height F.Cu walls already leave only the
  via route.
- `aggregate_state_cap_is_shared_across_candidate_calls`: an exhausting
  `_search` stub recorded per-call budgets; the real search must stay within
  and keep the shared cap.
- `critical_limited_tier_samples_both_families_and_all_pitches`: the stubbed
  per-pitch slices are internal; the aggregate (60 states spent, exhausted,
  nothing committed) is checked.

### test_critical_replay.py

Skipped:
- `failed_cohort_owner_retries_without_reopening_accepted_peer`,
  `critical_cohort_rejects_changed_prefix_or_foreign_owner` and
  `corrupt_cohort_is_not_hidden_by_a_single_net_retry`: each injects a fault
  into `route_unified`'s result (a dropped member, a changed prefix, foreign
  copper); the Rust router has no injection seam and no natural board yields
  those results.
- `failed_critical_replay_is_located_for_issues_and_view`: built on
  `test_local_pair_branches.support_fixture` and the workbench issue index
  (`copper issues`); outside this group's routing scope.

Adapted:
- `unified_single_fallback_keeps_the_real_source_profile`: Python patched the
  local surface candidates away; an F.Cu wall (y = 7..25 mm) now defeats the
  bounded local maze, so the generous limit is 60 mm instead of 40 mm.
- `critical_cohort_uses_shared_engine_and_preserves_profile`,
  `clock_and_data_priorities_negotiate_together`: the recorded single
  `route_unified` call is shown by the `unified_critical_cohort` strategy.
- `shared_clock_cohort_still_tunes_declared_matching_group`: only the
  `retry=False` case (the `retry=True` case injected a cohort miss).
- `port_inventory_is_bounded_and_outside_routing_identity`: Python's
  `compare=False` equality becomes the routing fingerprint; the report
  document does carry the inventory.

### test_critical_diagnostics.py

Skipped:
- `preflight_debug_prints_the_full_traceback` and
  `route_board_debug_prints_the_full_traceback`: Python traceback printing
  under `--debug` through a patched exploding stage; no Rust equivalent.
- `preflight_reports_lanes_and_rejections_from_the_critical_stage`: drove the
  preflight CLI with a patched router; the lane table and rejection report it
  printed are covered by the lane-table and rejection tests here.

`lane_table_estimates_screening_delay_from_the_declared_stackup` computes
Python's `propagation_delay` inline (length x sqrt(er) / c).

### test_critical_reuse.py

All skipped: `prepare_single_critical_reuse`, `validate_single_critical_reuse`
and `route_critical_nets(retained_critical=..., retained_accesses=...)` have no
Rust counterpart. The module is on the routing-refactor retire list and no
pipeline stage called it.

### test_pair_tuning_spans.py

- `exact_constructor_uses_fallback_only_with_full_held_context`: Python's
  `tuning_board=None` (no span fallback) has no Rust form; every caller passes
  the held board, so only the tuned outcome is checked.
- `legacy_success_never_enters_span_fallback`: Python patched the fallback to
  raise; here the exact pair's tuned length must equal the legacy tuner's.
- Python's identity checks (`a is b`) are equality checks.

### test_pair_return_boundaries.py

Rust bug found and fixed: `pair_vias::return_via` did not refuse a clearance
index bound to another source (different non-copper fields, or missing source
track/via occurrences), so `old_geometry_index_cannot_authorize_current_source`
failed. The Python binding check is restored.
