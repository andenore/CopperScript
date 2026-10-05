"""Escape-first order, transactional placement and no-area-search gate."""
from dataclasses import replace

import pytest

import pcbir.fanout as fanout
import pcbir.package_access as access
from pcbir import (BoardOutline, ComponentPlacementRule, CopperLayer, FootprintPad,
    GlobalRouterOptions, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, RouteKind, Size, TrackSegment, nm_from_mm,
    PlacementPlannerOptions, DetailedRouterOptions, route_critical_nets, route_global)
from pcbir import RigidPlacementCluster, RigidPlacementMember, PlacementTarget
from pcbir.clusters import footprint_geometry_digest
from pcbir.drc import run_physical_drc
from pcbir.flow import PhysicalFlowStatus, run_routing_pipeline
from pcbir.routeflow import FeedbackStatus, PlacementRoutingResult


def fixture(critical=False):
    package = PhysicalFootprint("two", (
        FootprintPad("1", Point.mm(0, -1), Size.mm(.3, .3)),
        FootprintPad("2", Point.mm(0, 1), Size.mm(.3, .3))), Size.mm(2, 3))
    one = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm(.3, .3)),), Size.mm(1, 1))
    board = PhysicalBoard("access", BoardOutline.rectangle(16, 16),
        {package.name: package, one.name: one},
        (Placement("U", package.name, Point.mm(5, 7)),
         Placement("JA", one.name, Point.mm(12, 6)),
         Placement("JB", one.name, Point.mm(12, 8))),
        (PhysicalNet("A", (PadReference("U", "1"), PadReference("JA", "1"))),
         PhysicalNet("B", (PadReference("U", "2"), PadReference("JB", "1")))),
        net_routing_rules=(NetRoutingRule("A", RouteKind.CRITICAL, max_vias=0),) if critical else (),
        placement_rules=(ComponentPlacementRule("U", allowed_orientations=(0, 45)),))
    fanout_options = fanout.FanoutOptions(minimum_component_pads=2,
        maximum_neighbor_distance_nm=nm_from_mm(3), two_leg_escapes=False)
    guides_options = GlobalRouterOptions(tile_size_nm=nm_from_mm(2))
    return board, route_global(board, guides_options), fanout_options, guides_options


def test_ordinary_exits_precede_critical_routes_and_survive_exact_acceptance(monkeypatch):
    board, guides, options, _ = fixture(critical=True)
    calls = []
    real_fanout, real_critical = access.route_fanout, access.route_critical_nets
    def escape(source, settings):
        assert not source.tracks and not source.vias
        calls.append("exits")
        return real_fanout(source, settings)
    def critical(source, route, *, reserved_accesses, on_progress=None):
        assert calls == ["exits"]
        assert reserved_accesses.accesses and not reserved_accesses.pending_pads
        calls.append("critical")
        return real_critical(source, route, reserved_accesses=reserved_accesses, on_progress=on_progress)
    monkeypatch.setattr(access, "route_fanout", escape)
    monkeypatch.setattr(access, "route_critical_nets", critical)
    result = access.preflight_package_access(board, guides, options)
    assert result.ready and calls == ["exits", "critical"]
    assert result.board.tracks[:len(result.fanout.created_tracks)] == result.fanout.created_tracks
    assert result.board.vias[:len(result.fanout.created_vias)] == result.fanout.created_vias
    assert result.critical.reserved_track_count == len(result.fanout.created_tracks)
    assert result.critical.reserved_via_count == len(result.fanout.created_vias)
    assert all(v.net != "A" for v in result.board.vias)  # max_vias=0 retained
    assert not any(f.severity.value == "error" and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
                   for f in run_physical_drc(result.board).findings)


