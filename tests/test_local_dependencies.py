"""Exact, bounded dependency-cone repair; probe copper is never a committed route."""
from collections import Counter
from dataclasses import replace
from types import SimpleNamespace

import pytest

from pcbir import (BoardOutline, CopperLayer, DetailedRouterOptions, EscapeFeedbackOptions,
                   FootprintPad, GlobalRouterOptions, NetRoutingRule, PadReference, PhysicalBoard, Stackup, Via,
                   PhysicalFootprint, PhysicalNet, Placement, PlacementPlannerOptions,
                   PlacementRoutingFeedbackOptions, Point, Size, TrackSegment,
                   nm_from_mm, run_routing_pipeline)
from pcbir.drc import run_physical_drc
from pcbir.escape_feedback import _merge_local_detail, _reroute_local_dependencies


@pytest.fixture
def corridor():
    footprint = PhysicalFootprint("pad", (FootprintPad("1", Point(0, 0), Size.mm(.4, .4)),), Size.mm(.6, .6))
    poses = tuple(Placement(ref, footprint.name, Point.mm(x, y)) for ref, x, y in (
        ("A1", 10, 3), ("A2", 10, 7), ("B1", 7, 2.5), ("B2", 7, 7.5),
        ("C1", 2.5, 2.5), ("C2", 2.5, 7.5), ("G1", 11.5, 5)))
    nets = tuple(PhysicalNet(name, tuple(PadReference(ref, "1") for ref in refs))
                 for name, refs in (("A", ("A1", "A2")), ("B", ("B1", "B2")),
                                    ("C", ("C1", "C2")), ("GND", ("G1",))))
    board = PhysicalBoard("dependency-corridor", BoardOutline.rectangle(14, 10),
        {footprint.name: footprint}, poses, nets,
        net_routing_rules=tuple(NetRoutingRule(name, allowed_layers=(CopperLayer.FRONT,), max_vias=0)
                                for name in ("A", "B", "C")))
    settings = DetailedRouterOptions(pitch_nm=nm_from_mm(.5), maximum_passes=2,
                                    maximum_search_states=20_000, repair_budget_multiplier=1)
    initial = run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
            refinement_passes=0,
            fixed_references=frozenset(pose.reference for pose in poses)),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(1)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1), detailed_options=settings)
    assert all(item.connected for item in initial.detailed.nets)
    # A legal existing ordinary-net fence reaches close to both board edges.
    # Components themselves retain the standard 2 mm placement-edge margin.
    fence = TrackSegment("B", Point.mm(7, .375), Point.mm(7, 9.625),
                         nm_from_mm(.25), CopperLayer.FRONT)
    initial = _merge_local_detail(initial,
        replace(initial.board, tracks=(*(t for t in initial.board.tracks if t.net != "B"), fence)),
        None, frozenset({"B"}))
    assert not [f for f in initial.drc.findings if f.severity.value == "error"]
    # This new ground reservation traps A against B's vertical route. B can
    # move west only when it is included in the same ordinary-net transaction.
    reservation = TrackSegment("GND", Point.mm(8, 5), Point.mm(13.5, 5),
                               nm_from_mm(.5), CopperLayer.FRONT)
    fixed = replace(initial.critical.board, tracks=(reservation,))
    return initial, fixed, settings


def test_real_secondary_blocker_expands_and_preserves_unrelated_copper(corridor):
    initial, fixed, settings = corridor
    events = []
    before = initial.board
    repaired = _reroute_local_dependencies(initial, fixed, before.tracks, before.vias,
        frozenset({"A"}), frozenset({"A", "B", "C"}), settings,
        EscapeFeedbackOptions(maximum_dependency_expansions=2),
        on_progress=lambda phase, event, details: events.append((phase, event, details)))
    assert repaired is not None
    board, result, changed, expansions = repaired
    assert changed == frozenset({"A", "B"}) and expansions == 1
    assert all(item.connected for item in result.nets)
    assert Counter(t for t in board.tracks if t.net == "C") == Counter(t for t in before.tracks if t.net == "C")
    assert not (Counter(fixed.tracks) - Counter(board.tracks))
    assert not [finding for finding in run_physical_drc(board).findings if finding.severity.value == "error"]
    assert [d["kind"] for _, event, d in events if event == "started"] == ["transaction", "probe_only", "transaction"]
    assert initial.board == before and fixed.tracks == (fixed.tracks[0],)
    repeated = _reroute_local_dependencies(initial, fixed, before.tracks, before.vias,
        frozenset({"A"}), frozenset({"A", "B", "C"}), settings, EscapeFeedbackOptions())
    assert repeated == repaired


@pytest.mark.parametrize("options", [EscapeFeedbackOptions(maximum_dependency_expansions=0),
                                    EscapeFeedbackOptions(maximum_local_blockers=1)])
