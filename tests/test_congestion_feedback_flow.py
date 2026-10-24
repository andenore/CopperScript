"""Congestion-driven placement feedback wired into the unified pipeline (stage B)."""
from __future__ import annotations

from dataclasses import replace
from math import hypot

import pytest

import pcbir.flow as flow
from pcbir import (
    BoardOutline,
    CopperKeepout,
    CopperLayer,
    DesignRules,
    FootprintPad,
    GlobalRouterOptions,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    PlacementPlannerOptions,
    PlacementRoutingFeedbackOptions,
    Point,
    PolygonRing,
    PolygonWithHoles,
    Size,
    nm_from_mm,
    route_global,
    run_routing_pipeline,
)
from pcbir.physical import NetRoutingRule
from pcbir.routeflow import FeedbackStatus, PlacementRoutingResult
from pcbir.unified_router import UnifiedRouterOptions

GLOBAL = GlobalRouterOptions(tile_size_nm=nm_from_mm("2"))


def _box(name, x0, y0, x1, y1):
    ring = PolygonRing((Point.mm(x0, y0), Point.mm(x1, y0), Point.mm(x1, y1), Point.mm(x0, y1)))
    return CopperKeepout(name, (CopperLayer.FRONT,), PolygonWithHoles(ring))


def _boxed_in_board() -> PhysicalBoard:
    """SIG's pad S1 sits in a front-copper pocket whose only exit, a 0.8 mm
    channel, is plugged by the pad of the movable part B (on no net). SIG may
    only use F.Cu, so it stays open until B moves out of the channel mouth."""
    footprint = PhysicalFootprint("test/one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
                                  Size.mm(1, 1))
    keepouts = (
        _box("pocket-west", "3.4", "5.4", "4.2", "8.6"),
        _box("pocket-north", "3.4", "7.8", "5.8", "8.6"),
        _box("pocket-south", "3.4", "5.4", "5.8", "6.2"),
        _box("channel-north", "5.8", "7.4", "8.0", "8.6"),
        _box("channel-south", "5.8", "5.4", "8.0", "6.6"),
    )
    return PhysicalBoard(
        "CongestionFeedback", BoardOutline.rectangle(24, 14), {footprint.name: footprint},
        (Placement("S1", footprint.name, Point.mm(5, 7)),
         Placement("S2", footprint.name, Point.mm(19, 7)),
         Placement("B", footprint.name, Point.mm(8, 7))),
        (PhysicalNet("SIG", (PadReference("S1", "1"), PadReference("S2", "1"))),),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.15"), minimum_track_width_nm=nm_from_mm("0.15"),
                          default_track_width_nm=nm_from_mm("0.2"), default_via_size_nm=nm_from_mm("0.6"),
                          default_via_drill_nm=nm_from_mm("0.3"), minimum_hole_clearance_nm=nm_from_mm("0.25")),
        copper_keepouts=keepouts,
        net_routing_rules=(NetRoutingRule("SIG", allowed_layers=(CopperLayer.FRONT,)),),
    )


def _run(board, **kwargs):
    # The supplied placement is the incumbent: no placer run, S1/S2 locked.
    placement = PlacementRoutingResult(FeedbackStatus.PASS, board, route_global(board, GLOBAL),
                                       "fixture", (), 0, True)
    kwargs.setdefault("unified_options", UnifiedRouterOptions(time_budget_s=30))
    return run_routing_pipeline(
        board, initial_placement=placement, global_options=GLOBAL,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
                                                  refinement_passes=0, fixed_references=("S1", "S2")),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_feedback_movement_nm=nm_from_mm(1), **kwargs)


def _sig_connected(result) -> bool:
    return next(item.connected for item in result.detailed.nets if item.net == "SIG")


def _pose(result, reference):
    return next(p for p in result.placement_and_global.board.placements if p.reference == reference)


def test_loop_moves_the_blocking_part_and_connects_the_net():
    board = _boxed_in_board()
    baseline = _run(board)
    assert not _sig_connected(baseline)
    assert baseline.placement_feedback is None   # the loop is off by default

    result = _run(board, placement_feedback_trials=4)
    feedback = result.placement_feedback
    assert feedback is not None and feedback.accepted_moves >= 1, feedback.report()
    assert _sig_connected(result)
    assert feedback.final_score < feedback.initial_score
    assert feedback.initial_failed_nets == ("SIG",) and feedback.final_failed_nets == ()
    assert result.placement_and_global.placement_candidate.endswith("-congestion")
    accepted = [trial for trial in feedback.trials if trial.outcome == "accepted"]
    assert [trial.reference for trial in accepted] == ["B"] * len(accepted)
    assert feedback.reruns <= 4 and len(feedback.trials) >= feedback.reruns
    # B moved away from the tile that made it a candidate.
    reason = accepted[0].reason
    assert reason.startswith("hot_tile:")
    ix, iy = (int(v) for v in reason.split(";")[0].split(":")[1].split(","))
    x0, y0, x1, y1 = feedback.initial_congestion.tile_bounds(ix, iy)
    centre = ((x0 + x1) / 2, (y0 + y1) / 2)
    before, after = _pose(baseline, "B").position, _pose(result, "B").position
    assert hypot(after.x_nm - centre[0], after.y_nm - centre[1]) > hypot(before.x_nm - centre[0],
                                                                          before.y_nm - centre[1])
    # Locked parts never move; the routed board is the accepted placement's.
    assert _pose(result, "S1") == _pose(baseline, "S1") and _pose(result, "S2") == _pose(baseline, "S2")
    assert {p.reference: p for p in result.board.placements}["B"] == _pose(result, "B")
    report = feedback.report()
    assert report["accepted_moves"] == feedback.accepted_moves
    assert len(report["hot_tiles"]) <= 16 and all(len(t["bounds_mm"]) == 4 for t in report["hot_tiles"])
    assert report["trials"][0]["reference"] == "B"