@pytest.mark.parametrize("defect", ["stale_placement", "stale_guides", "unowned_track", "protected_net", "short"])
def test_critical_router_rejects_stale_unowned_or_illegal_reservations(defect):
    board, guides, options, _ = fixture(critical=True)
    exits = fanout.route_fanout(board, options)
    if defect == "stale_placement":
        exits = replace(exits, board=replace(exits.board, placements=(
            replace(board.placements[0], position=Point.mm(6, 7)), *board.placements[1:])))
    elif defect == "stale_guides":
        guides = replace(guides, placement_fingerprint="stale")
    elif defect == "unowned_track":
        exits = replace(exits, created_tracks=())
    else:
        t = exits.created_tracks[0]
        t = replace(t, net="A") if defect == "protected_net" else replace(t, width_nm=nm_from_mm(4))
        tracks = (t, *exits.created_tracks[1:])
        exits = replace(exits, board=replace(exits.board, tracks=tracks), created_tracks=tracks)
    if defect == "short":
        result = route_critical_nets(board, guides, reserved_accesses=exits)
        assert result.status.value == "failed" and result.locked_tracks == exits.created_tracks
        assert any("DRC-" in diagnostic for item in result.nets for diagnostic in item.diagnostics)
    else:
        with pytest.raises(ValueError):
            route_critical_nets(board, guides, reserved_accesses=exits)


def test_pending_exit_stops_area_router_with_zero_search_passes(monkeypatch):
    board, guides, options, _ = fixture()
    monkeypatch.setattr(fanout, "_candidates", lambda *args: ())
    monkeypatch.setattr("pcbir.flow.optimize_placement_for_routing", lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, "fixture", (), 0, True))
    monkeypatch.setattr("pcbir.flow.route_detailed", lambda *args, **kwargs:
        pytest.fail("area router must not be called"))
    result = run_routing_pipeline(board, fanout_options=options,
        package_access_options=access.PackageAccessOptions(maximum_trials=0))
    assert result.status is PhysicalFlowStatus.FAIL
    assert not result.package_access.ready and len(result.package_access.pending_pads) == 2
    assert result.detailed.metrics.passes == 0
    assert result.detailed.metrics.unrouted_net_count == 2
    assert result.board.metadata["package_access"] == "blocked"
    assert all("preflight" in item.diagnostics[0] for item in result.detailed.nets)


def test_pending_boundary_channel_stops_area_despite_complete_dogbones(monkeypatch):
    board, guides, settings, _ = fixture()
    monkeypatch.setattr("pcbir.flow.optimize_placement_for_routing", lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, "fixture", (), 0, True))
    real = access.analyze_boundary_access
    def blocked(source, fan, options=None):
        result = real(source, fan, options)
        return replace(result, ports=result.ports[1:], pending_pads=(result.ports[0].pad,))
    monkeypatch.setattr(access, "analyze_boundary_access", blocked)
    monkeypatch.setattr("pcbir.flow.route_detailed", lambda *args, **kwargs: pytest.fail("area must not start"))
    result = run_routing_pipeline(board, fanout_options=settings,
        package_access_options=access.PackageAccessOptions(maximum_trials=0, maximum_pattern_trials=0))
    assert not result.fanout.pending_pads and result.package_access.boundary.pending_pads
    assert not result.package_access.ready and result.detailed.metrics.passes == 0
    assert result.fanout.boundary_accesses is None  # Provisional witnesses stay uncommitted.
    assert result.package_access.pending_pads == frozenset(result.package_access.boundary.pending_pads)
    proposals = list(access.package_placement_trials(result.package_access, PlacementPlannerOptions(), nm_from_mm(.5)))
    assert proposals and {ref for ref, _ in proposals} == {"U"}


def test_missing_boundary_evidence_cannot_make_access_ready():
    board, guides, settings, _ = fixture()
    result = access.preflight_package_access(board, guides, settings)
    assert result.ready and result.boundary.ready
    assert not replace(result, boundary=None).ready


