from __future__ import annotations

from dataclasses import replace
from math import hypot

from pcbir import Point, nm_from_mm
import pcbir.placement as placement

from test_unified_router import _board


def test_ungrouped_parts_start_at_their_connected_pads() -> None:
    board = _board({"SIG": ((3, 10), (17, 10))})
    far = Point.mm(22, 22)
    board = replace(board, placements=(board.placements[0], replace(board.placements[1], position=far)))
    options = placement.PlacementPlannerOptions(fixed_references=frozenset({"SIG_0"}), satellite_seeding=True)
    source = {pose.reference: pose for pose in board.placements}
    seeded = placement._initial_seed(board, source, options, 0)
    anchor = source["SIG_0"].position
    moved = seeded["SIG_1"].position
    assert hypot(moved.x_nm - anchor.x_nm, moved.y_nm - anchor.y_nm) < nm_from_mm(3)
    off = placement._initial_seed(board, source, replace(options, satellite_seeding=False), 0)["SIG_1"].position
    assert hypot(off.x_nm - anchor.x_nm, off.y_nm - anchor.y_nm) > nm_from_mm(10)


def test_swaps_do_not_trade_millimetres_of_wirelength_for_micrometres_of_escape_width() -> None:
    from pcbir.placement import _meaningfully_better
    current = (0, 31_650_836, 0, 2_932_621_664)
    # 10 um less escape deficit, 13 mm more wirelength: not better.
    assert not _meaningfully_better((0, 31_640_836, 0, 2_946_041_664), current)
    # 0.2 mm less escape deficit still wins over wirelength.
    assert _meaningfully_better((0, 31_450_836, 0, 2_946_041_664), current)
    # Within the step, wirelength decides.
    assert _meaningfully_better((0, 31_660_836, 0, 2_932_000_000), current)
    # The relative-rule penalty always comes first.
    assert not _meaningfully_better((1, 0, 0, 0), current)
