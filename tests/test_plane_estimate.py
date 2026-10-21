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


def _walled_board():
    from pcbir import TrackSegment, nm_from_mm
    from pcbir.plane import PlaneStitchOptions, stitch_zone_pads
    board = _board(19)
    back = CopperZone("plane-b", "GND", (CopperLayer.BACK,), PolygonWithHoles(PolygonRing((
        Point.mm(1, 1), Point.mm(19, 1), Point.mm(19, 11), Point.mm(1, 11)))))
    board = replace(board, zones=(*board.zones, back), nets=(*board.nets, PhysicalNet("X", ())))
    stitched = stitch_zone_pads(board, PlaneStitchOptions(maximum_radius_nm=nm_from_mm(2), include_surface_zones=True))
    j3 = Point.mm(17, 6)
    # Drop J3's own contact (as if it relied on the F.Cu pour) and wall it in with a foreign trace.
    own_via = min(stitched.board.vias, key=lambda v: abs(v.position.x_nm - j3.x_nm) + abs(v.position.y_nm - j3.y_nm))
    keep = tuple(t for t in stitched.board.tracks if own_via.position not in (t.start, t.end))
    corners = [Point.mm(15.3, 4.3), Point.mm(18.7, 4.3), Point.mm(18.7, 7.7), Point.mm(15.3, 7.7)]
    ring = tuple(TrackSegment("X", corners[i], corners[(i + 1) % 4], nm_from_mm("0.2"), CopperLayer.FRONT)
                 for i in range(4))
    walled = replace(stitched.board, tracks=(*keep, *ring),
                     vias=tuple(v for v in stitched.board.vias if v is not own_via))
    return replace(stitched, board=walled)


def test_walled_off_pad_is_restitched_with_explicit_copper():
    from pcbir import nm_from_mm
    from pcbir.plane import PlaneStitchOptions, repair_cut_off_plane_pads
    stitch = _walled_board()
    assert estimate_plane_connectivity(stitch.board).report()["open_pads"] == {"GND": ["J3.1"]}
    repaired, still = repair_cut_off_plane_pads(stitch, PlaneStitchOptions(maximum_radius_nm=nm_from_mm(2), include_surface_zones=True))
    assert still == ()
    assert len(repaired.board.vias) == len(stitch.board.vias) + 1
    assert estimate_plane_connectivity(repaired.board).opens == 0
