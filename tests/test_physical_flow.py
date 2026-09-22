from __future__ import annotations

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
