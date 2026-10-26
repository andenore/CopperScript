from __future__ import annotations

from dataclasses import replace

import pcbir.flow as flow_module

from pcbir import (
    BoardOutline,
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
        plane_stitch_options=PlaneStitchOptions(),
        detailed_feedback_trials=2,
    )

    assert result.plane_stitch is not None
    assert result.plane_stitch.added_via_count >= 1
    assert result.detailed.locked_via_count == result.plane_stitch.added_via_count
    assert result.detailed.metrics.routed_net_count == 1
    assert result.detailed_feedback_trials == 0
    assert result.board.vias[:result.detailed.locked_via_count] == result.plane_stitch.board.vias


def test_direct_unified_routes_clock_without_legacy_bootstrap(monkeypatch):
    from pcbir.physical import NetRoutingRule, RouteKind, TrackSegment
    from pcbir.unified_router import UnifiedRouterOptions
    footprint = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))
    board = PhysicalBoard("Direct", BoardOutline.rectangle(20, 12), {"one": footprint},
        (Placement("J1", "one", Point.mm(3, 6)), Placement("J2", "one", Point.mm(17, 6))),
        (PhysicalNet("CLK", (PadReference("J1", "1"), PadReference("J2", "1"))),),
        net_routing_rules=(NetRoutingRule("CLK", kind=RouteKind.CLOCK, max_vias=0,
            max_length_nm=nm_from_mm(18), allowed_layers=(CopperLayer.FRONT,)),))
    def forbidden(*args, **kwargs):
        raise AssertionError("direct unified routing invoked a legacy solver")
    for name in ("route_critical_nets", "route_detailed", "_plan_decouplers", "_plan_ordinary_escapes",
                 "improve_critical_placement"):
        monkeypatch.setattr(flow_module, name, forbidden)
    result = run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0,
            fixed_references=("J1", "J2")),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        unified_options=UnifiedRouterOptions(direct=True, time_budget_s=15), critical_feedback_trials=1)
    assert result.board.metadata["detailed_router"] == "unified"
    assert result.status is PhysicalFlowStatus.PASS, (result.critical.to_json(), result.drc.findings, result.detailed.nets)
    assert result.critical.nets[0].strategy == "unified_final_review"
    assert result.critical.nets[0].connected


def test_direct_unified_does_not_certify_independent_differential_paths():
    from types import SimpleNamespace
    from pcbir.physical import NetRoutingRule, RouteKind, TrackSegment
    footprint = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))
    poses, nets, tracks, rules = [], [], [], []
    for name, other, y in (("P", "N", 5), ("N", "P", 7)):
        poses += [Placement(name+"1", "one", Point.mm(3, y)), Placement(name+"2", "one", Point.mm(17, y))]
        nets.append(PhysicalNet(name, (PadReference(name+"1", "1"), PadReference(name+"2", "1"))))
        tracks.append(TrackSegment(name, Point.mm(3, y), Point.mm(17, y), nm_from_mm("0.2"), CopperLayer.FRONT))
        rules.append(NetRoutingRule(name, kind=RouteKind.DIFFERENTIAL, differential_partner=other,
            pair_gap_nm=nm_from_mm("0.2"), max_skew_nm=nm_from_mm("0.5")))
    board = PhysicalBoard("Pair", BoardOutline.rectangle(20, 12), {"one": footprint}, tuple(poses), tuple(nets),
        tracks=tuple(tracks), net_routing_rules=tuple(rules))
    drc = flow_module.run_physical_drc(board)
    review = flow_module._unified_profile_review(SimpleNamespace(board=board, routing_fingerprint="final"),
        SimpleNamespace(routing_fingerprint="global"), drc)
    assert review.status.value == "failed"
    assert review.nets[0].nets == ("N", "P")
    assert not review.nets[0].connected
    assert any("coupling and return-path" in item for item in review.nets[0].diagnostics)


def test_direct_unified_keeps_immutable_macro_copper(tmp_path):
    from test_hard_macros import fixture
    from pcbir.hard_macros import materialize_hard_macros
    from pcbir.unified_router import UnifiedRouterOptions
    _, _, bind = fixture(tmp_path)
    source = bind()
    fixed = materialize_hard_macros(source)
    result = run_routing_pipeline(source,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0,
            fixed_references=("U1", "R1")),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        unified_options=UnifiedRouterOptions(direct=True, time_budget_s=15))
    assert result.board.materialized_macros == fixed.materialized_macros
    assert all(track in result.board.tracks for track in fixed.tracks)
    assert all(via in result.board.vias for via in fixed.vias)


def test_direct_unified_does_not_seed_failed_nets_with_legacy_fanout(monkeypatch):
    import pcbir.fanout as fanout_module
    from pcbir.fanout import FanoutOptions
    from pcbir.physical import NetRoutingRule, RouteKind
    from pcbir.unified_router import UnifiedRouterOptions
    footprint = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))
    wall = CopperKeepout("wall", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(9, 0), Point.mm(11, 0), Point.mm(11, 12), Point.mm(9, 12)))))
    board = PhysicalBoard("NoFallback", BoardOutline.rectangle(20, 12), {"one": footprint},
        (Placement("J1", "one", Point.mm(3, 6)), Placement("J2", "one", Point.mm(17, 6))),
        (PhysicalNet("CLK", (PadReference("J1", "1"), PadReference("J2", "1"))),),
        copper_keepouts=(wall,), net_routing_rules=(NetRoutingRule("CLK", kind=RouteKind.CLOCK,
            max_vias=0, allowed_layers=(CopperLayer.FRONT,)),))
    def forbidden(*args, **kwargs):
        raise AssertionError("direct unified routing used legacy fanout as a fallback")
    monkeypatch.setattr(fanout_module, "route_fanout", forbidden)
    result = run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0,
            fixed_references=("J1", "J2")),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        fanout_options=FanoutOptions(),
        unified_options=UnifiedRouterOptions(direct=True, time_budget_s=10, maximum_iterations=2))
    assert result.board.metadata["detailed_router"] == "unified"
    assert result.status is PhysicalFlowStatus.FAIL
    assert not result.detailed.nets[0].connected


def test_standard_unified_pipeline_keeps_coupled_pair_and_routes_other_nets():
    from test_critical_routing import _pair_board
    from pcbir.route_summary import connection_summary
    from pcbir.unified_router import UnifiedRouterOptions
    source = _pair_board()
    one = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))
    board = replace(source, footprints={**source.footprints, "one": one},
        placements=(*source.placements, Placement("S1", "one", Point.mm(10, 4)),
                    Placement("S2", "one", Point.mm(30, 20))),
        nets=(*source.nets, PhysicalNet("SIGNAL", (PadReference("S1", "1"), PadReference("S2", "1")))))
    result = run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0,
            fixed_references=tuple(p.reference for p in board.placements)),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        unified_options=UnifiedRouterOptions(maximum_iterations=20, time_budget_s=20))
    pair = next(item for item in result.critical.nets if len(item.nets) == 2)
    assert pair.connected and pair.coupled_length_nm > 0
    assert pair.strategy != "unified_final_review"
    assert result.board.metadata["detailed_router"] == "unified"
    assert next(n for n in result.detailed.nets if n.net == "SIGNAL").connected
    assert set(result.critical.locked_tracks) <= set(result.board.tracks)
    summary = connection_summary(result.board, result.drc)
    assert summary["signal"]["opens"] == 0
    assert summary["drc"]["errors"] == 0
