from __future__ import annotations

from dataclasses import replace

from pcbir import Point
from pcbir.layout import placement_intent_findings
from pcbir.physical import ComponentPlacementRule

from test_unified_router import _board


def test_remote_fixed_part_is_reported() -> None:
    board = _board({"SIG": ((3, 10), (17, 10))})
    rules = tuple(ComponentPlacementRule(pose.reference, fixed_position=pose.position) for pose in board.placements)
    board = replace(board, placement_rules=rules)
    codes = [f.code for f in placement_intent_findings(board)]
    assert codes.count("REMOTE_FIXED_PART") == 2
    near = replace(board, placements=(board.placements[0], replace(board.placements[1], position=Point.mm(8, 13))))
    near = replace(near, placement_rules=tuple(
        ComponentPlacementRule(pose.reference, fixed_position=pose.position) for pose in near.placements))
    assert not [f for f in placement_intent_findings(near) if f.code == "REMOTE_FIXED_PART"]
