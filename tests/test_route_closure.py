from dataclasses import replace

from pcbir import (
    BoardOutline, CopperLayer, CopperZone, FootprintPad, PadReference,
    PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size,
    PolygonRing, PolygonWithHoles, TrackSegment, nm_from_mm,
    run_routing_pipeline, PlacementPlannerOptions, PlacementRoutingFeedbackOptions,
)
from pcbir.detailed import (DetailedNetResult, DetailedRoutingMetrics,
                            DetailedRoutingResult, DetailedRoutingStatus)
from pcbir.route_closure import close_detailed_lands


def _result(*, reported: bool, copper: bool) -> DetailedRoutingResult:
    footprint = PhysicalFootprint("test/terminal", (
        FootprintPad("1", Point(0, 0), Size.mm(1, 1)),), Size.mm(1, 1))
    tracks = (TrackSegment("A", Point.mm(3, 5), Point.mm(7, 5),
                           nm_from_mm("0.2"), CopperLayer.FRONT),) if copper else ()
    board = PhysicalBoard(
        "Closure", BoardOutline.rectangle(10, 10), {footprint.name: footprint},
        tuple(Placement(ref, footprint.name, Point.mm(x, 5)) for ref, x in (("J1", 3), ("J2", 7))),
        (PhysicalNet("A", (PadReference("J1", "1"), PadReference("J2", "1"))),),
        tracks=tracks, metadata={"detailed_routing": "complete"},
    )
    return DetailedRoutingResult(
        DetailedRoutingStatus.SUCCESS, board,
        (DetailedNetResult("A", reported, len(tracks), 0, nm_from_mm(4) if copper else 0,
                           0, () if reported else ("search budget exhausted",)),),
        DetailedRoutingMetrics(int(reported), int(not reported), 0, 0,
                               len(tracks), 0, nm_from_mm(4) if copper else 0, 1),
        0, 0, "global", "routing",
    )


def test_closure_does_not_promote_failed_search_flags() -> None:
    initial = _result(reported=False, copper=True)
    closed, drc, _ = close_detailed_lands(initial)
    assert not closed.nets[0].connected
    assert closed.nets[0].diagnostics == initial.nets[0].diagnostics
    assert closed.status is DetailedRoutingStatus.PARTIAL
    assert closed.board.metadata["detailed_routing"] == "partial"
    assert "DRC-ROUTE-INCOMPLETE" in {item.code for item in drc.findings}


def test_closure_rejects_stale_success_without_copper() -> None:
    closed, drc, _ = close_detailed_lands(_result(reported=True, copper=False))
    assert not closed.nets[0].connected
    assert closed.metrics.unrouted_net_count == 1
    assert closed.status is DetailedRoutingStatus.PARTIAL
    assert {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"} <= {item.code for item in drc.findings}


def test_default_zone_deferral_does_not_trigger_placement_repair() -> None:
    board = _result(reported=False, copper=False).board
    board = replace(board, zones=(CopperZone(
        "plane", "A", (CopperLayer.BACK,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in (
            (1, 1), (9, 1), (9, 9), (1, 9))))),
    ),), metadata={})
    pipeline = run_routing_pipeline(
        board, placement_options=PlacementPlannerOptions(
            candidate_count=1, analytical_iterations=0, refinement_passes=0),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_feedback_trials=2,
    )
    assert pipeline.detailed_feedback_trials == 0
    assert pipeline.detailed.metrics.unrouted_net_count == 1
