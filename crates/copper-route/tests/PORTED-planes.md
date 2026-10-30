# Ported routing tests: planes, stitching, fanout, vias, tuning, cleanup

Behaviour tests ported from the former Python suite (`tests/test_*.py` at
d39067a). Shared board builders live in `tests/support_planes/mod.rs`.
Counts are per Python test function; a parametrized Python test is one Rust
test that loops over its cases.

| Python file | Ported | Skipped (why) | Ignored | Rust test file |
|---|---|---|---|---|
| test_plane_stitch.py | 24 | 0 | 0 | `tests/plane_stitch.rs` |

## Notes

- `test_plane_stitch.py::test_preferred_ground_search_stops_when_later_vias_cannot_win`
  monkeypatched `_candidate_points` to count evaluated candidates; the Rust
  test keeps the observable part (a first-ring via site) only.
- `test_large_land_uses_same_side_pour_without_inventing_via_or_fill` checked
  `board.zone_fills`; the Rust board has no zone fills, so the test asserts no
  copper polygon is invented instead.
