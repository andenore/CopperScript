from dataclasses import replace

import pytest

from pcbir import (BoardOutline, ComponentPlacementRule, CopperLayer, FootprintPad,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, Placement, Point,
    PhysicalNet, RouteKind, Size, TrackSegment, PlacementPlannerOptions, GlobalRouterOptions,
    RigidPlacementCluster, RigidPlacementMember, PlacementTarget, nm_from_mm,
    route_global, route_critical_nets)
from pcbir.critical import _fingerprint, CriticalRoutingStatus
from pcbir.critical_feedback import improve_critical_placement, critical_placement_trials
from pcbir.clusters import footprint_geometry_digest
from pcbir.routing import GlobalRoutingStatus


def rebind(result, *, nets=None, tracks=None):
    nets = result.nets if nets is None else nets
    tracks = result.locked_tracks if tracks is None else tracks
    return replace(result, board=replace(result.board, tracks=tracks), nets=nets, locked_tracks=tracks,
        status=CriticalRoutingStatus.FAILED if any(not net.connected for net in nets) else CriticalRoutingStatus.SUCCESS,
        routing_fingerprint=_fingerprint(result.global_routing_fingerprint, list(tracks), list(result.locked_vias), list(nets)))


def fixture():
    fp = PhysicalFootprint("offset", (FootprintPad("1", Point.mm(1, 0), Size.mm(.4, .4)),), Size.mm(3, 3))
    board = PhysicalBoard("CriticalFeedback", BoardOutline.rectangle(30, 28), {fp.name: fp},
        tuple(Placement(ref, fp.name, Point.mm(x, y)) for ref, x, y in
              (("A", 5, 8), ("B", 23, 8), ("C", 5, 20), ("D", 23, 20))),
        (PhysicalNet("TARGET", (PadReference("A", "1"), PadReference("B", "1"))),
         PhysicalNet("KEPT", (PadReference("C", "1"), PadReference("D", "1")))),
        net_routing_rules=(NetRoutingRule("KEPT", RouteKind.CRITICAL, priority=2, max_vias=0),
                           NetRoutingRule("TARGET", RouteKind.CRITICAL, priority=1, max_vias=0)),
        placement_rules=(ComponentPlacementRule("A", allowed_orientations=(0, 180)),
                         ComponentPlacementRule("B", fixed_position=Point.mm(23, 8), fixed_rotation_degrees=0)))
    options = GlobalRouterOptions(tile_size_nm=nm_from_mm(2), maximum_iterations=2)
    guides = route_global(board, options)
    complete = route_critical_nets(board, guides)
    assert all(net.connected for net in complete.nets)
    # Model an exhausted TARGET search, with no TARGET copper ever accepted.
    baseline = rebind(complete, nets=tuple(replace(net, connected=False, track_count=0,
                        diagnostics=("bounded fixture failure",)) if net.nets == ("TARGET",) else net
                        for net in complete.nets),
                      tracks=tuple(track for track in complete.locked_tracks if track.net != "TARGET"))
    return board, guides, baseline, options


def test_rotation_trial_repairs_failure_and_rebuilds_all_critical_copper():
    board, guides, baseline, options = fixture()
    records = []
    starts = []
    result = improve_critical_placement(board, guides, baseline, maximum_trials=1,
                                       global_options=options, on_trial=records.append,
                                       on_trial_started=lambda index, ref, trial: starts.append((index, ref, trial)))
    assert result.accepted_moves == 1 and len(result.trials) == 1
    assert result.trials[0].reference == "A" and result.trials[0].rotation_degrees == "180"
    assert result.trials[0].changed_references == ("A",)
    assert all(net.connected for net in result.critical.nets)
    assert result.board.tracks == ()
    assert result.board.placements[1:] == board.placements[1:]
    assert records == list(result.trials)
    assert starts == [(1, "A", result.board)]
    assert improve_critical_placement(board, guides, baseline, maximum_trials=1,
                                     global_options=options).critical == result.critical


@pytest.mark.parametrize("defect", ["previous_open", "actual_open_keep_flags", "short", "missing_result"])
def test_failed_trial_preserves_incumbent_and_checks_fresh_geometry(monkeypatch, defect):
    board, guides, baseline, options = fixture()
    def unsafe(trial, global_route, **kwargs):
        result = route_critical_nets(trial, global_route)
        if defect == "previous_open":
            return rebind(result, nets=tuple(replace(net, connected=False) if net.nets == ("KEPT",) else net
                                             for net in result.nets),
                          tracks=tuple(track for track in result.locked_tracks if track.net != "KEPT"))
        if defect == "actual_open_keep_flags":
            return rebind(result, tracks=tuple(track for track in result.locked_tracks if track.net != "KEPT"))
        if defect == "missing_result":
            return rebind(result, nets=tuple(net for net in result.nets if net.nets != ("KEPT",)))
        target = next(track for track in result.locked_tracks if track.net == "TARGET")
        return rebind(result, tracks=(*result.locked_tracks, replace(target, net="KEPT")))
    monkeypatch.setattr("pcbir.critical_feedback.route_critical_nets", unsafe)
    result = improve_critical_placement(board, guides, baseline, maximum_trials=2, global_options=options)
    assert result.accepted_moves == 0 and len(result.trials) == 2
    assert result.board == board and result.critical == baseline and result.global_route == guides
    assert all(trial.outcome != "accepted" for trial in result.trials)


