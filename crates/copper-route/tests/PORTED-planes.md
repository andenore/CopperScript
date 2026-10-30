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
