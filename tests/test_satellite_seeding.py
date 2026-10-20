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
    options = placement.PlacementPlannerOptions(fixed_references=frozenset({"SIG_0"}))
    source = {pose.reference: pose for pose in board.placements}
    seeded = placement._initial_seed(board, source, options, 0)
    anchor = source["SIG_0"].position
    moved = seeded["SIG_1"].position
    assert hypot(moved.x_nm - anchor.x_nm, moved.y_nm - anchor.y_nm) < nm_from_mm(3)
    off = placement._initial_seed(board, source, replace(options, satellite_seeding=False), 0)["SIG_1"].position
    assert hypot(off.x_nm - anchor.x_nm, off.y_nm - anchor.y_nm) > nm_from_mm(10)
