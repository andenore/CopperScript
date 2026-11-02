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
| test_critical_replay.py | 12 | 8 | 4 | 0 | `src/critical_tests/critical_replay.rs`; the CLI case in `crates/copper/tests/critical_cli.rs` |
| test_critical_diagnostics.py | 12 | 9 | 3 | 0 | `src/critical_tests/critical_diagnostics.rs` |
| test_critical_reuse.py | 11 | 0 | 11 | 0 | none: `pcbir.critical_reuse` was not ported |
| test_pair_tuning_spans.py | 10 | 10 | 0 | 0 | `src/critical_tests/pair_tuning_spans.rs` |
| test_pair_return_boundaries.py | 9 | 9 | 0 | 0 | `src/critical_tests/pair_return_boundaries.rs` |
| test_pair_terminal_fallback.py | 8 | 8 | 0 | 0 | `src/critical_tests/pair_terminal_fallback.rs`; the CLI case in `crates/copper/tests/critical_cli.rs` |
| test_pair_geometry.py | 8 | 0 | 8 | 0 | none: `pcbir.pair_geometry` was not ported |
| test_critical_bundles.py | 8 | 6 | 2 | 0 | `src/critical_tests/critical_bundles.rs` (the frozen-fixture comparison stays in `tests/critical_bundles.rs`) |
| test_single_ended_return_vias.py | 7 | 4 | 3 | 0 | `src/critical_tests/single_ended_return_vias.rs` |
| test_pair_search_resume.py | 7 | 6 | 1 | 0 | `src/critical_tests/pair_search_resume.rs` |
| test_pair_joint_supports.py | 7 | 7 | 0 | 0 | `src/critical_tests/pair_joint_supports.rs` |
| test_critical_preflight.py | 7 | 0 | 7 | 0 | none: the `critical-preflight` command was not ported |
| test_pair_opposite_faces.py | 6 | 6 | 0 | 0 | `src/critical_tests/pair_opposite_faces.rs` |
| test_pair_converging_ports.py | 6 | 6 | 0 | 0 | `src/critical_tests/pair_converging_ports.rs` |
| test_critical_lane_review.py | 6 | 5 | 1 | 0 | `src/critical_tests/critical_lane_review.rs`; the route-board case in `crates/copper/tests/critical_cli.rs` |
| test_pair_taper.py | 5 | 5 | 0 | 0 | `src/critical_tests/pair_taper.rs` |
| test_pair_recessed_via_ports.py | 5 | 4 | 1 | 0 | `src/critical_tests/pair_recessed_via_ports.rs` |
| test_pair_refine.py | 4 | 4 | 0 | 0 | `src/critical_tests/pair_refine.rs` |
| test_pair_budget_escalation.py | 4 | 4 | 0 | 0 | `src/critical_tests/pair_budget_escalation.rs` |
| test_critical_search_order.py | 4 | 4 | 0 | 0 | `src/critical_tests/critical_search_order.rs` |
| test_recessed_pair_ports.py | 3 | 3 | 0 | 0 | `src/critical_tests/recessed_pair_ports.rs` |
| test_planned_escape_copper.py | 3 | 3 | 0 | 0 | `src/critical_tests/planned_escape_copper.rs` |
| test_pair_through_hole_contacts.py | 3 | 3 | 0 | 0 | `src/critical_tests/pair_through_hole_contacts.rs` |
| test_pair_hybrid_contact_anchor.py | 3 | 3 | 0 | 0 | `src/critical_tests/pair_hybrid_contact_anchor.rs` |
| test_pair_collinear_fork.py | 3 | 3 | 0 | 0 | `src/critical_tests/pair_collinear_fork.rs` |
| test_pair_outer_transitions.py | 2 | 2 | 0 | 0 | `src/critical_tests/pair_outer_transitions.rs` |

**Totals so far:** 31 of 46 files, 256 Python tests: 212 ported, 44
skipped, 0 ignored.

Rust bugs found and fixed by the ports:
- `pair_vias::return_via` accepted a clearance index bound to another source
  (`test_pair_return_boundaries.py`).
- `route_critical_nets` accepted a negative `bundle_repair_limit`
  (`test_critical_bundles.py`).
- `route-board` length-option errors lost Python's positive/nonnegative wording
  (`test_pair_terminal_fallback.py`).

Remaining files, in the assigned order: pair_tuning_span_copper_contacts,
critical_stage_planning, critical_return_preparation, bundle_tuning,
partial_pair_macro_ports, pair_transition_diagnostics, pair_offset_ports,
pair_obstacle_tuning_fallback, pair_approach_ranking, loose_pair_repair,
collinear_pair_ports, pair_transition_families, pair_staggered_contacts,
pair_lateral_ports, pair_departure.

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

