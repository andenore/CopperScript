from __future__ import annotations

from dataclasses import replace

from pcbir import (
    BoardOutline, ComponentPlacementRule, CopperKeepout, CopperLayer,
    CopperZone, DetailedRouterOptions, EscapeFeedbackOptions, FootprintPad,
    GlobalRouterOptions, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, PlacementPlannerOptions,
    PlacementRoutingFeedbackOptions, PlaneStitchOptions, Point, PolygonRing,
    PolygonWithHoles, Size, Stackup, improve_zone_escapes, nm_from_mm,
    run_routing_pipeline, stitch_zone_pads,
)
from pcbir.escape_feedback import _candidate_placements


def _trapped_ground_board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    zone = CopperZone(
        "ground", "GND", (CopperLayer.INTERNAL_1,),
        PolygonWithHoles(PolygonRing((
            Point.mm(1, 1), Point.mm(19, 1),
            Point.mm(19, 11), Point.mm(1, 11),
        ))),
    )
    via_block = CopperKeepout(
        "via-block", tuple(Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )).copper_layers),
        PolygonWithHoles(PolygonRing((
            Point.mm(3, 4), Point.mm("6.3", 4),
            Point.mm("6.3", 8), Point.mm(3, 8),
        ))),
        block_tracks=False, block_zones=False,
    )
    return PhysicalBoard(
        "EscapeFeedback", BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("G1", footprint.name, Point.mm(5, 6)),
            Placement("G2", footprint.name, Point.mm(17, 6)),
        ),
        (PhysicalNet("GND", (
            PadReference("G1", "1"), PadReference("G2", "1"),
        )),),
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )),
        zones=(zone,), copper_keepouts=(via_block,),
        placement_rules=(ComponentPlacementRule(
            "G1", allowed_orientations=(0, 45, 90),
        ),),
    )


def test_feedback_moves_only_from_unrouted_board_to_open_ground_exit() -> None:
    board = _trapped_ground_board()
    fixed = PlacementPlannerOptions(
        candidate_count=1, analytical_iterations=0, refinement_passes=0,
        fixed_references=frozenset({"G1", "G2"}),
    )
    initial = run_routing_pipeline(
        board,
        placement_options=fixed,
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_options=DetailedRouterOptions(
            pitch_nm=nm_from_mm(1), maximum_passes=1,
        ),
    )
    result = improve_zone_escapes(
        initial, PlaneStitchOptions(maximum_radius_nm=nm_from_mm(1)),
        placement_options=PlacementPlannerOptions(
            candidate_count=1, analytical_iterations=0, refinement_passes=0,
        ),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        detailed_options=DetailedRouterOptions(
            pitch_nm=nm_from_mm(1), maximum_passes=1,
        ),
        options=EscapeFeedbackOptions(
            maximum_trials=1, movement_nm=nm_from_mm(1), nearby_components=0,
        ),
    )

    assert result.plane_stitch.complete
    assert any(attempt.accepted for attempt in result.attempts)
    assert any(attempt.decision.startswith("accepted:") for attempt in result.attempts)
    assert result.pipeline.placement_and_global.board.placements[0].position != Point.mm(5, 6)
    assert board.placements[0].position == Point.mm(5, 6)


def test_escape_feedback_considers_explicit_45_degree_rotation() -> None:
    board = _trapped_ground_board()
    candidates = _candidate_placements(
        board, frozenset({PadReference("G1", "1")}),
        PlacementPlannerOptions(), EscapeFeedbackOptions(maximum_trials=1),
    )
    assert any(
        candidate.placements[0].rotation_degrees == 45
        for candidate, _, _ in candidates
    )


def test_escape_feedback_prioritizes_placement_change_before_escape_bias() -> None:
    board = _trapped_ground_board()
    candidates = _candidate_placements(
        board, frozenset({PadReference("G1", "1")}),
        PlacementPlannerOptions(), EscapeFeedbackOptions(maximum_trials=2),
    )
    first = next(candidates)
    second = next(candidates)
    assert first[2] == "current placement / nearest escape"
    assert second[0].placements != board.placements
    assert second[1] is None


