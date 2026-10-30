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
