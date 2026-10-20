"""The raster plane estimate names the pads it expects to stay cut off."""

from dataclasses import replace

from pcbir import (
    BoardOutline, CopperLayer, CopperZone, FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles, Size,
)
from pcbir.plane_estimate import estimate_plane_connectivity


def _board(zone_right_mm):
    footprint = PhysicalFootprint("one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
                                  Size.mm(1, 1))
    placements = tuple(Placement(f"J{i}", footprint.name, Point.mm(x, 6))
                       for i, x in enumerate((3, 7, 17), 1))
    net = PhysicalNet("GND", tuple(PadReference(p.reference, "1") for p in placements))
    board = PhysicalBoard("estimate", BoardOutline.rectangle(20, 12), {footprint.name: footprint},
                          placements, (net,))
    zone = CopperZone("plane", "GND", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(1, 1), Point.mm(zone_right_mm, 1), Point.mm(zone_right_mm, 11), Point.mm(1, 11)))))
    return replace(board, zones=(zone,))


def test_pads_outside_the_largest_group_are_named():
    estimate = estimate_plane_connectivity(_board(10))
    assert estimate.opens == 1
    assert estimate.report()["open_nets"] == ["GND"]
    assert estimate.report()["open_pads"] == {"GND": ["J3.1"]}


def test_a_connected_plane_names_no_pads():
    estimate = estimate_plane_connectivity(_board(19))
    assert estimate.opens == 0 and estimate.report()["open_pads"] == {}