### test_pair_terminal_fallback.py

Rust divergence fixed: `route-board` refused a non-positive length option with
"expected a finite length in mm", dropping Python's "positive"/"nonnegative"
bound, so `cli_exposes_positive_computational_pin_access_step` failed. The
Python messages are restored.

### test_pair_geometry.py

All skipped: `differential_pair_inventory` (the read-only
`differential_pair_screens` report) has no Rust port. The routing-refactor
plan still lists `pair_geometry` as a kept read-only report, so this is a
missing port rather than a retired feature.

### test_critical_bundles.py

Rust bug found and fixed: `route_critical_nets` accepted a negative
`bundle_repair_limit` (silently disabling repair) where Python raised
"critical bundle repair limit cannot be negative".

Skipped:
- `rejected_bundle_repair_restores_the_previous_state_exactly`: injects a
  rejection into `_route_pair_group` after a successful repair leg; no Rust
  seam, and no natural board rejects a connected leg.
- `preflight_reports_the_bundle_order_and_repairs`: the `critical_preflight`
  CLI is not ported.

### test_single_ended_return_vias.py

`negotiation_uses_existing_nearby_returns_without_adding_or_dropping_them`
was parametrized over the Python and native A*; Rust has one search.

Skipped:
- `exact_commit_cannot_be_bypassed_by_a_permissive_raster`: patches the
  unified router's private `_via_owners` raster; no Rust seam.
- `native_refill_confirms_existing_returns_join_both_reference_faces`: needs
  `kicad-cli` (skipped in Python when absent) and filled-plane verification,
  which the planes group owns.
- `early_escape_candidates_and_reservations_obey_existing_return_requirement`:
  exercises fanout internals (`_legal_choices`, `_reserve`, `_maze_choices`),
  owned by the fanout group.

### test_pair_search_resume.py

Python's `_search` + `register_pair_search` is a `Continuation` built over
`SpineSearch` and pushed onto `stats.continuations`.

Skipped:
- `stale_source_prefix_and_escaped_geometry_are_rejected`: Rust
  `resume_pair_candidates` takes no source board and keeps no ownership
  fingerprints; each frontier owns its immutable escaped board and the
  critical owner resumes only its own per-board stats, so a stale source or
  replaced escaped board cannot reach it.

### test_pair_joint_supports.py

`support_fixture` (from `test_local_pair_branches.py`) is rebuilt here; the
return-boundary and pair-via fixtures are shared with their ports.
`len(complete.completed)` reads a test-only `completed_len()`.

### test_critical_preflight.py

All skipped: the `critical-preflight` command (and its checkpoint/partial
artifact interruption hooks, all driven through monkeypatched stages) has no
Rust counterpart.

### test_pair_opposite_faces.py

- `critical_owner_accepts_opposite_face_transition`: Python patched the
  surface search away; terminals on opposite faces leave the single matched
  transition as the owner's only route.

### test_critical_lane_review.py

`match_tuning` is the stage's tuning documents (`id`, `status`) in Rust.

Skipped:
- `preflight_reports_the_review_of_the_critical_stage`: the
  `critical-preflight` CLI is not ported (the route-board report case is).

### test_pair_recessed_via_ports.py

`test_pair_outer_transitions.py`'s fixture is shared from its port.

Skipped:
- `local_collar_calls_have_a_finite_shared_allowance`: counted the
  `maximum_states` of each `local_access_path` call through a monkeypatch;
  Rust `local_via_ports` has the same 32-call / 16-site bound but no seam to
  observe the calls.

### test_pair_refine.py

- `owner_preserves_complete_accepted_incumbent_on_bad_shortcut`: Python
  injected each defective shortcut into `_improve_pair_spine`. Here the gap
  and native-open defects fail the owner's exact gates
  (`route_pair_exact` + `validate_candidate`), the profile defect is a
  skew-warned fallback, and `improve_pair_spine` over the real shortcuts
  keeps a complete connected pair no longer than the incumbent. The width
  and layer defects are dropped by the owner's private faithfulness filter,
  which has no seam.

### test_critical_search_order.py

Python's `SimpleNamespace` guides are `GlobalNetRoute`s with one access.
`critical_progress` is the crate-private `critical_group` emitter.

### test_planned_escape_copper.py

`test_unified_router._board` is rebuilt here; `_release_escape_masks`,
`_copper_keepouts` and `_plane_pin_halos` are the crate-private `flow`
functions.
