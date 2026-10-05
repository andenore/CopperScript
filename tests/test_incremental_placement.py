"""Placement repair keeps exact unrelated copper and rejects unsafe reuse."""
from collections import Counter
from dataclasses import replace

import pytest

from pcbir import (
    DetailedRouterOptions, EscapeFeedbackOptions, FootprintPad, GlobalRouterOptions,
    NetRoutingRule, PadReference, PhysicalFootprint, PhysicalNet, Placement,
    PlacementPlannerOptions, PlacementRoutingFeedbackOptions, PlaneStitchOptions,
    Point, RouteKind, Size, nm_from_mm, run_routing_pipeline, stitch_zone_pads,
)
from pcbir.drc import run_physical_drc
from pcbir.fanout import FanoutOptions, FanoutResult
from pcbir.incremental_placement import repair_placement_trial
from test_escape_feedback import _trapped_ground_board


@pytest.fixture
def placement_case():
    source = _trapped_ground_board()
    single = source.footprints["test/one-pad"]
    package = PhysicalFootprint("test/two-pad", (
        single.pads[0], FootprintPad("2", Point.mm(0, 1), Size.mm(.6, .6)),
    ), Size.mm(1, 2))
    source = replace(source, footprints={**source.footprints, package.name: package},
        placements=(replace(source.placements[0], footprint=package.name),
                    source.placements[1], Placement("S", single.name, Point.mm(12, 9)),
                    Placement("K1", single.name, Point.mm(10, 3)),
                    Placement("K2", single.name, Point.mm(17, 3))),
        nets=(*source.nets,
              PhysicalNet("SIGNAL", (PadReference("G1", "2"), PadReference("S", "1"))),
              PhysicalNet("KEEP", (PadReference("K1", "1"), PadReference("K2", "1")))),
        net_routing_rules=(NetRoutingRule("KEEP", kind=RouteKind.CLOCK, max_vias=0),))
    detail = DetailedRouterOptions(pitch_nm=nm_from_mm(.5), maximum_passes=2,
                                   maximum_search_states=20_000)
    global_options = GlobalRouterOptions(tile_size_nm=nm_from_mm(2))
    placement_options = PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
                                                 refinement_passes=0)
    initial = run_routing_pipeline(source,
        placement_options=replace(placement_options,
            fixed_references=frozenset(p.reference for p in source.placements)),
        global_options=global_options,
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_options=detail)
    plane = PlaneStitchOptions(maximum_radius_nm=nm_from_mm(1))
    baseline = stitch_zone_pads(initial.board, plane)
    assert baseline.pending_pads == (PadReference("G1", "1"),)
    assert all(item.connected for item in initial.detailed.nets if item.net != "GND"), initial.detailed.nets
    assert initial.critical.nets and initial.critical.nets[0].connected
    trial = replace(initial.placement_and_global.board, placements=tuple(
        replace(p, position=Point.mm(7, 6)) if p.reference == "G1" else p
        for p in source.placements))
    kwargs = dict(options=EscapeFeedbackOptions(), detailed_options=detail,
                  placement_options=placement_options, global_options=global_options)
    early = replace(plane, only_pads=frozenset({PadReference("G1", "1")}))
    return initial, trial, baseline, early, plane, kwargs


def repair(case, **overrides):
    initial, trial, baseline, early, plane, kwargs = case
    return repair_placement_trial(initial, trial, baseline, early, plane, **(kwargs | overrides))


def test_real_move_repairs_incident_net_and_preserves_critical_copper(placement_case):
    initial, trial, baseline, _, _, _ = placement_case
    before = initial.board
    events = []
    result = repair(placement_case, on_progress=lambda *event: events.append(event))
    assert result is not None and result.plane_stitch.complete
    assert result.changed_references == ("G1",)
    assert result.repair_nets == ("SIGNAL",)
    assert result.rebuilt_zone_nets == ("GND",)
    assert result.pipeline.placement_and_global.board == trial
    assert result.pipeline.critical.nets == initial.critical.nets
    for objects in ("tracks", "vias"):
        assert Counter(item for item in getattr(result.pipeline.board, objects) if item.net == "KEEP") == Counter(
            item for item in getattr(before, objects) if item.net == "KEEP")
    assert result.pipeline.detailed.global_routing_fingerprint == result.pipeline.placement_and_global.global_route.routing_fingerprint
    assert result.pipeline.critical.global_routing_fingerprint == result.pipeline.detailed.global_routing_fingerprint
    assert result.pipeline.critical.board.metadata["global_routing_fingerprint"] == result.pipeline.detailed.global_routing_fingerprint
    assert result.pipeline.board.metadata["global_routing_fingerprint"] == result.pipeline.detailed.global_routing_fingerprint
    assert result.pipeline.detailed.global_routing_fingerprint != initial.detailed.global_routing_fingerprint
    assert result.pipeline.critical.routing_fingerprint != initial.critical.routing_fingerprint
    assert all(item.connected for item in result.pipeline.detailed.nets if item.net != "GND")
    assert not [f for f in run_physical_drc(result.plane_stitch.board).findings
                if f.severity.value == "error" and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]
    assert initial.board == before and baseline.board.placements != trial.placements
    assert any(phase == "zone_subset_search" for phase, _, _ in events)
    assert repair(placement_case) == result


