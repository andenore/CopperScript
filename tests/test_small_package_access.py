from dataclasses import replace

from pcbir import (BoardOutline, CopperLayer, CopperZone, FootprintPad, PadReference,
    PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing,
    PolygonWithHoles, Size, Stackup, DetailedRouterOptions, TrackSegment, nm_from_mm)
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.package_access import PackageAccessOptions, preflight_package_access
from pcbir.plane import PlaneStitchOptions
from pcbir.routing import route_global
from test_package_access import fixture


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


def plane_fixture():
    board, _, fanout, _ = fixture()
    board = replace(board, stackup=Stackup(copper_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1,
        CopperLayer.INTERNAL_2, CopperLayer.BACK)),
        zones=(CopperZone("plane", "B", (CopperLayer.INTERNAL_1,),
                         PolygonWithHoles(PolygonRing(board.outline.vertices))),))
    return board, route_global(board), fanout


def test_plane_contacts_are_reserved_by_default_and_opt_out_is_explicit():
    board, guides, settings = plane_fixture()
    result = preflight_package_access(board, guides, settings)
    assert result.ready and result.plane_stitch.complete
    assert set(result.plane_stitch.stitched_pads) == {PadReference("U", "2"), PadReference("JB", "1")}
    late = preflight_package_access(board, guides, settings,
                                    options=PackageAccessOptions(reserve_plane_contacts=False))
    assert late.plane_stitch is None
    selected = preflight_package_access(board, guides, settings,
        plane_options=PlaneStitchOptions(only_pads=frozenset({PadReference("U", "2")})))
    assert selected.plane_stitch.stitched_pads == (PadReference("U", "2"),)


def test_plane_failure_blocks_area_search_instead_of_publishing_partial_access(monkeypatch):
    import pcbir.flow as flow
    import pcbir.package_access as access
    from pcbir.plane import PlaneStitchResult
    from pcbir.routeflow import FeedbackStatus, PlacementRoutingResult
    board, guides, settings = plane_fixture()
    monkeypatch.setattr(flow, "optimize_placement_for_routing", lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, "fixture", (), 0, True))
    monkeypatch.setattr(access, "stitch_zone_pads", lambda source, options:
        PlaneStitchResult(source, (), (PadReference("U", "2"),), 0, 0))
    monkeypatch.setattr(flow, "route_detailed", lambda *args, **kwargs:
                        (_ for _ in ()).throw(AssertionError("area search must be gated")))
    result = flow.run_routing_pipeline(board, fanout_options=settings,
        package_access_options=PackageAccessOptions(maximum_trials=0, maximum_pattern_trials=0))
    assert not result.package_access.ready and result.detailed.metrics.passes == 0