def test_global_failure_is_not_credited_as_critical_improvement(monkeypatch):
    board, guides, baseline, options = fixture()
    monkeypatch.setattr("pcbir.critical_feedback.route_global", lambda trial, options:
                        replace(route_global(trial, options), status=GlobalRoutingStatus.UNREACHABLE))
    result = improve_critical_placement(board, guides, baseline, maximum_trials=1, global_options=options)
    assert result.trials[0].outcome == "global_failed" and result.critical == baseline


def test_fixed_components_and_orientation_restrictions_are_not_broadened():
    board, guides, baseline, options = fixture()
    assert not tuple(critical_placement_trials(board, {"TARGET"},
        PlacementPlannerOptions(fixed_references=("A",)), nm_from_mm(.5)))
    restricted = replace(board, placement_rules=(replace(board.placement_rules[0], allowed_orientations=(0,)),
                                                 board.placement_rules[1]))
    trials = tuple(critical_placement_trials(restricted, {"TARGET"}, PlacementPlannerOptions(), nm_from_mm(.5)))
    assert trials and all(trial.placements[0].rotation_degrees == 0 for _, trial in trials)


def test_rigid_member_trial_moves_entire_unit_and_cannot_displace_fixed_companion():
    board, _, _, _ = fixture()
    fp = board.footprints["offset"]
    companion = Placement("E", fp.name, Point.mm(5, 12))
    digest = footprint_geometry_digest(fp)
    cluster = RigidPlacementCluster("unit", PlacementTarget("A"),
        (RigidPlacementMember("A", fp.name, digest, Point(0, 0)),
         RigidPlacementMember("E", fp.name, digest, Point.mm(0, 4))), "synthetic fixture", (0, 180))
    board = replace(board, placements=(*board.placements, companion), rigid_clusters=(cluster,))
    trials = tuple(critical_placement_trials(board, {"TARGET"}, PlacementPlannerOptions(), nm_from_mm(.5)))
    assert trials and trials[0][1].placements[-1].position == Point.mm(5, 4)
    fixed = replace(board, placement_rules=(*board.placement_rules, ComponentPlacementRule("E", fixed_position=companion.position)))
    assert not tuple(critical_placement_trials(fixed, {"TARGET"}, PlacementPlannerOptions(), nm_from_mm(.5)))


@pytest.mark.parametrize("defect", ["source_copper", "moved_source", "changed_result", "negative_budget"])
def test_stale_or_invalid_baseline_rejected(defect):
    board, guides, baseline, options = fixture()
    trials = 1
    if defect == "source_copper":
        board = replace(board, tracks=baseline.locked_tracks)
    elif defect == "moved_source":
        board = replace(board, placements=(replace(board.placements[0], position=Point.mm(6, 8)), *board.placements[1:]))
    elif defect == "changed_result":
        baseline = replace(baseline, routing_fingerprint="wrong")
    else:
        trials = -1
    with pytest.raises(ValueError):
        improve_critical_placement(board, guides, baseline, maximum_trials=trials, global_options=options)


def test_disabled_feedback_returns_exact_baseline():
    board, guides, baseline, options = fixture()
    result = improve_critical_placement(board, guides, baseline, global_options=options)
    assert result.board == board and result.critical == baseline and result.trials == ()


def test_pipeline_routes_ordinary_copper_from_the_accepted_critical_pose(monkeypatch):
    from pcbir.flow import run_routing_pipeline
    from pcbir.routeflow import FeedbackStatus, PlacementRoutingResult
    from pcbir.detailed import DetailedRouterOptions

    board, guides, baseline, options = fixture()
    monkeypatch.setattr("pcbir.flow.optimize_placement_for_routing", lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, "fixture", (), 0, True))
    monkeypatch.setattr("pcbir.flow.route_critical_nets", lambda *_, **__: baseline)
    result = run_routing_pipeline(board, global_options=options, critical_feedback_trials=1,
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm(1), maximum_passes=1))
    assert result.critical_feedback.accepted_moves == 1
    assert result.board.placements == result.critical_feedback.board.placements
    assert result.placement_and_global.board == result.critical_feedback.board
    assert result.critical.global_routing_fingerprint == result.placement_and_global.global_route.routing_fingerprint
    assert result.detailed.global_routing_fingerprint == result.critical.global_routing_fingerprint
    assert result.detailed.board.tracks[:len(result.critical.locked_tracks)] == result.critical.locked_tracks


def test_45_degree_rule_is_available_to_feedback_without_broadening_other_rules():
    board, _, _, _ = fixture()
    board = replace(board, placement_rules=(replace(board.placement_rules[0], allowed_orientations=(0, 45)),
                                            board.placement_rules[1]))
    trials = tuple(critical_placement_trials(board, {"TARGET"}, PlacementPlannerOptions(), nm_from_mm(.5)))
    assert trials[0][1].placements[0].rotation_degrees == 45