def test_allowed_45_degree_rotation_rebuilds_offset_pad_route(placement_case):
    initial, trial, baseline, early, plane, kwargs = placement_case
    # First make a baseline at x=6: rotating the off-centre ground pad moves
    # it east out of the via keepout while the ordinary pad moves as well.
    package = initial.placement_and_global.board.footprints["test/two-pad"]
    package = replace(package, pads=(replace(package.pads[0], position=Point.mm(0, 1)),
                                     replace(package.pads[1], position=Point.mm(0, -1))))
    source = replace(initial.placement_and_global.board,
        footprints={**initial.board.footprints, package.name: package},
        copper_keepouts=tuple(replace(k, outline=replace(k.outline, outer=replace(k.outline.outer,
            vertices=(Point.mm(3, 4), Point.mm(7.3, 4), Point.mm(7.3, 8), Point.mm(3, 8)))))
            for k in initial.board.copper_keepouts),
        placements=tuple(replace(p, position=Point.mm(6, 6)) if p.reference == "G1" else p
                         for p in initial.board.placements))
    initial = run_routing_pipeline(source, placement_options=replace(kwargs["placement_options"],
        fixed_references=frozenset(p.reference for p in source.placements)),
        global_options=kwargs["global_options"], detailed_options=kwargs["detailed_options"],
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1))
    baseline = stitch_zone_pads(initial.board, plane)
    assert baseline.pending_pads
    trial = replace(initial.placement_and_global.board, placements=tuple(
        replace(p, rotation_degrees=45) if p.reference == "G1" else p for p in source.placements))
    events = []
    result = repair_placement_trial(initial, trial, baseline, early, plane, **kwargs,
        on_progress=lambda *event: events.append(event))
    assert result is not None and result.plane_stitch.complete, events
    assert result.repair_nets == ("SIGNAL",)
    assert result.pipeline.board.placements[0].rotation_degrees == 45


@pytest.mark.parametrize("guard", ["critical", "fanout-owner", "crowded", "incomplete",
                                   "source-change", "illegal", "reference", "net-cap",
                                   "locked-collision", "missing-ownership", "boundary-owner"])
