from dataclasses import replace
import pytest

from pcbir.physical import (BoardSide, CopperKeepout, CopperLayer, PadKind,
                            Point, PolygonRing, PolygonWithHoles, Size)
from pcbir.placement import _legal, PlacementPlannerOptions
from test_routing_quality import bypass_board


@pytest.mark.parametrize('angle', (0, 90, 180, 270))
@pytest.mark.parametrize('reverse', (False, True))
def test_rectangular_land_corner_crossing_concave_barrier_matches_drc(angle, reverse):
    from pcbir.physical import BoardOutline, PhysicalBoard, PhysicalFootprint, FootprintPad, Placement
    from pcbir.placement import transformed_local_point
    from pcbir.drc import run_physical_drc, PhysicalDrcPolicy
    frame = Placement('frame', 'land', Point.mm(15,15), rotation_degrees=angle)
    def point(x,y):
        return transformed_local_point(frame, Point.mm(str(x-15), str(y-15)))
    polygon = tuple(point(x,y) for x,y in ((2,2),(3,2),(3,8),(12,8),(12,9),(2,9)))
    if reverse:
        polygon = tuple(reversed(polygon))
    keepout = CopperKeepout('bent-barrier', (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(polygon)), block_pads=True, block_footprints=False)
    footprint = PhysicalFootprint('land', (FootprintPad('1', Point(0,0), Size.mm('1.52','1.78')),), Size.mm('1.52','1.78'))
    pose = Placement('PAD','land', point(11,7.8), rotation_degrees=angle)
    board = PhysicalBoard('CornerCrossing', BoardOutline.rectangle(30,30), {'land':footprint},
        (pose,), (), copper_keepouts=(keepout,))
    options = PlacementPlannerOptions()
    assert not _legal(pose, {}, board, options)
    report = run_physical_drc(board, policy=PhysicalDrcPolicy(require_completed_detailed_route=False))
    assert any(f.code == 'DRC-COPPER-KEEPOUT' for f in report.findings)
    clear = replace(pose, position=point(10,6))
    assert _legal(clear, {}, board, options)  # Concave notch is usable, not a convex hull.
    report = run_physical_drc(replace(board, placements=(clear,)), policy=PhysicalDrcPolicy(require_completed_detailed_route=False))
    assert not any(f.code == 'DRC-COPPER-KEEPOUT' for f in report.findings)


def board_with_strip(x0, x1, layers=(CopperLayer.FRONT,)):
    board = bypass_board()
    strip = CopperKeepout("isolation", layers, PolygonWithHoles(PolygonRing((
        Point.mm(x0, 4), Point.mm(x1, 4), Point.mm(x1, 12), Point.mm(x0, 12)))),
        block_pads=True, block_footprints=False)
    return replace(board, copper_keepouts=(strip,))


def test_pad_only_barrier_allows_body_but_rejects_copper():
    options = PlacementPlannerOptions()
    board = board_with_strip(7.8, 8.2)
    pose = board.placements[0]
    assert _legal(pose, {}, board, options)  # Insulating body crosses, lands do not.
    blocked = board_with_strip(8.9, 9.1)
    assert not _legal(pose, {}, blocked, options)
    assert _legal(replace(pose, side=BoardSide.BACK), {}, blocked, options)
    assert not _legal(replace(pose, rotation_degrees=90), {},
                      board_with_strip(7.9, 8.1), options)


def test_unconnected_plated_pad_obeys_inner_layer_barrier():
    board = board_with_strip(8.9, 9.1, (CopperLayer.BACK,))
    fp = board.footprints["chip"]
    fp = replace(fp, pads=tuple(replace(p, kind=PadKind.THROUGH_HOLE,
                                        drill=Size.mm(".2", ".2")) for p in fp.pads))
    board = replace(board, footprints={**board.footprints, "chip": fp}, nets=(), decoupling_links=())
    assert not _legal(board.placements[0], {}, board, PlacementPlannerOptions())