def test_ready_pipeline_materializes_owned_layer_anchors_before_area(monkeypatch):
    from collections import Counter
    import pcbir.flow as flow
    from pcbir.pin_escape import RoutingAccess, verified_routing_access
    from pcbir.routing_clearance import RoutingClearanceIndex
    board, guides, settings, _ = fixture()
    monkeypatch.setattr(flow, "optimize_placement_for_routing", lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, "fixture", (), 0, True))
    real = flow.route_detailed
    calls, events = [], []
    def area(source, route, options, **kwargs):
        anchors = kwargs["fanout_accesses"]
        assert anchors and all(isinstance(a, RoutingAccess) for a in anchors.values())
        index = RoutingClearanceIndex(source)
        names = {p:n.name for n in source.nets for p in n.pads}
        assert all(verified_routing_access(source, p, names[p], a, index) for p,a in anchors.items())
        assert Counter(kwargs["fanout_created_tracks"]) <= Counter(source.tracks)
        calls.append(source)
        return real(source, route, options, **kwargs)
    monkeypatch.setattr(flow, "route_detailed", area)
    result = run_routing_pipeline(board, fanout_options=settings,
        package_access_options=access.PackageAccessOptions(maximum_trials=0),
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm(.5), maximum_passes=1),
        on_progress=lambda *event: events.append(event))
    assert len(calls) == 1 and result.package_access.ready
    baseline = result.package_access.fanout
    assert baseline.boundary_accesses is None and result.fanout.boundary_accesses
    assert result.fanout.board == calls[0]
    assert Counter(result.fanout.created_tracks)-Counter(baseline.created_tracks) == Counter(calls[0].tracks)-Counter(result.package_access.board.tracks)
    assert any(p == "package_boundary_reservation" and e == "finished" for p,e,_ in events)
    assert all(n.connected for n in result.detailed.nets)


def test_failed_critical_group_also_stops_area_and_preserves_ordinary_exits(monkeypatch):
    board, guides, options, _ = fixture(critical=True)
    real = access.route_critical_nets
    def failed(source, route, **kwargs):
        result = real(source, route, **kwargs)
        # Keep stale connected flags deliberately: fresh copper connectivity wins.
        tracks = tuple(t for t in result.board.tracks if t.net != "A")
        return replace(result, board=replace(result.board, tracks=tracks), locked_tracks=tracks)
    monkeypatch.setattr(access, "route_critical_nets", failed)
    result = access.preflight_package_access(board, guides, options)
    assert result.failed_critical_nets == {"A"} and not result.ready
    blocked = access.blocked_area_result(result)
    assert blocked.metrics.passes == 0 and blocked.board.vias == result.fanout.created_vias


def test_legal_move_rebuilds_exits_from_unrouted_source_and_is_deterministic(monkeypatch):
    board, guides, options, router = fixture()
    real = fanout._candidates
    def choices(position, center, settings):
        return () if center == Point.mm(5, 7) else real(position, center, settings)
    monkeypatch.setattr(fanout, "_candidates", choices)
    baseline = access.preflight_package_access(board, guides, options)
    kwargs = dict(options=access.PackageAccessOptions(maximum_trials=1), global_options=router)
    repaired = access.improve_package_access(baseline, options, **kwargs)
    assert repaired.ready and repaired.accepted_moves == 1
    assert repaired.trials[0].outcome == "accepted"
    assert repaired.trials[0].changed_references == ("U",)
    assert repaired.source.placements[0].position == Point.mm(5.5, 7)
    assert not repaired.source.tracks and not repaired.source.vias
    assert repaired.source.placements[1:] == board.placements[1:]
    assert repaired == access.improve_package_access(baseline, options, **kwargs)
    assert baseline.source == board and not baseline.fanout.accesses


@pytest.mark.parametrize("defect", ["lost_exit", "hard_drc", "new_critical_failure", "lost_eligibility"])
def test_unsafe_feedback_proposal_rolls_back_with_trial_budget(monkeypatch, defect):
    board, guides, options, router = fixture()
    complete = access.preflight_package_access(board, guides, options)
    pending = PadReference("U", "1")
    baseline = replace(complete, fanout=replace(complete.fanout,
        pending_pads=(pending,), accesses={p:v for p,v in complete.fanout.accesses.items() if p != pending}))
    real = access.preflight_package_access
    def unsafe(source, route, *args, **kwargs):
        result = real(source, route, *args, **kwargs)
        if defect == "lost_exit":
            return replace(result, fanout=replace(result.fanout, accesses={pending: result.fanout.accesses[pending]}))
        if defect == "hard_drc":
            return replace(result, hard_findings=1)
        if defect == "new_critical_failure":
            return replace(result, failed_critical_nets=frozenset({"A"}))
        return replace(result, fanout=replace(result.fanout, pin_analysis=()))
    monkeypatch.setattr(access, "preflight_package_access", unsafe)
    result = access.improve_package_access(baseline, options,
        options=access.PackageAccessOptions(maximum_trials=2), global_options=router)
    assert result.source == board and result.board == baseline.board
    assert result.accepted_moves == 0 and len(result.trials) == 2
    assert all(t.outcome != "accepted" for t in result.trials)


