"""A legal dogbone is not proof of a usable, jointly allocated package exit."""
from dataclasses import replace
from types import MappingProxyType

import pytest

from pcbir.boundary_access import BoundaryAccessOptions, package_collar
from pcbir.drc import run_physical_drc
from pcbir.fanout import FanoutResult
from pcbir.physical import (BoardOutline, BoardSide, CopperKeepout, CopperLayer, FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles, Size, Stackup, TrackSegment, Via, nm_from_mm)


def rectangle(name, layers, left, top, right, bottom):
    return CopperKeepout(name, layers, PolygonWithHoles(PolygonRing((
        Point.mm(left, top), Point.mm(right, top), Point.mm(right, bottom), Point.mm(left, bottom)))),
        block_tracks=True, block_vias=False, block_zones=False)


def fixture(two=False, layers=None):
    lands = (FootprintPad("1", Point.mm(0, -.5 if two else 0), Size.mm(.3, .3)),)
    if two:
        lands += (FootprintPad("2", Point.mm(0, .5), Size.mm(.3, .3)),)
    package = PhysicalFootprint("package", lands, Size.mm(8, 8))
    one = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm(.3, .3)),), Size.mm(1, 1))
    poses = (Placement("U", "package", Point.mm(6, 6)), Placement("J1", "one", Point.mm(15, 4)))
    nets = (PhysicalNet("A", (PadReference("U", "1"), PadReference("J1", "1"))),)
    if two:
        poses += (Placement("J2", "one", Point.mm(15, 8)),)
        nets += (PhysicalNet("B", (PadReference("U", "2"), PadReference("J2", "1"))),)
    board = PhysicalBoard("boundary", BoardOutline.rectangle(20, 16),
        {"package": package, "one": one}, poses, nets,
        stackup=Stackup(copper_layers=layers) if layers else Stackup())
    tracks, vias, accesses = [], [], {}
    for land, net in zip(lands, nets):
        start = Point(nm_from_mm(6), nm_from_mm(6) + land.position.y_nm)
        end = Point(nm_from_mm(6.8), start.y_nm)
        tracks.append(TrackSegment(net.name, start, end, board.rules.default_track_width_nm, CopperLayer.FRONT))
        vias.append(Via(net.name, end, board.rules.default_via_size_nm, board.rules.default_via_drill_nm))
        accesses[PadReference("U", land.number)] = end
    routed = replace(board, tracks=tuple(tracks), vias=tuple(vias))
    return FanoutResult(routed, MappingProxyType(accesses), (), len(tracks), len(vias),
                         tuple(vias), created_tracks=tuple(tracks))


def ring(layers):
    return (rectangle("left", layers, 2, 2, 2.5, 10),
            rectangle("right", layers, 9.5, 2, 10, 10),
            rectangle("top", layers, 2, 2, 10, 2.5),
            rectangle("bottom", layers, 2, 9.5, 10, 10))


def hard_errors(board):
    return [f for f in run_physical_drc(board).findings if f.severity.value == "error"
            and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


def test_collar_transforms_courtyard_and_includes_outlying_pad():
    fan = fixture()
    fp = replace(fan.board.footprints["package"], courtyard=(
        Point.mm(-2, -1), Point.mm(2, -1), Point.mm(2, 1), Point.mm(-2, 1)))
    fp = replace(fp, pads=(*fp.pads, FootprintPad("9", Point.mm(4, 0), Size.mm(1, 1))))
    board = replace(fan.board, footprints={**fan.board.footprints, "package": fp},
                    placements=(replace(fan.board.placements[0], rotation_degrees=45, side=BoardSide.BACK),
                                *fan.board.placements[1:]))
    collar = package_collar(board, "U", 0)
    assert collar.bounds.min_x < nm_from_mm(3.6) and collar.bounds.min_y < nm_from_mm(4)
    assert collar.bounds.max_x > nm_from_mm(8) and collar.bounds.max_y > nm_from_mm(8)


@pytest.mark.parametrize("kwargs", [{"port_step_nm": 0}, {"maximum_detour_nm": -1},
                                   {"refinement_step_nm": nm_from_mm(1)}, {"maximum_ports_per_edge": 0}])
def test_invalid_bounds(kwargs):
    with pytest.raises(ValueError):
        BoundaryAccessOptions(**kwargs)


