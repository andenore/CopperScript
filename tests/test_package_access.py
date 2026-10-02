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
    def critical(source, route, *, reserved_accesses):
        assert calls == ["exits"]
        assert reserved_accesses.accesses and not reserved_accesses.pending_pads
        calls.append("critical")
        return real_critical(source, route, reserved_accesses=reserved_accesses)
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
    def unsafe(source, route, *args):
        result = real(source, route, *args)
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