def test_fixed_owners_and_explicit_45_degree_orientations_are_respected():
    board, guides, options, _ = fixture()
    baseline = access.preflight_package_access(board, guides, options)
    baseline = replace(baseline, fanout=replace(baseline.fanout, pending_pads=(PadReference("U", "1"),)))
    trials = list(access.package_placement_trials(baseline, PlacementPlannerOptions(), nm_from_mm(.5)))
    assert trials and any(trial.placements[0].rotation_degrees == 45 for _, trial in trials)
    assert all(trial.placements[0].rotation_degrees in {0,45} for _, trial in trials)
    assert not list(access.package_placement_trials(baseline,
        PlacementPlannerOptions(fixed_references={"U"}), nm_from_mm(.5)))


def test_pipeline_routes_area_only_after_successful_preflight(monkeypatch):
    board, guides, options, _ = fixture(critical=True)
    monkeypatch.setattr("pcbir.flow.optimize_placement_for_routing", lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, "fixture", (), 0, True))
    result = run_routing_pipeline(board, fanout_options=options,
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm(.5), maximum_passes=1),
        package_access_options=access.PackageAccessOptions(maximum_trials=0))
    assert result.package_access.ready and result.detailed.metrics.passes == 1
    assert result.critical.reserved_track_count > 0


@pytest.mark.parametrize("defect", ["source_copper", "stale_guides"])
def test_preflight_rejects_invalid_source_or_guides(defect):
    board, guides, options, _ = fixture()
    if defect == "source_copper":
        board = replace(board, tracks=(TrackSegment("A", Point.mm(5,6), Point.mm(6,6),
                                                   nm_from_mm(.2), CopperLayer.FRONT),))
    else:
        guides = replace(guides, placement_fingerprint="stale")
    with pytest.raises(ValueError):
        access.preflight_package_access(board, guides, options)


def test_options_validate_bounds():
    with pytest.raises(ValueError):
        access.PackageAccessOptions(maximum_trials=-1)
    with pytest.raises(ValueError):
        access.PackageAccessOptions(movement_nm=0)
    for budget in (-1, 3):
        with pytest.raises(ValueError, match="pattern"):
            access.PackageAccessOptions(maximum_pattern_trials=budget)


def pattern_trap():
    """An ordinary dogbone blocks a length-constrained critical launch.

    Real radial domains and exact critical routing are used, not a mocked
    connected flag. There is a legal alternative ordinary via on the other side.
    """
    board, _, settings, global_options = fixture(critical=True)
    package = PhysicalFootprint("two", (
        FootprintPad("1", Point.mm(0, -1), Size.mm(.3, .3)),
        FootprintPad("2", Point.mm(0, -.4), Size.mm(.3, .3))), Size.mm(2, 3))
    board = replace(board, footprints={"two": package, "one": board.footprints["one"]},
        placements=(board.placements[0], replace(board.placements[1], position=Point.mm(3, 6)),
                    replace(board.placements[2], position=Point.mm(13, 6.6))),
        net_routing_rules=(NetRoutingRule("A", RouteKind.CRITICAL, max_vias=0,
            allowed_layers=(CopperLayer.FRONT,), max_length_nm=nm_from_mm(2)),))
    return board, route_global(board, global_options), settings