def test_loop_with_no_failures_records_an_idle_outcome():
    free = _run(replace(_boxed_in_board(), copper_keepouts=()), placement_feedback_trials=4)
    assert _sig_connected(free)
    assert free.placement_feedback.stop_reason == "no_failed_nets"
    assert free.placement_feedback.trials == () and free.placement_feedback.accepted_moves == 0


def test_legacy_router_keeps_the_critical_controller_and_unified_maps_it(monkeypatch):
    board = _boxed_in_board()
    calls = []
    original = flow.improve_critical_placement

    def spy(*args, **kwargs):
        calls.append(kwargs.get("maximum_trials"))
        return original(*args, **kwargs)

    monkeypatch.setattr(flow, "improve_critical_placement", spy)
    legacy = _run(board, unified_options=None, critical_feedback_trials=1)
    assert calls == [1]
    assert legacy.placement_feedback is None and legacy.critical_feedback is not None

    calls.clear()
    unified = _run(board, critical_feedback_trials=1)
    assert calls == []
    assert unified.critical_feedback is None
    assert unified.placement_feedback is not None
    assert unified.placement_feedback.reruns <= 1


def test_negative_budget_is_rejected():
    with pytest.raises(ValueError):
        _run(_boxed_in_board(), placement_feedback_trials=-1)


def test_maps_are_normalised_to_their_own_95th_percentile_before_adding():
    import numpy as np
    from pcbir.congestion_feedback import CongestionMap, combine_normalised, feedback_map

    capacity = np.full((2, 2), 10.0)
    router = CongestionMap(tile_nm=nm_from_mm(1), x0_nm=0, y0_nm=0, nx=2, ny=2,
                           demand=np.array([[0.0, 1.0], [1.0, 1.0]]), capacity=capacity, source="unified_router")
    estimate = CongestionMap(tile_nm=nm_from_mm(1), x0_nm=0, y0_nm=0, nx=2, ny=2,
                             demand=np.array([[500.0, 0.0], [500.0, 0.0]]), capacity=np.full((2, 2), 1.0),
                             source="rudy")
    combined = combine_normalised(router, estimate)
    assert combined.source == "combined" and np.array_equal(combined.capacity, capacity)
    # Each map's positive tiles sit at its own 95th percentile, so both count 1.
    assert combined.utilisation().tolist() == [[1.0, 1.0], [2.0, 1.0]]
    # An all-zero router map leaves the estimate's shape; no router map: estimate alone.
    quiet = combine_normalised(CongestionMap(tile_nm=nm_from_mm(1), x0_nm=0, y0_nm=0, nx=2, ny=2,
                                             demand=np.zeros((2, 2)), capacity=capacity, source="unified_router"),
                               estimate)
    assert quiet.utilisation().tolist() == [[1.0, 0.0], [1.0, 0.0]]
    board = _boxed_in_board()
    assert feedback_map(board, None).source == "rudy"
    on_router = feedback_map(board, CongestionMap(tile_nm=nm_from_mm(1), x0_nm=-nm_from_mm(1), y0_nm=0, nx=26,
                                                   ny=14, demand=np.zeros((14, 26)), capacity=np.ones((14, 26)),
                                                   source="unified_router"))
    assert (on_router.x0_nm, on_router.nx, on_router.ny) == (-nm_from_mm(1), 26, 14)


def test_loop_halves_the_step_and_stops_after_two_rounds_without_improvement():
    from pcbir.congestion_feedback import FeedbackState, run_placement_feedback

    board = _boxed_in_board()
    baseline = _run(board)
    state = FeedbackState(board, baseline.placement_and_global.global_route, baseline.critical,
                          baseline.detailed, baseline.drc)
    calls = []

    def rerun(trial_board, global_route):
        calls.append(trial_board)
        return state   # never better

    outcome = run_placement_feedback(
        state, trials=10, movement_nm=nm_from_mm(1),
        placement_options=PlacementPlannerOptions(fixed_references=("S1", "S2")),
        global_options=GLOBAL, rerun=rerun, score=flow._feedback_score)
    assert outcome.stop_reason == "no_improvement" and outcome.rounds == 2
    assert outcome.accepted_moves == 0 and outcome.state is state
    assert [t.step_nm for t in outcome.trials] == [nm_from_mm(1)] * 2 + [nm_from_mm("0.5")] * 2
    # A proposal whose global route fails is recorded but costs no rerun.
    assert {t.outcome for t in outcome.trials} <= {"not_improved", "global_failed"}
    assert len(calls) == outcome.reruns == sum(t.outcome == "not_improved" for t in outcome.trials) >= 1
    # Every pose is tried once.
    poses = [(t.reference, t.delta_nm, t.rotation_degrees) for t in outcome.trials]
    assert len(set(poses)) == len(poses)
    calls.clear()
    budget = run_placement_feedback(
        state, trials=3, movement_nm=nm_from_mm(1),
        placement_options=PlacementPlannerOptions(fixed_references=("S1", "S2")),
        global_options=GLOBAL, rerun=rerun, score=flow._feedback_score)
    assert budget.stop_reason == "trial_budget" and budget.reruns == len(calls) == 3
