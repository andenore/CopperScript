"""Geometric and transaction regressions from the six-layer routing review."""

from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size, TrackSegment, Via, nm_from_mm)
from pcbir.routing_costs import length_cost, preference_cost


def fanout_board():
    footprint = PhysicalFootprint(
        "test/one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    placements, nets, tracks, vias, accesses = [], [], [], [], {}
    for net, y in (("A", 3), ("B", 8)):
        pads = []
        for suffix, x, anchor_x in (("1", 3, 4), ("2", 12, 11)):
            pad = PadReference(net + suffix, "1")
            position, anchor = Point.mm(x, y), Point.mm(anchor_x, y)
            placements.append(Placement(pad.component, footprint.name, position))
            pads.append(pad)
            accesses[pad] = anchor
            tracks.append(TrackSegment(net, position, anchor, nm_from_mm("0.2"), CopperLayer.FRONT))
            vias.append(Via(net, anchor, nm_from_mm("0.6"), nm_from_mm("0.3")))
        nets.append(PhysicalNet(net, tuple(pads)))
        tracks.append(TrackSegment(net, Point.mm(4, y), Point.mm(11, y), nm_from_mm("0.2"), CopperLayer.BACK))
    board = PhysicalBoard(
        "SubsetRepair", BoardOutline.rectangle(15, 12), {footprint.name: footprint},
        tuple(placements), tuple(nets), tracks=tuple(tracks), vias=tuple(vias),
    )
    return board, accesses


def test_costs_are_physical_and_invariant_to_edge_splitting():
    a, b, c = Point.mm(0, 0), Point.mm("0.3", 0), Point.mm(1, 0)
    def cost(first, second):
        return length_cost(first, second) + preference_cost(first, second, 2, 4, 2)
    assert cost(a, c) == cost(a, b) + cost(b, c)
    assert cost(a, c) == 20_000_000