def test_pattern_negotiation_replaces_blocking_dogbone_without_moving_components():
    board, guides, settings = pattern_trap()
    baseline = access.preflight_package_access(board, guides, settings,
        options=access.PackageAccessOptions(maximum_pattern_trials=0))
    assert baseline.failed_critical_nets == {"A"} and not baseline.pending_pads
    events = []
    result = access.preflight_package_access(board, guides, settings,
        on_progress=lambda phase, event, details: events.append((phase, event, details)))
    assert result.ready and result.source == board
    assert result.board.placements == board.placements
    assert result.accepted_moves == 0 and result.trials == ()
    assert result.pattern_trials[0].strategy == "critical_first"
    assert result.pattern_trials[0].outcome == "accepted"
    assert result.pattern_trials[0].revalidated
    assert result.fanout.accesses.keys() == baseline.fanout.accesses.keys()
    assert result.fanout.created_vias != baseline.fanout.created_vias
    assert result.fanout.board.metadata == board.metadata
    assert all(t.net == "B" for t in result.fanout.created_tracks)
    assert all(v.net == "B" for v in result.fanout.created_vias)
    assert result.board.tracks[:len(result.fanout.created_tracks)] == result.fanout.created_tracks
    assert result.critical.reserved_track_count == len(result.fanout.created_tracks)
    assert result.critical.reserved_via_count == len(result.fanout.created_vias)
    assert not result.hard_findings
    assert result == access.preflight_package_access(board, guides, settings)
    assert any(phase == "package_pattern_trial" and event == "finished"
               and details["outcome"] == "accepted" for phase, event, details in events)


@pytest.mark.parametrize("defect", ["lost_exit", "lost_eligibility", "hard_drc", "new_failure", "no_improvement"])
def test_pattern_proposal_is_revalidated_and_rolls_back_atomically(monkeypatch, defect):
    board, guides, settings = pattern_trap()
    baseline = access.preflight_package_access(board, guides, settings,
        options=access.PackageAccessOptions(maximum_pattern_trials=0))
    real = access._access_result
    def unsafe(source, route, fan, critical, plane, *args):
        result = real(source, route, fan, critical, plane, *args)
        if not result.failed_critical_nets:  # Final revalidation, not the incumbent.
            if defect == "lost_exit":
                return replace(result, fanout=replace(result.fanout, accesses={}))
            if defect == "lost_eligibility":
                return replace(result, fanout=replace(result.fanout, pin_analysis=()))
            if defect == "hard_drc":
                return replace(result, hard_findings=1)
            if defect == "new_failure":
                return replace(result, failed_critical_nets=frozenset({"B"}))
            return replace(result, failed_critical_nets=baseline.failed_critical_nets)
        return result
    monkeypatch.setattr(access, "_access_result", unsafe)
    result = access.preflight_package_access(board, guides, settings,
        options=access.PackageAccessOptions(maximum_pattern_trials=1))
    assert result.board == baseline.board and result.fanout == baseline.fanout
    assert result.critical == baseline.critical
    assert len(result.pattern_trials) == 1 and result.pattern_trials[0].outcome != "accepted"


def test_ready_preflight_does_not_pay_for_pattern_probes(monkeypatch):
    board, guides, settings, _ = fixture(critical=True)
    real = access.route_critical_nets
    def critical(source, route, *, reserved_accesses, on_progress=None):
        return real(source, route, reserved_accesses=reserved_accesses, on_progress=on_progress)
    monkeypatch.setattr(access, "route_critical_nets", critical)
    result = access.preflight_package_access(board, guides, settings)
    assert result.ready and not result.pattern_trials


