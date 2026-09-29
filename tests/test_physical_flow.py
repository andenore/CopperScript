from __future__ import annotations

from dataclasses import replace

import pcbir.flow as flow_module
from pcbir.routeflow import FeedbackStatus, PlacementRoutingResult
from pcbir.routing import GlobalRoutingStatus, route_global

from pcbir import (
    BoardOutline,
    DetailedRouterOptions,
    FootprintPad,
    GlobalRouterOptions,
    PadReference,
    PhysicalBoard,
    PhysicalFlowStatus,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    PlacementPlannerOptions,
    PlacementRoutingFeedbackOptions,
    Point,
    Size,
    nm_from_mm,
    run_routing_pipeline,
    CopperKeepout,
    CopperLayer,
    CopperZone,
    PlaneStitchOptions,
    PolygonRing,
    PolygonWithHoles,
    Stackup,
)


def test_steps_four_through_eight_share_one_fail_closed_pipeline() -> None:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    board = PhysicalBoard(
        "Pipeline",
        BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(3, 6)),
            Placement("J2", footprint.name, Point.mm(17, 6)),
        ),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
    )

    result = run_routing_pipeline(
        board,
        placement_options=PlacementPlannerOptions(
            candidate_count=1, analytical_iterations=0, refinement_passes=0
        ),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm("2")),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm("0.5")),
    )

    assert result.status is PhysicalFlowStatus.PASS
    assert result.placement_and_global.full_route_certified
    assert result.board.metadata["detailed_routing"] == "complete"
    assert result.drc.token.board_digest


def test_plane_pad_escapes_are_reserved_before_detailed_signals() -> None:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    board = PhysicalBoard(
        "EarlyPlane", BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("G1", footprint.name, Point.mm(3, 3)),
            Placement("G2", footprint.name, Point.mm(17, 3)),
            Placement("S1", footprint.name, Point.mm(3, 9)),
            Placement("S2", footprint.name, Point.mm(17, 9)),
        ),
        (
            PhysicalNet("GND", (PadReference("G1", "1"), PadReference("G2", "1"))),
            PhysicalNet("SIGNAL", (PadReference("S1", "1"), PadReference("S2", "1"))),
        ),
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )),
        zones=(CopperZone(
            "plane", "GND", (CopperLayer.INTERNAL_1,),
            PolygonWithHoles(PolygonRing((
                Point.mm(1, 1), Point.mm(19, 1),
                Point.mm(19, 11), Point.mm(1, 11),
            ))),
        ),),
    )
    result = run_routing_pipeline(
        board,
        placement_options=PlacementPlannerOptions(
            candidate_count=1, analytical_iterations=0, refinement_passes=0,
        ),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_options=DetailedRouterOptions(
            pitch_nm=nm_from_mm(1), maximum_passes=1, defer_zone_nets=True,
        ),
        plane_stitch_options=PlaneStitchOptions(),
        detailed_feedback_trials=2,
    )

    assert result.plane_stitch is not None
    assert result.plane_stitch.added_via_count >= 1
    assert result.detailed.locked_via_count == result.plane_stitch.added_via_count
    assert result.detailed.metrics.routed_net_count == 1
    assert result.detailed_feedback_trials == 0
    assert result.board.vias[:result.detailed.locked_via_count] == result.plane_stitch.board.vias


def test_detailed_failure_can_trigger_legal_placement_retry(monkeypatch) -> None:
    footprint = PhysicalFootprint(
        "test/one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    keepout = CopperKeepout(
        "pin-trap", (CopperLayer.FRONT, CopperLayer.BACK),
        PolygonWithHoles(PolygonRing((
            Point.mm(2, 5), Point.mm(4, 5),
            Point.mm(4, 7), Point.mm(2, 7),
        ))),
    )
    board = PhysicalBoard(
        "Feedback", BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (Placement("J1", footprint.name, Point.mm(3, 6)),
         Placement("J2", footprint.name, Point.mm(17, 6))),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
        copper_keepouts=(keepout,),
    )
    options = PlacementPlannerOptions(
        candidate_count=1, analytical_iterations=0, refinement_passes=0,
        fixed_references=("J2",),
    )
    common = dict(
        placement_options=options,
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm("2")),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"),
                                               maximum_passes=1),
    )
    # Seed a deliberately stale guide to exercise detailed-placement repair.
    # The corrected global access model now rejects the trapped pin outright.
    assert route_global(board, common["global_options"]).status is GlobalRoutingStatus.UNREACHABLE
    guide = route_global(replace(board, copper_keepouts=()), common["global_options"])
    assert guide.status is GlobalRoutingStatus.SUCCESS
    monkeypatch.setattr(flow_module, "optimize_placement_for_routing", lambda *_: (
        PlacementRoutingResult(
            FeedbackStatus.PASS, board, guide, "seeded", (), 0, True,
        )
    ))
    initial = run_routing_pipeline(board, **common)
    retried = run_routing_pipeline(
        board, **common, detailed_feedback_trials=4,
        detailed_feedback_movement_nm=nm_from_mm("3"),
    )
    assert initial.detailed.metrics.unrouted_net_count == 1
    assert retried.detailed_feedback_trials > 0
    assert retried.detailed.metrics.unrouted_net_count == 0
    assert retried.placement_and_global.board.placements[1] == board.placements[1]