def test_unsupported_transactions_fallback_without_mutation(placement_case, guard):
    initial, trial, baseline, early, plane, kwargs = placement_case
    before = initial.board
    if guard == "critical":
        trial = replace(trial, placements=tuple(
            replace(p, position=Point.mm(11, 3)) if p.reference == "K1" else p for p in trial.placements))
    elif guard == "fanout-owner":
        initial = replace(initial, fanout=FanoutResult(initial.critical.board,
            {PadReference("G1", "2"): Point.mm(5, 7)}, (), 0, 0))
    elif guard == "crowded":
        initial = replace(initial, fanout=FanoutResult(initial.critical.board, {}, (), 0, 0))
        kwargs = kwargs | {"fanout_options": FanoutOptions(minimum_component_pads=2)}
    elif guard == "incomplete":
        initial = replace(initial, detailed=replace(initial.detailed, nets=tuple(
            replace(n, connected=False) if n.net == "SIGNAL" else n for n in initial.detailed.nets)))
    elif guard == "source-change":
        # Any source-side data change is outside a pure pose transaction.
        trial = replace(trial, name="other-board")
    elif guard == "illegal":
        trial = replace(trial, placements=tuple(replace(p, position=Point.mm(.5, .5))
            if p.reference == "G1" else p for p in trial.placements))
    elif guard == "reference":
        trial = replace(trial, placements=(*trial.placements,
            Placement("OTHER", "test/one-pad", Point.mm(3, 3))))
    elif guard == "locked-collision":
        # Place the grounded pad on the unchanged KEEP clock track, without
        # changing either endpoint of that critical route.
        trial = replace(trial, placements=tuple(replace(p, position=Point.mm(13, 3))
            if p.reference == "G1" else p for p in trial.placements))
    elif guard == "missing-ownership":
        baseline = replace(baseline, board=replace(baseline.board,
            tracks=tuple(t for t in baseline.board.tracks if t.net != "KEEP")))
    elif guard == "boundary-owner":
        from pcbir.pin_escape import RoutingAccess
        track = next(t for t in initial.board.tracks if t.net == "SIGNAL")
        initial = replace(initial, fanout=FanoutResult(initial.critical.board, {}, (), 0, 0,
            boundary_accesses={PadReference("G1", "2"): RoutingAccess(track.end, track.layer, track.start, (track,))}))
    elif guard == "net-cap":
        # Two completed ordinary nets incident to a three-pad package exceed
        # the one-net cap even though both are otherwise eligible for repair.
        package = initial.board.footprints["test/two-pad"]
        package = replace(package, pads=(*package.pads,
            replace(package.pads[0], number="3", position=Point.mm(0, -1))))
        source = replace(initial.placement_and_global.board,
            footprints={**initial.board.footprints, package.name: package},
            net_routing_rules=(), nets=tuple(replace(n, pads=(*n.pads, PadReference("G1", "3")))
                                             if n.name == "KEEP" else n for n in initial.board.nets))
        initial = run_routing_pipeline(source,
            placement_options=replace(kwargs["placement_options"],
                fixed_references=frozenset(p.reference for p in source.placements)),
            global_options=kwargs["global_options"], detailed_options=kwargs["detailed_options"],
            feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1))
        assert all(n.connected for n in initial.detailed.nets if n.net != "GND")
        baseline = stitch_zone_pads(initial.board, plane)
        before = initial.board
        trial = replace(source, placements=trial.placements)
        kwargs = kwargs | {"options": EscapeFeedbackOptions(maximum_local_blockers=1)}
    events = []
    result = repair_placement_trial(initial, trial, baseline, early, plane,
        **kwargs, on_progress=lambda *event: events.append(event))
    assert result is None
    assert initial.board == before
    assert any(phase == "zone_incremental_guard" and "reason" in details for phase, _, details in events)


def test_access_stage_is_rebased_without_claiming_old_domain_analysis(placement_case):
    initial, trial, _, early, plane, kwargs = placement_case
    source = initial.placement_and_global.board
    initial = run_routing_pipeline(source,
        placement_options=replace(kwargs["placement_options"],
            fixed_references=frozenset(p.reference for p in source.placements)),
        global_options=kwargs["global_options"], detailed_options=kwargs["detailed_options"],
        fanout_options=FanoutOptions(),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1))
    assert initial.package_access.ready
    result = repair_placement_trial(initial, trial, stitch_zone_pads(initial.board, plane),
        early, plane, **kwargs, fanout_options=FanoutOptions())
    assert result is not None
    access = result.pipeline.package_access
    assert access.ready and access.source == trial
    assert access.global_route == result.pipeline.placement_and_global.global_route
    assert access.critical == result.pipeline.critical
    assert access.plane_stitch.board.metadata["global_routing_fingerprint"] == access.global_route.routing_fingerprint
    assert access.fanout.pin_analysis == () and access.fanout.assignment is None
    assert access.trials == () and access.accepted_moves == 0


@pytest.mark.parametrize("enabled", [True, False])
def test_full_pipeline_comparison_uses_same_acceptance_gates(placement_case, monkeypatch, enabled):
    import pcbir.escape_feedback as feedback
    initial, trial, _, _, plane, kwargs = placement_case
    calls = []
    full = feedback.run_routing_pipeline
    def count_full(*args, **options):
        calls.append(args[0])
        return full(*args, **options)
    monkeypatch.setattr(feedback, "run_routing_pipeline", count_full)
    monkeypatch.setattr(feedback, "_candidate_placements", lambda *args: iter([(trial, None, "known legal move")]))
    result = feedback.improve_zone_escapes(initial, plane,
        placement_options=kwargs["placement_options"], global_options=kwargs["global_options"],
        detailed_options=kwargs["detailed_options"],
        options=EscapeFeedbackOptions(maximum_trials=1, maximum_local_trials=0,
                                      incremental_placement=enabled))
    assert result.plane_stitch.complete and result.attempts[0].accepted
    assert len(calls) == (0 if enabled else 1)
    assert result.attempts[0].strategy == ("incremental_placement" if enabled else "full_pipeline")