def test_dependency_disabled_or_net_cap_rolls_back(corridor, options):
    initial, fixed, settings = corridor
    before = initial.board
    assert _reroute_local_dependencies(initial, fixed, before.tracks, before.vias,
        frozenset({"A"}), frozenset({"A", "B", "C"}), settings, options) is None
    assert initial.board == before


def test_locked_secondary_blocker_never_joins_cone(corridor):
    initial, fixed, settings = corridor
    b_tracks = tuple(t for t in initial.board.tracks if t.net == "B")
    fixed = replace(fixed, tracks=(*fixed.tracks, *b_tracks))
    movable = tuple(t for t in initial.board.tracks if t.net != "B")
    assert _reroute_local_dependencies(initial, fixed, movable, initial.board.vias,
        frozenset({"A"}), frozenset({"A", "B", "C"}), settings, EscapeFeedbackOptions()) is None
    assert all(t in fixed.tracks for t in b_tracks)


def test_noneligible_secondary_blocker_cannot_be_displaced(corridor):
    initial, fixed, settings = corridor
    assert _reroute_local_dependencies(initial, fixed, initial.board.tracks, initial.board.vias,
        frozenset({"A"}), frozenset({"A", "C"}), settings, EscapeFeedbackOptions()) is None


@pytest.mark.parametrize("count", [-1, 9])
def test_dependency_expansion_bounds_are_validated(count):
    with pytest.raises(ValueError):
        EscapeFeedbackOptions(maximum_dependency_expansions=count)


@pytest.mark.parametrize("violation", ["unaffected", "fixed-prefix"])
def test_even_connected_subset_cannot_rewrite_unowned_copper(corridor, monkeypatch, violation):
    import pcbir.escape_feedback as feedback
    initial, fixed, settings = corridor
    anchor = TrackSegment("A", Point.mm(10, 3), Point.mm(10, 3.2), nm_from_mm(.25), CopperLayer.FRONT)
    if violation == "fixed-prefix":
        fixed = replace(fixed, tracks=(*fixed.tracks, anchor))
    real = feedback.route_detailed
    def corrupt(source, *args, **kwargs):
        result = real(source, *args, **kwargs)
        tracks = tuple(t for t in result.board.tracks
                       if (t.net != "C" if violation == "unaffected" else t != anchor))
        return replace(result, board=replace(result.board, tracks=tracks))
    monkeypatch.setattr(feedback, "route_detailed", corrupt)
    assert _reroute_local_dependencies(initial, fixed, initial.board.tracks, initial.board.vias,
        frozenset({"A", "B"}), frozenset({"A", "B", "C"}), settings,
        EscapeFeedbackOptions(maximum_dependency_expansions=0)) is None


def test_probe_via_discovers_inner_layer_blocker_but_is_never_committed(corridor, monkeypatch):
    import pcbir.escape_feedback as feedback
    initial, fixed, settings = corridor
    fixed = replace(fixed, stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                                         CopperLayer.INTERNAL_2, CopperLayer.BACK)))
    blocker = TrackSegment("B", Point.mm(5, 5), Point.mm(7, 5), nm_from_mm(.25), CopperLayer.INTERNAL_2)
    via = Via("A", Point.mm(6, 5), nm_from_mm(.8), nm_from_mm(.4))
    calls = []
    def search(source, *args, only_nets, **kwargs):
        calls.append(only_nets)
        probe = len(calls) == 2
        return SimpleNamespace(board=replace(source, vias=(via,) if probe else ()),
            nets=tuple(SimpleNamespace(net=net, connected=probe) for net in sorted(only_nets)),
            metrics=SimpleNamespace(total_conflict_overflow=0))
    monkeypatch.setattr(feedback, "route_detailed", search)
    monkeypatch.setattr(feedback, "close_detailed_lands", lambda result: (result, None, None))
    before = fixed
    assert _reroute_local_dependencies(initial, fixed, (blocker,), (), frozenset({"A"}),
        frozenset({"A", "B"}), settings, EscapeFeedbackOptions(maximum_dependency_expansions=1)) is None
    assert calls == [frozenset({"A"}), frozenset({"A"}), frozenset({"A", "B"})]
    assert fixed == before and not fixed.vias


def test_probe_without_new_blockers_stops_without_repeating_same_search(corridor, monkeypatch):
    import pcbir.escape_feedback as feedback
    initial, fixed, settings = corridor
    calls = []
    def search(source, *args, only_nets, **kwargs):
        calls.append(only_nets)
        return SimpleNamespace(board=source,
            nets=tuple(SimpleNamespace(net=net, connected=len(calls) == 2) for net in sorted(only_nets)),
            metrics=SimpleNamespace(total_conflict_overflow=0))
    monkeypatch.setattr(feedback, "route_detailed", search)
    monkeypatch.setattr(feedback, "close_detailed_lands", lambda result: (result, None, None))
    assert _reroute_local_dependencies(initial, fixed, (), (), frozenset({"A"}),
        frozenset({"A", "B"}), settings, EscapeFeedbackOptions()) is None
    assert len(calls) == 2
