from __future__ import annotations

from dataclasses import replace

from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size, nm_from_mm, TrackSegment, Via)
from pcbir.routing_clearance import RoutingClearanceIndex


def _board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    return PhysicalBoard(
        "DetailedRoute",
        BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(3, 6)),
            Placement("J2", footprint.name, Point.mm(17, 6)),
        ),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
    )


def test_same_net_candidate_vias_must_meet_drill_spacing() -> None:
    board = _board()
    clearance = RoutingClearanceIndex(board)
    first = Via(
        "SIGNAL", Point.mm(10, 6), nm_from_mm("0.8"), nm_from_mm("0.4"),
        CopperLayer.FRONT, CopperLayer.BACK,
    )
    too_close = replace(first, position=Point.mm("10.5", 6))
    legal = replace(first, position=Point.mm(12, 6))

    assert clearance.can_via(first.net, first.position, first.size_nm,
                             first.from_layer, first.to_layer)
    assert clearance.can_via(too_close.net, too_close.position, too_close.size_nm,
                             too_close.from_layer, too_close.to_layer)
    assert not clearance.candidate_vias_clear((first, too_close))
    assert clearance.candidate_via_conflict(
        (first, too_close), allow_movable_conflicts=True,
    ) == too_close
    assert clearance.candidate_vias_clear((first, legal))


def test_clearance_index_distinguishes_movable_from_locked_blockers() -> None:
    board = _board()
    candidate = TrackSegment(
        "OTHER", Point.mm(10, 3), Point.mm(10, 9),
        nm_from_mm("0.25"), CopperLayer.FRONT,
    )
    crossing = TrackSegment(
        "SIGNAL", Point.mm(3, 6), Point.mm(17, 6),
        nm_from_mm("0.25"), CopperLayer.FRONT,
    )
    clearance = RoutingClearanceIndex(board)
    clearance.add_track(crossing)

    assert clearance.blocking_track_nets(candidate) == (
        frozenset({"SIGNAL"}), False,
    )
    locked = RoutingClearanceIndex(replace(board, tracks=(crossing,)))
    assert locked.blocking_track_nets(candidate) == (frozenset(), True)