def test_feedback_retargets_new_pending_pad_after_accepted_move() -> None:
    board = _trapped_ground_board()
    other_block = CopperKeepout(
        "other-via-block", board.stackup.copper_layers,
        PolygonWithHoles(PolygonRing((
            Point.mm(16, 5), Point.mm(18, 5),
            Point.mm(18, 7), Point.mm(16, 7),
        ))),
        block_tracks=False, block_zones=False,
    )
    board = replace(board, copper_keepouts=(*board.copper_keepouts, other_block))
    fixed = PlacementPlannerOptions(
        candidate_count=1, analytical_iterations=0, refinement_passes=0,
        fixed_references=frozenset({"G1", "G2"}),
    )
    route_options = DetailedRouterOptions(
        pitch_nm=nm_from_mm(1), maximum_passes=1,
    )
    initial = run_routing_pipeline(
        board, placement_options=fixed,
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_options=route_options,
    )
    result = improve_zone_escapes(
        initial, PlaneStitchOptions(maximum_radius_nm=nm_from_mm(1)),
        placement_options=PlacementPlannerOptions(
            candidate_count=1, analytical_iterations=0, refinement_passes=0,
        ),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        detailed_options=route_options,
        options=EscapeFeedbackOptions(
            maximum_trials=2, movement_nm=nm_from_mm(1), nearby_components=0,
        ),
    )

    accepted = [attempt for attempt in result.attempts if attempt.accepted]
    assert len(accepted) == 2
    assert accepted[0].description.startswith("G1 move")
    assert accepted[1].description.startswith("G2 move")
    assert result.plane_stitch.complete


def test_local_plane_escape_rips_up_and_reroutes_successful_signal() -> None:
    footprint = PhysicalFootprint(
        "test/single", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    zone = CopperZone(
        "ground", "GND", (CopperLayer.INTERNAL_1,),
        PolygonWithHoles(PolygonRing((
            Point.mm(1, 1), Point.mm(19, 1),
            Point.mm(19, 11), Point.mm(1, 11),
        ))),
    )
    # The signal's straight front-layer route crosses the only westward
    # ground escape. The signal itself can legally detour around that escape.
    via_keepout = CopperKeepout(
        "right-side-via-block", (
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        ),
        PolygonWithHoles(PolygonRing((
            Point.mm("3.9", 0), Point.mm(19, 0),
            Point.mm(19, 12), Point.mm("3.9", 12),
        ))),
        block_tracks=False, block_zones=False,
    )
    board = PhysicalBoard(
        "LocalRipup", BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("G1", footprint.name, Point.mm(5, 6)),
            Placement("S1", footprint.name, Point.mm(4, 3)),
            Placement("S2", footprint.name, Point.mm(4, 9)),
        ),
        (
            PhysicalNet("GND", (PadReference("G1", "1"),)),
            PhysicalNet("SIGNAL", (
                PadReference("S1", "1"), PadReference("S2", "1"),
            )),
        ),
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )),
        zones=(zone,), copper_keepouts=(via_keepout,),
    )
    fixed = PlacementPlannerOptions(
        candidate_count=1, analytical_iterations=0, refinement_passes=0,
        fixed_references=frozenset({"G1", "S1", "S2"}),
    )
    detail_options = DetailedRouterOptions(
        pitch_nm=nm_from_mm("0.5"), maximum_passes=2,
        maximum_search_states=20_000,
    )
    initial = run_routing_pipeline(
        board, placement_options=fixed,
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_options=detail_options,
    )
    plane_options = PlaneStitchOptions(
        step_nm=nm_from_mm("0.5"), maximum_radius_nm=nm_from_mm(2),
    )
    assert next(item for item in initial.detailed.nets
                if item.net == "SIGNAL").connected
    assert not stitch_zone_pads(initial.board, plane_options).complete
    repaired = improve_zone_escapes(
        initial, plane_options, detailed_options=detail_options,
        options=EscapeFeedbackOptions(maximum_trials=0, maximum_local_trials=5),
    )
    assert repaired.plane_stitch.complete
    assert repaired.attempts[0].description.startswith("local rip-up G1.1: SIGNAL")
    assert next(item for item in repaired.pipeline.detailed.nets
                if item.net == "SIGNAL").connected
    assert repaired.pipeline.board.tracks != initial.board.tracks
