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
