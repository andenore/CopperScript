from dataclasses import replace

from pcbir import (BoardOutline, FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size, TrackSegment, nm_from_mm)
from pcbir.fanout import FanoutOptions, route_fanout


def small_package(pitch=.65):
    pads = tuple(FootprintPad(str(i + side * 4 + 1), Point.mm(-2.15 if side == 0 else 2.15,
        (i - 1.5) * pitch), Size.mm(1.1, .4)) for side in (0, 1) for i in range(4))
    package = PhysicalFootprint("tssop8", pads, Size.mm(3, 3))
    one = PhysicalFootprint("source", (FootprintPad("1", Point(0, 0), Size.mm(.5, .5)),), Size.mm(1, 1))
    return PhysicalBoard("small dense", BoardOutline.rectangle(25, 20),
        {f.name: f for f in (package, one)},
        (Placement("U", package.name, Point.mm(10, 10)), Placement("P", one.name, Point.mm(20, 10))),
        (PhysicalNet("V3V3", (PadReference("U", "1"), PadReference("U", "7"), PadReference("P", "1"))),))


def test_default_fanout_covers_eight_pin_dense_package_but_not_sparse_package():
    board = small_package()
    result = route_fanout(board)
    assert set(result.accesses) == {PadReference("U", "1"), PadReference("U", "7")}
    assert not result.pending_pads
    assert not route_fanout(small_package(pitch=2)).pin_analysis
    assert not route_fanout(board, FanoutOptions(escape_small_dense_packages=False)).pin_analysis


def test_default_fanout_does_not_force_two_terminal_passive_vias():
    board = small_package()
    package = replace(board.footprints["tssop8"], name="passive", pads=board.footprints["tssop8"].pads[:2])
    passive = replace(board, footprints={"passive": package},
        placements=(Placement("R", "passive", Point.mm(10, 10)),),
        nets=(PhysicalNet("V3V3", (PadReference("R", "1"), PadReference("R", "2"))),))
    result = route_fanout(passive)
    assert not result.pin_analysis and not result.board.vias
