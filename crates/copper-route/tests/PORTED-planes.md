# Ported routing tests: planes, stitching, fanout, vias, tuning, cleanup

Behaviour tests ported from the former Python suite (`tests/test_*.py` at
d39067a). Shared board builders live in `tests/support_planes/mod.rs`.
Counts are per Python test function; a parametrized Python test is one Rust
test that loops over its cases.

| Python file | Ported | Skipped (why) | Ignored | Rust test file |
|---|---|---|---|---|
| test_plane_stitch.py | 24 | 0 | 0 | `tests/plane_stitch.rs` |
| test_length_match_tuning.py | 19 | 0 | 0 | `tests/length_match_tuning.rs`; `src/test_support_planes.rs` (`validate_candidate`) |
| test_route_cleanup.py | 1 (+3 already in `src/route_cleanup.rs`) | 10: 9 `prune_track_stubs` cases (not ported to Rust: its settler never owns mutable copper, see the `route_cleanup` module doc); 1 installed-KiCad case (`kicad-cli` not installed) | 0 | `tests/route_cleanup.rs` |
| test_fanout_and_plane_verify.py | 11 | 3: `run_physical_drc` monkeypatch (Python internals); 2 installed-KiCad cases (`kicad-cli` not installed) | 0 | `tests/fanout_and_plane_verify.rs` |
| test_pad_via_arrays.py | 13 | 0 (the critical-preflight half of the report test: the command is gone) | 0 | `tests/pad_via_arrays.rs`; `crates/copper/tests/via_in_pad_reports.rs` |
| test_pad_stitch.py | 11 | 2: installed-KiCad cases (`kicad-cli` not installed) | 0 | `tests/pad_stitch.rs` |
| test_serpentine_tuning.py | 12 | 0 | 0 | `src/test_support_planes.rs` (7 `UnitTuner` cases); `tests/serpentine_tuning.rs` (5) |
| test_nested_lattice.py | 12 | 0 | 0 | `tests/nested_lattice.rs` (each case runs on both fixtures; Python's seeded `random` is a fixed LCG) |
| test_group_tuning.py | 11 | 0 (parts of 2 tests: see notes) | 0 | `tests/group_tuning.rs` (10); `src/test_support_planes.rs` (`drop_group_steps`) |
| test_breakout_regions.py | 11 | 0 | 0 | `tests/breakout_regions.rs` (9); `src/test_support_planes.rs` (2: `route_single`, `route_pair` + `validate_candidate`) |
| test_interstitial_fanout.py | 10 | 0 (installed-KiCad half of the native-project test and the `MAX_TERMINAL_CAPACITY_PROPOSALS` monkeypatch dropped: see notes) | 0 | `tests/interstitial_fanout.rs` (7); `src/test_support_planes.rs` (3: `legal_choices`, `maze_choices`) |
| test_fanout_late_bypass_completion.py | 7 | 2: the 7-fault rollback test (substituted `route_decouplers` proposals: Python internals, Rust has no seam); the compact-report test (`LateBypassCompletionReport` is not modelled in Rust) | 0 | `tests/fanout_late_bypass_completion.rs` |
| test_surface_power_breakout.py | 8 | 0 | 0 | `tests/surface_power_breakout.rs` |
| test_route_closure.py | 8 | 0 | 0 | `tests/route_closure.rs` |
| test_fanout_access_pressure.py | 6 | 2: the failed-native-gate rollback (`run_physical_drc` monkeypatch: Python internals); pin analysis is observational (`FanoutPinAnalysis` is not modelled in Rust) | 0 | `tests/fanout_access_pressure.rs` |
| test_two_leg_fanout.py | 7 | 0 | 0 | `tests/two_leg_fanout.rs` (5, the parametrized anchor cases loop); `src/test_support_planes.rs` (2: `two_leg_candidates`, `maze_choices`) |
| test_pad_via_in_pad.py | 7 | 0 (the `PadViaInPadRule(..., "open")` constructor check is a source-lowering check: see notes) | 0 | `tests/pad_via_in_pad.rs` |
| test_native_routing_clearance.py | 5 | 1: native refill with an installed KiCad CLI/Python (`kicad-cli` not installed) | 0 | `tests/native_routing_clearance.rs` |
| test_local_pair_branches.py | 6 | 0 | 0 | `tests/local_pair_branches.rs` (parametrized cases loop) |
| test_breakout_region_cache.py | 5 (the storage-bound case adapted: see notes) | 1: the shared source-epoch LRU (`cache_info`: Python internals; Rust has no cache) | 0 | `tests/breakout_region_cache.rs` |
| test_via_lattice.py | 5 | 0 | 0 | `tests/via_lattice.rs` |
| test_via_hole_clearance.py | 5 | 0 | 0 | `tests/via_hole_clearance.rs` (4, parametrized cases loop); `src/test_support_planes.rs` (1: `legal_via_sites`) |
| test_ordinary_neck_down.py | 4 | 1: stub pruning joins neck-down cuts (`prune_track_stubs` has no Rust port, see `route_cleanup`) | 0 | `tests/ordinary_neck_down.rs` |
| test_decoupling_track_containment.py | 5 | 0 | 0 | `src/test_support_planes.rs` (parametrized cases loop) |
| test_decoupling_cell_maze.py | 4 | 1: the shared-budget case (counted `local_access_path` calls and their `maximum_states` through a monkeypatch: Python internals) | 0 | `tests/decoupling_cell_maze.rs` (2); `src/test_support_planes.rs` (2: `cell_maze_launches`) |
| test_cli_router_flags.py | 7 | 0 | 0 | `crates/copper/tests/cli_router_flags.rs` (see notes) |
| test_via_layer_contacts.py | 3 | 1: native smoothing DRC with an installed KiCad CLI (`kicad-cli` not installed) | 0 | `src/test_support_planes.rs` (parametrized finishes loop) |
| test_route_style.py | 4 | 0 (the `allow_movable_conflicts=True` half of the immutable-obstacle case: Rust has no such mode) | 0 | `tests/route_style.rs` |

## Notes

- `test_serpentine_tuning.py`: `UnitTuner.windows`, `.inside`, `.ordered` and
  `window_capacity` are `pub(crate)` for the crate-private cases; the Python
  `tuner.inside = lambda ...` override becomes an explicit `inside` slot set.
  A `tuning_style = "zigzag"` rule cannot be constructed in Rust
  (`TuningStyle::parse` rejects it).

- `test_pad_via_arrays.py`: `ViaArrayPlacementError.diagnostic_board` is not
  carried in Rust; the test asserts the named site rejection and an unchanged
  board. `PadViaInPadRule`'s constructor checks are source-lowering checks in
  Rust (covered by the invalid-parameter cases). Placement legality uses
  `copper_drc::legality::Checker` (`_legal`). The hard-macro case registers
  the array materializer as the `copper` binary does.

- The fake KiCad runner of the Python native-verification tests is a
  stand-in `kicad-cli` shell script (`support_planes::fake_kicad_cli`,
  Unix only) that writes the scripted DRC report.

- Crate-private items are tested in `src/test_support_planes.rs`, which
  includes `tests/support_planes` by path (`extern crate self as copper_route`
  under `cfg(test)` lets the shared builders name the crate).
- `test_length_match_tuning.py::test_preflight_prints_one_line_per_match_group`
  drove the Python `critical_preflight` CLI with routing monkeypatched; the
  Rust test checks the same console line through `match_tuning_line`.
- `test_candidates_completing_an_over_skew_group_are_not_rejected_during_routing`
  is split: the DRC half is an integration test, the `validate_candidate`
  half a crate-private one.

- `test_plane_stitch.py::test_preferred_ground_search_stops_when_later_vias_cannot_win`
  monkeypatched `_candidate_points` to count evaluated candidates; the Rust
  test keeps the observable part (a first-ring via site) only.
- `test_large_land_uses_same_side_pour_without_inventing_via_or_fill` checked
  `board.zone_fills`; the Rust board has no zone fills, so the test asserts no
  copper polygon is invented instead.

- `test_group_tuning.py`: the hemmed-bundle test's last part monkeypatched
  `_tune_bundle_pairs` away to exercise the final pass alone, and
  `test_boards_without_tuning_groups_never_reach_the_group_tuner` replaced
  `GroupTuner` with a refusing stub; Rust has no such seams, so those parts
  are dropped and the observable reports (no group units, no
  `tuning_groups`, no `tuning group` in the line) are kept. Python's
  `_drop_group_steps` took the tuning state as separate dicts; Rust bundles
  them in `RoutingTuning` and results are told apart by `strategy`.
  `tuning_result` (match-tuning document to `MatchTuningResult`) moved to
  `support_planes` and now reads units, groups and lanes.

- `test_breakout_regions.py`: `route_single` and `route_pair` are
  `pub(crate)` for the two crate-private cases (`route_pair` gets the board
  itself as its tuning board and no access-track preference, the Python
  defaults). `_test_point_spacing` takes the square root of the exact
  squared distance in `f64`, as Python did. The JSON checks read `copper_route::drc_report::run_physical_drc`'s
  document. `a_pair_escapes_the_pin_field_only_with_breakout_values` takes
  about 9 s in release (three unroutable searches at a 20 000-state limit).

- `test_interstitial_fanout.py`: `maze_choices` is `pub(crate)`. Rust's
  `legal_choices` takes the outline sites and no pad. The plane-launch case
  called `_stitch_land` directly; Rust's land search is private, so the test
  runs `stitch_zone_pads` limited to U.5 with `include_surface_zones` (a
  two-layer board's B.Cu pour is a surface zone) and checks the same via and
  path. The native-project test keeps the exported rule values; its
  installed-KiCad half is not ported (`kicad-cli` not installed). The
  terminal-budget part that set `MAX_TERMINAL_CAPACITY_PROPOSALS` to 0 is
  dropped (a Rust constant). Rust's `can_via` always checks hole copper
  (Python's `check_hole_copper=True`).

- `test_fanout_late_bypass_completion.py`: Rust's `complete_late_bypasses`
  returns no completion report (no `late_bypass` status, passes or reason)
  and `FanoutResult` carries no `pin_analysis`; a completed pass is checked
  as cleared pending obligations with appended back-side semantic tracks,
  an unchanged one as a field-by-field copy of the incumbent. The normal
  fanout case patched `_legal_choices` away to force the early bypass to
  fail and observed `route_decouplers`; without those seams it checks the
  shared end state (link verified, ledger covered, created copper equal to
  the board's). The obstructed half of the old-witness case needed a
  substituted proposal and is dropped. The one-capacitor-two-targets board
  check runs where Rust makes it, when a board state is decoded. Scene
  builders are in `support_planes/decoupling.rs`.

- `test_route_closure.py`: the Python tests passed a `SimpleNamespace` as
  the pipeline; Rust's `routing_complete_with_fill` takes a
  `RoutingPipelineResult`, so the tests run the pipeline once on the
  closure board and set the fields each case names (status, certification,
  global guide, critical status and nets, detailed result).

- `test_fanout_access_pressure.py`: Rust has no `_candidates` seam, so the
  four tests that scripted candidate domains run on the physical-window board
  of the Python `test_real_candidate_generation...` case (via keepouts tile the
  board and leave whole-via windows open): U.1 reaches (3, 6) and (5, 4), U.2
  only (3, 6). The locked-exit case opens only (3, 6). `FanoutPinAnalysis` is
  not modelled; the candidate-count and diagnostic checks become accesses,
  pending pads and unchanged copper (constrained-first order shows as the
  access order). `refined_candidate_count` of the radial-sampling case is
  covered by the off-grid (4.9, 6) access the 0.5 mm coarse grid cannot reach.
  Scene builders (`trap_board`, `via_windows`, `two_leg_board`) are in
  `support_planes/fanout.rs`.

- `test_two_leg_fanout.py`: `two_leg_candidates` and `maze_choices` are
  already `pub(crate)`. The bounded-maze case emptied `_legal_choices` to reach
  the maze through `route_fanout`; Rust has no seam, so it calls
  `maze_choices` on the same pin and checks its candidate as committed copper
  (verified anchor, DRC, determinism, budget 1 finds nothing). The
  `pin_analysis` two-leg counts are not modelled. A zero site budget is
  rejected by `FanoutOptions::validated`.

- `test_pad_via_in_pad.py`: the `replace(board, via_in_pad_rules=...)`
  checks are `copper_pcb::physicalize::revalidate`. Rust's `PadViaInPadRule`
  has no process field, so the wrong-process case is the `"open"` source
  case. The editor-digest case uses `copper_editor::overlay` (a
  `copper-route` dev-dependency); Rust boards have no `zone_fills`.

- `test_native_routing_clearance.py`: `routing_clearance_rules` is private
  to `copper_pcb::kicad_pcb`; the tests read the net clearance rules from the
  generated `.kicad_dru` (no file is written when there are none, Python's
  empty tuple). The tests live with the other ported routing tests for the
  shared builders.

- `test_local_pair_branches.py`: scenes are in `support_planes/pairs.rs`
  (`crossed_contacts`, and `pair_board`, the `_pair_board` of
  `test_critical_routing.py`). The fixture's float land rows are built on an
  exact quarter-millimetre grid.

- `test_breakout_region_cache.py`: `copper_drc::breakout::BreakoutRegions`
  keeps no membership cache, so the `_nearest_region` call counts and cache
  sizes have no Rust counterpart. The storage-bound case checks that 8300
  distinct queries leave the answers exact; the changed-source case asserts
  each variant's own physical answer (moved U, 1 nm breakout and swapped nets
  leave no region; tiny lands, a clearance change and six layers keep U.1's)
  instead of comparing against `_nearest_region`. `bypass_board` (of
  `test_routing_quality.py`) is in `support_planes`.

- `test_via_lattice.py`: `ball_grid` (of `test_escape_plan.py`) is in
  `support_planes`; `plan_via_lattices` takes the Python keyword defaults
  (`surface_rings=2`, `band_rings=1`) explicitly.

- `test_via_hole_clearance.py`: `routing::legal_via_sites` is `pub(crate)`
  for the crate-private global via-site case.

- `test_ordinary_neck_down.py`: `checked_access_path` is called with the
  Python defaults (`allow_movable_conflicts=False`, `allow_orthogonal=True`);
  `Counter(result) != Counter(owned)` compares sorted track lists.

- `test_decoupling_track_containment.py`: `flow::track_is_contained` was
  already `pub(crate)`; `decoupling_paths::connections`, `ConnectionWork`
  (with `local_paths`), `Emit` and `Land` are now `pub(crate)`. Python's
  object identity (`is held`) is equality.

- `test_decoupling_cell_maze.py`: `cell_grid` is in
  `support_planes/decoupling.rs`; `cell_maze_launches` is `pub(crate)`. The
  recovery case stubbed `_cell_maze_launches` away for its failing baseline;
  Rust has no seam and the maze only runs with `maze_escapes`, so the
  baseline disables it. The no-work cases forbade `local_access_path` through
  a monkeypatch; Rust checks that nothing is emitted and the shared
  `local_paths` allowance is unchanged.

- `test_cli_router_flags.py`: runs the `copper` binary. Every retired switch
  is already checked in-crate (`route_board::option_tests`), so the binary
  test checks three spellings end to end. The parser-level
  `_congestion_feedback_budget`, `_unified_options` and
  `_apply_routing_profile` checks are observed in the report's `work_limits`
  (congestion budget 4/2/0; `fine_regions` off/arrays/dense); the
  direct-mode overrides are covered by the in-crate
  `expert_overrides_govern_direct_mode_and_reports`.

- `test_via_layer_contacts.py`: `route_smoothing::contacts_preserved`,
  `route_cleanup::Settler` (its fields and `via_layers`) and
  `route_cleanup::net_pad_shapes` are `pub(crate)`. Rust's
  `via_copper_contact` takes the stackup from a board, so the span case
  builds a three-layer board instead of passing the layers.