def test_plane_contact_can_negotiate_ordinary_exits_with_exact_final_rebuild(monkeypatch):
    from pcbir import CopperZone, PolygonWithHoles, PolygonRing
    from pcbir.plane import PlaneStitchOptions
    board, _, settings = pattern_trap()
    zone = CopperZone("plane", "A", (CopperLayer.BACK,),
        PolygonWithHoles(PolygonRing(board.outline.vertices)))
    board = replace(board, net_routing_rules=(), zones=(zone,))
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    # Isolate one constrained ground via window. Clearance, annulus/pad
    # exclusion, surface copper and final plane ownership remain real checks.
    monkeypatch.setattr("pcbir.plane._candidate_points", lambda *args: (Point.mm(4, 6),))
    monkeypatch.setattr("pcbir.plane.surface_path_to_via", lambda *args, **kwargs: None)
    planes = PlaneStitchOptions(include_surface_zones=True,
        only_pads=frozenset({PadReference("U", "1")}))
    baseline = access.preflight_package_access(board, guides, settings, planes,
        options=access.PackageAccessOptions(maximum_pattern_trials=0))
    assert baseline.pending_pads == {PadReference("U", "1")}
    assert not baseline.fanout.pending_pads and not baseline.hard_findings
    result = access.preflight_package_access(board, guides, settings, planes)
    assert result.ready and result.source == board
    assert result.pattern_trials[0].strategy == "critical_and_plane_first"
    assert result.pattern_trials[0].outcome == "accepted"
    assert set(result.fanout.accesses) == set(baseline.fanout.accesses)
    assert all(item.net == "B" for item in (*result.fanout.created_tracks, *result.fanout.created_vias))
    assert result.plane_stitch.added_via_count == 1
    assert result.plane_stitch.board.vias[-1].net == "A"
    assert not result.hard_findings


def test_unavoidable_critical_failure_skips_repeated_pattern_search(monkeypatch):
    board, guides, settings = pattern_trap()
    baseline = access.preflight_package_access(board, guides, settings,
        options=access.PackageAccessOptions(maximum_pattern_trials=0))
    calls = []
    def critical(*args, **kwargs):
        calls.append(kwargs.get("reserved_accesses"))
        return baseline.critical
    monkeypatch.setattr(access, "route_critical_nets", critical)
    result = access.preflight_package_access(board, guides, settings)
    assert not result.ready and result.board == baseline.board
    assert len(calls) == 2 and calls[0] is not None and calls[1] is None
    assert not result.pattern_trials


@pytest.mark.parametrize("budget", [0, 1])
def test_pipeline_obeys_pattern_budget_before_starting_area(monkeypatch, budget):
    board, guides, settings = pattern_trap()
    monkeypatch.setattr("pcbir.flow.optimize_placement_for_routing", lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, "fixture", (), 0, True))
    result = run_routing_pipeline(board, fanout_options=settings,
        package_access_options=access.PackageAccessOptions(maximum_trials=0, maximum_pattern_trials=budget),
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm(.5), maximum_passes=1))
    assert result.package_access.ready == bool(budget)
    assert result.detailed.metrics.passes == budget
    assert len(result.package_access.pattern_trials) == budget


def test_cli_pattern_budget_is_explicit_and_bounded():
    from pcbir.cli import _parser
    parser = _parser()
    assert parser.parse_args(["route-board", "board.copper"]).package_pattern_trials == 2
    assert parser.parse_args(["route-board", "board.copper", "--package-pattern-trials", "0"]).package_pattern_trials == 0
    with pytest.raises(SystemExit):
        parser.parse_args(["route-board", "board.copper", "--package-pattern-trials", "3"])
    assert parser.parse_args(["route-board", "board.copper"]).package_initial_pair_states == 6000
    assert parser.parse_args(["route-board", "board.copper", "--package-initial-pair-states", "0"]).package_initial_pair_states == 0
    with pytest.raises(SystemExit):
        parser.parse_args(["route-board", "board.copper", "--package-initial-pair-states", "-1"])
    with pytest.raises(ValueError, match="initial pair"):
        access.PackageAccessOptions(initial_pair_state_limit=-1)


