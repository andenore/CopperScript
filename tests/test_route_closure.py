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
from pcbir.route_closure import routing_complete_with_fill


def _deferred_plane_fixture():
    from types import SimpleNamespace
    from pcbir.critical import CriticalRoutingStatus
    from pcbir.flow import PhysicalFlowStatus
    from pcbir.drc import physical_board_digest, run_physical_drc
    from pcbir.plane_verify import PlaneVerification
    from pcbir.backends import KiCadPcbBackend
    from pcbir.backends.kicad_project import kicad_export_digest
    result = _result(reported=True, copper=True)
    footprint = next(iter(result.board.footprints))
    zone = CopperZone("ground", "GND", (CopperLayer.BACK,), PolygonWithHoles(PolygonRing(
        tuple(Point.mm(x,y) for x,y in ((1,1),(9,1),(9,9),(1,9))))))
    board = replace(result.board, placements=(*result.board.placements,
        Placement("G1", footprint, Point.mm(3,3)), Placement("G2", footprint, Point.mm(7,3))),
        nets=(*result.board.nets,PhysicalNet("GND",(PadReference("G1","1"),PadReference("G2","1")))),
        zones=(zone,), metadata={"detailed_routing":"partial"})
    deferred = DetailedNetResult("GND",False,0,0,0,0,("zone net awaits verified fill and pad stitching",))
    result = replace(result,board=board,nets=(*result.nets,deferred),
        metrics=replace(result.metrics,unrouted_net_count=1),status=DetailedRoutingStatus.PARTIAL)
    pipeline = SimpleNamespace(status=PhysicalFlowStatus.FAIL,
        placement_and_global=SimpleNamespace(full_route_certified=True),
        critical=SimpleNamespace(status=CriticalRoutingStatus.SUCCESS),package_access=None,detailed=result)
    drc = run_physical_drc(board)
    evidence = PlaneVerification(True,physical_board_digest(board),
        kicad_export_digest(KiCadPcbBackend().generate(board)),"filled","report","10.0.6",0,0,0,())
    return pipeline,board,drc,evidence


def test_external_fill_can_complete_routing_without_changing_physical_signoff():
    from pcbir.drc import DrcDecision
    pipeline,board,drc,evidence = _deferred_plane_fixture()
    original_token = drc.token
    assert routing_complete_with_fill(pipeline,board,drc,evidence)
    assert drc.decision is DrcDecision.FAIL and drc.token == original_token
    assert not routing_complete_with_fill(pipeline,board,drc,None)


def test_routing_closure_rejects_stale_native_findings_search_failures_and_blocked_preflight():
    from pcbir.drc import DrcFinding,DrcSeverity
    from pcbir.critical import CriticalRoutingStatus
    from types import SimpleNamespace
    pipeline,board,drc,evidence = _deferred_plane_fixture()
    for bad in (replace(evidence,board_digest="stale"),replace(evidence,export_digest="stale"),
                replace(evidence,passed=False),replace(evidence,unconnected_count=1),
                replace(evidence,other_violation_count=1),replace(evidence,findings=("short",))):
        assert not routing_complete_with_fill(pipeline,board,drc,bad)
    pipeline.package_access = SimpleNamespace(ready=False)
    assert not routing_complete_with_fill(pipeline,board,drc,evidence)
    pipeline.package_access = None
    pipeline.critical = SimpleNamespace(status=CriticalRoutingStatus.FAILED)
    assert not routing_complete_with_fill(pipeline,board,drc,evidence)
    pipeline.critical = SimpleNamespace(status=CriticalRoutingStatus.SUCCESS)
    failed = replace(pipeline.detailed.nets[0],connected=False,diagnostics=("search budget exhausted",))
    pipeline.detailed = replace(pipeline.detailed,nets=(failed,*pipeline.detailed.nets[1:]))
    assert not routing_complete_with_fill(pipeline,board,drc,evidence)
    pipeline,board,drc,evidence = _deferred_plane_fixture()
    hard = replace(drc,findings=(*drc.findings,DrcFinding("DRC-CLEARANCE",DrcSeverity.ERROR,"bad")))
    assert not routing_complete_with_fill(pipeline,board,hard,evidence)


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