@pytest.mark.parametrize("case", ("easy", "alternate", "full_only", "unavoidable", "rollback"))
def test_staged_scheduler_work_fallback_and_identity_gate(monkeypatch, case):
    # Isolate scheduling/rollback. Other tests above use actual copper/native
    # gates; pair_vias tests exercise limited searches and coupled acceptance.
    board, guides, settings, _ = fixture(critical=True)
    template = access.preflight_package_access(board, guides, settings)
    board = replace(board, net_routing_rules=tuple(NetRoutingRule(n, RouteKind.DIFFERENTIAL,
        differential_partner=p, pair_gap_nm=nm_from_mm(.2)) for n, p in (("A", "B"), ("B", "A"))))
    guides = route_global(board)
    original = template.fanout
    alternative = replace(original, created_tracks=tuple(replace(t, end=Point(t.end.x_nm + 1, t.end.y_nm))
                                                         for t in original.created_tracks))
    proposals, searches, events = [], [], []
    def escape(source, options):
        proposals.append(source)
        return original if len(proposals) == 1 else alternative
    def critical(source, route, *, reserved_accesses=None, pair_state_limit=None, on_progress=None):
        searches.append((pair_state_limit, reserved_accesses))
        connected = (case == "easy" or case == "alternate" and reserved_accesses is not original
                     or case == "full_only" and pair_state_limit is None)
        # Clean-owner probe can propose a new pattern, including a rejected
        # transaction. Preserve all work in telemetry regardless of selection.
        if reserved_accesses is None and case in {"alternate", "rollback"}:
            connected = True
        if (case == "rollback" and reserved_accesses is not None
                and reserved_accesses.created_tracks == alternative.created_tracks):
            connected = True
        nets = tuple(replace(n, connected=connected, search_states=3 if pair_state_limit else 30,
                             pair_searches=1) for n in template.critical.nets)
        return replace(template.critical, nets=nets,
            status=access.CriticalRoutingStatus.SUCCESS if connected else access.CriticalRoutingStatus.FAILED)
    def evaluate(source, route, fan, critical, plane, *args):
        failed = frozenset() if critical.nets[0].connected else frozenset({"A"})
        # Reject a "connected" proposal that lost an original required access.
        if case == "rollback" and fan.created_tracks == alternative.created_tracks:
            fan = replace(fan, accesses={})
        return replace(template, source=source, global_route=route, fanout=fan, critical=critical,
                       failed_critical_nets=failed)
    monkeypatch.setattr(access, "route_fanout", escape)
    monkeypatch.setattr(access, "route_critical_nets", critical)
    monkeypatch.setattr(access, "_access_result", evaluate)
    monkeypatch.setattr(access, "_failures", lambda source, critical:
        (frozenset() if critical.nets[0].connected else frozenset({"A"}), 0))
    options = access.PackageAccessOptions(maximum_pattern_trials=1, initial_pair_state_limit=12)
    result = access.preflight_package_access(board, guides, settings, options=options,
        on_progress=lambda *event: events.append(event))
    assert result.ready == (case in {"easy", "alternate", "full_only"})
    assert [tier.name for tier in result.search_tiers] == (
        ["initial"] if case in {"easy", "alternate"} else ["initial", "full"])
    assert sum(tier.expanded_states for tier in result.search_tiers) == sum(3 if cap else 30 for cap, _ in searches)
    assert sum(tier.critical_passes for tier in result.search_tiers) == len(searches)
    assert sum(tier.selected for tier in result.search_tiers) == 1
    if case == "easy":
        assert len(searches) == 1 and not result.pattern_trials
    elif case == "alternate":
        assert len(searches) == 3 and result.pattern_trials[0].outcome == "accepted"
        assert result.pattern_trials[0].search_tier == "initial"
        assert all(details['search_tier'] == 'initial' for phase, _, details in events
                   if phase in {'package_pattern_trial', 'package_pattern_probe'})
    else:
        # Full fallback retries the original ordinary pattern, not a leaked
        # clean-probe board or an unaccepted candidate's copper.
        assert next(fan for cap, fan in searches if cap is None) is original
        assert any(phase == "package_search_fallback" for phase, _, _ in events)
    if case == "rollback":
        assert result.fanout.accesses == original.accesses
        assert all(t.outcome == "access_not_improved" for t in result.pattern_trials)
    if case == "full_only":
        assert result.search_tiers[-1].selected


@pytest.mark.parametrize("initial,patterns", ((0, 2), (6000, 0)))
def test_disabling_staging_retains_one_full_budget_tier(initial, patterns):
    # Pair-free fixtures also use a single full tier, as no pair work is capped.
    board, guides, settings, _ = fixture(critical=True)
    result = access.preflight_package_access(board, guides, settings,
        options=access.PackageAccessOptions(initial_pair_state_limit=initial, maximum_pattern_trials=patterns))
    assert len(result.search_tiers) == 1 and result.search_tiers[0].name == "full"
    assert result.search_tiers[0].pair_state_limit is None and result.search_tiers[0].selected


def test_pending_ordinary_exit_can_negotiate_even_when_critical_owner_already_passes(monkeypatch):
    from types import MappingProxyType
    board, guides, settings = pattern_trap()
    real = access.route_fanout
    real_critical = access.route_critical_nets
    calls = []
    critical_calls = []
    def critical(source, route, *, reserved_accesses, on_progress=None):
        # An already-successful specialized route seeds the proposal; only
        # baseline routing and final full revalidation need a critical search.
        critical_calls.append(reserved_accesses)
        return real_critical(source, route, reserved_accesses=reserved_accesses, on_progress=on_progress)
    def exhausted_first_pattern(source, options):
        result = real(source, options)
        calls.append(result)
        if len(calls) == 1:
            # Simulate a bounded ordinary allocation exhausting its budget;
            # critical routing and the final escape proposal remain real.
            return replace(result, board=source, accesses=MappingProxyType({}),
                pending_pads=tuple(item.pad for item in result.pin_analysis),
                created_tracks=(), created_vias=(), added_track_count=0, added_via_count=0,
                pin_analysis=tuple(replace(item, selected_candidate_index=None) for item in result.pin_analysis))
        return result
    monkeypatch.setattr(access, "route_fanout", exhausted_first_pattern)
    monkeypatch.setattr(access, "route_critical_nets", critical)
    result = access.preflight_package_access(board, guides, settings)
    assert result.ready and result.source == board and not result.hard_findings
    assert len(calls) == 2
    assert len(critical_calls) == 2
    assert result.pattern_trials[0].outcome == "accepted"
    assert result.pattern_trials[0].revalidated


def test_critical_only_feedback_cannot_silently_drop_reserved_ordinary_access():
    from pcbir.critical_feedback import improve_critical_placement
    board, guides, options, _ = fixture(critical=True)
    baseline = access.preflight_package_access(board, guides, options)
    with pytest.raises(ValueError, match="package access"):
        improve_critical_placement(board, guides, baseline.critical, maximum_trials=1)


def test_rigid_companion_moves_with_owner_and_fixed_companion_blocks_entire_unit():
    board, _, options, router = fixture()
    companion = Placement("C", "one", Point.mm(5, 11))
    cluster = RigidPlacementCluster("unit", PlacementTarget("U"),
        (RigidPlacementMember("U", "two", footprint_geometry_digest(board.footprints["two"]), Point(0, 0)),
         RigidPlacementMember("C", "one", footprint_geometry_digest(board.footprints["one"]), Point.mm(0, 4))),
        "synthetic fixture", (0, 45))
    board = replace(board, placements=(*board.placements, companion), rigid_clusters=(cluster,))
    baseline = access.preflight_package_access(board, route_global(board, router), options)
    baseline = replace(baseline, fanout=replace(baseline.fanout, pending_pads=(PadReference("U", "1"),)))
    trials = list(access.package_placement_trials(baseline, PlacementPlannerOptions(), nm_from_mm(.5)))
    assert trials
    for _, trial in trials:
        if trial.placements[0].rotation_degrees == 0:
            assert trial.placements[-1].position.x_nm - trial.placements[0].position.x_nm == 0
            assert trial.placements[-1].position.y_nm - trial.placements[0].position.y_nm == nm_from_mm(4)
    assert not list(access.package_placement_trials(baseline,
        PlacementPlannerOptions(fixed_references={"C"}), nm_from_mm(.5)))


def test_stale_feedback_source_rejected_even_when_moves_disabled():
    board, guides, options, _ = fixture()
    baseline = access.preflight_package_access(board, guides, options)
    baseline = replace(baseline, global_route=replace(guides, placement_fingerprint="stale"))
    with pytest.raises(ValueError, match="stale"):
        access.improve_package_access(baseline, options, options=access.PackageAccessOptions(maximum_trials=0))
