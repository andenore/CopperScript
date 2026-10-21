"""Bounded, real-geometry placement probes; no consumer selectors or search mocks."""
from dataclasses import replace
from collections import Counter
from hashlib import sha256
import json

import pytest

from pcbir import (BoardOutline, ComponentPlacementRule, CopperLayer, CopperZone,
    DetailedRouterOptions, FootprintPad, GlobalRouterOptions, PadReference,
    NetRoutingRule, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, PlacementPlannerOptions,
    PlacementRoutingFeedbackOptions, Point, PolygonRing, PolygonWithHoles, Size,
    RigidPlacementCluster, RigidPlacementMember, PlacementTarget, RouteKind,
    Stackup, nm_from_mm, route_global)
from pcbir.clusters import footprint_geometry_digest
from pcbir.hard_macros import bind_hard_macro, materialize_hard_macros, validate_hard_macros
from pcbir.drc import run_physical_drc
from pcbir.fanout import FanoutOptions
from pcbir.flow import run_routing_pipeline
from pcbir.package_access import (PackageAccessOptions, improve_package_access,
    PackagePatternTrial, package_placement_trials, preflight_package_access)
from pcbir.placement import placement_metrics
from pcbir.physical import PlacementRegion
from pcbir.routing import _placement_fingerprint


def contact_distance_trap(*, renamed=False):
    moving, fixed, remote, rail, signal = (("IC_48", "FEED_9", "LOAD_21", "VDD_X", "DATA_Z")
        if renamed else ("DEVICE", "FEED", "REMOTE", "RAIL", "DATA"))
    package = PhysicalFootprint("synthetic-paired-lands", (
        FootprintPad("1", Point.mm(0, -1), Size.mm("0.3", "0.3")),
        FootprintPad("2", Point.mm(0, 1), Size.mm("0.3", "0.3"))), Size.mm(2, 3))
    terminal = PhysicalFootprint("synthetic-terminal", (
        FootprintPad("1", Point(0, 0), Size.mm("0.3", "0.3")),), Size.mm(1, 1))
    zone = CopperZone("scarce-contact", rail, (CopperLayer.INTERNAL_1,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in (
            ("3.95", "5.95"), ("4.05", "5.95"), ("4.05", "6.05"), ("3.95", "6.05"))))))
    board = PhysicalBoard("contact-placement-trap", BoardOutline.rectangle(16, 16),
        {package.name: package, terminal.name: terminal},
        (Placement(moving, package.name, Point.mm("7.5", 7)),
         Placement(fixed, terminal.name, Point.mm(3, 6)),
         Placement(remote, terminal.name, Point.mm(13, 8))),
        (PhysicalNet(rail, (PadReference(moving, "1"), PadReference(fixed, "1"))),
         PhysicalNet(signal, (PadReference(moving, "2"), PadReference(remote, "1")))),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                        CopperLayer.INTERNAL_2, CopperLayer.BACK)), zones=(zone,),
        placement_rules=(ComponentPlacementRule(moving, allowed_orientations=(0,)),))
    placement = PlacementPlannerOptions(fixed_references={fixed, remote})
    fanout = FanoutOptions(minimum_component_pads=2,
        maximum_neighbor_distance_nm=nm_from_mm(3), two_leg_escapes=False)
    global_options = GlobalRouterOptions(tile_size_nm=nm_from_mm(2))
    options = PackageAccessOptions(maximum_trials=1, maximum_pattern_trials=0,
                                  maximum_placement_probes=8)
    return board, placement, fanout, global_options, options, moving, signal


def hard_findings(board):
    return [f for f in run_physical_drc(board).findings if f.severity.value == "error"
            and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


@pytest.mark.parametrize("renamed", (False, True))
def test_contact_feasibility_outranks_shorter_wire_and_closes_one_trial(renamed):
    board, placement, fanout, global_options, options, moving, _ = contact_distance_trap(renamed=renamed)
    assert not hard_findings(board)
    guides = route_global(board, global_options)
    baseline = preflight_package_access(board, guides, fanout, options=options)
    assert not baseline.ready and baseline.plane_stitch.pending_pads == (PadReference(moving, "1"),)
    raw = list(package_placement_trials(baseline, placement, options.movement_nm))
    east, west = raw[0][1], raw[1][1]
    metric = lambda b: placement_metrics(b, {p.reference: p for p in b.placements}, placement)
    assert metric(east).weighted_wire_length_nm < metric(west).weighted_wire_length_nm
    # One expensive full trial in raw order cannot reach the contact.
    unranked = improve_package_access(baseline, fanout,
        options=replace(options, maximum_placement_probes=0),
        placement_options=placement, global_options=global_options)
    assert not unranked.ready and unranked.board == baseline.board
    assert unranked.placement_stop_reason == "trial_budget_exhausted"
    events = []
    result = improve_package_access(baseline, fanout, options=options,
        placement_options=placement, global_options=global_options,
        on_progress=lambda *e: events.append(e))
    assert result.ready and result.accepted_moves == 1 and len(result.trials) == 1
    assert result.source == west and result.source.placements[1:] == board.placements[1:]
    assert not result.source.tracks and not result.source.vias and not result.source.zone_fills
    assert result.trials[0].placement_probe_index == 2
    assert len(result.placement_probes) == options.maximum_placement_probes
    probes = {p.placement_fingerprint: p for p in result.placement_probes}
    assert probes[_placement_fingerprint(east)].plane_pending_pads
    assert not probes[_placement_fingerprint(west)].plane_pending_pads
    assert probes[_placement_fingerprint(west)].rank < probes[_placement_fingerprint(east)].rank
    assert result.placement_stop_reason == "ready" and not hard_findings(result.board)
    assert not result.board.zone_fills and result.board.metadata["fabrication_ready"] == "false"
    assert any(p == "package_placement_ranking" and e == "finished" and d["probes"] == 8
               for p, e, d in events)
    assert baseline.source == board and baseline.plane_stitch.pending_pads
    assert result == improve_package_access(baseline, fanout, options=options,
        placement_options=placement, global_options=global_options)


def test_standard_pipeline_uses_ranked_contact_placement_before_area():
    board, placement, fanout, global_options, options, _, signal = contact_distance_trap()
    # Seed zero applies a deterministic (-1 mm, +1 mm) jitter to the only
    # movable unit. Its nearest legal site is the same 7.5 mm / 7 mm trap.
    board = replace(board, placements=(replace(board.placements[0], position=Point.mm("8.5", 6)),
                                      *board.placements[1:]))
    # The trap relies on the source-position jitter, so satellite seeding
    # (which would start the unit at its connected pads) is off here.
    placement = replace(placement, candidate_count=1, analytical_iterations=0,
        refinement_passes=0, escape_spacing_passes=0,
        grid_step_nm=nm_from_mm("0.5"), legalization_candidates=1, satellite_seeding=False)
    result = run_routing_pipeline(board, placement_options=placement,
        global_options=global_options,
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1,
            preferred_candidate_id="candidate-00"),
        fanout_options=fanout, package_access_options=options,
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"), maximum_passes=1))
    assert result.package_access.ready
    assert result.package_access.accepted_moves == 1
    assert result.package_access.placement_probes
    assert next(n for n in result.detailed.nets if n.net == signal).connected
    assert not hard_findings(result.board)


@pytest.mark.parametrize("budget", (0, 1))
def test_probe_budget_cannot_expand_full_trial_budget_or_leak_copper(budget):
    board, placement, fanout, global_options, options, _, _ = contact_distance_trap()
    baseline = preflight_package_access(board, route_global(board, global_options), fanout, options=options)
    result = improve_package_access(baseline, fanout,
        options=replace(options, maximum_placement_probes=budget),
        placement_options=placement, global_options=global_options)
    assert len(result.placement_probes) == budget and len(result.trials) == 1
    assert result.board == baseline.board and result.source == board
    assert not result.accepted_moves and result.placement_stop_reason == "trial_budget_exhausted"


def test_total_probe_budget_is_not_reset_after_an_accepted_move():
    board, placement, fanout, global_options, options, _, _ = contact_distance_trap()
    names = {p.reference: f"SECOND_{p.reference}" for p in board.placements}
    def shifted(point):
        return Point(point.x_nm + nm_from_mm(16), point.y_nm)
    board = replace(board, outline=BoardOutline.rectangle(32, 16),
        placements=(*board.placements, *(replace(p, reference=names[p.reference], position=shifted(p.position))
            for p in board.placements)),
        nets=(*board.nets, *(replace(n, name=f"SECOND_{n.name}",
            pads=tuple(PadReference(names[p.component], p.pad) for p in n.pads)) for n in board.nets)),
        zones=(*board.zones, *(replace(z, id=f"second-{z.id}", net=f"SECOND_{z.net}",
            outline=PolygonWithHoles(PolygonRing(tuple(shifted(p) for p in z.outline.outer.vertices))))
            for z in board.zones)),
        placement_rules=(*board.placement_rules, *(replace(r, reference=names[r.reference])
            for r in board.placement_rules)))
    placement = replace(placement, fixed_references=placement.fixed_references |
                        {names[ref] for ref in placement.fixed_references})
    options = replace(options, maximum_trials=3)
    baseline = preflight_package_access(board, route_global(board, global_options), fanout, options=options)
    assert len(baseline.pending_pads) == 2
    result = improve_package_access(baseline, fanout, options=options,
        placement_options=placement, global_options=global_options)
    assert result.ready and result.accepted_moves == 2 and len(result.trials) == 3
    assert len(result.placement_probes) == options.maximum_placement_probes
    assert result.trials[0].placement_probe_index is not None
    assert all(t.placement_probe_index is None for t in result.trials[1:])
    assert [t.outcome for t in result.trials] == ["accepted", "access_not_improved", "accepted"]
    assert not hard_findings(result.board) and not result.source.tracks


@pytest.mark.parametrize("disabled", (False, True))
def test_fixed_or_disabled_placements_do_not_run_probes(disabled):
    board, placement, fanout, global_options, options, _, _ = contact_distance_trap()
    baseline = preflight_package_access(board, route_global(board, global_options), fanout, options=options)
    if disabled:
        options = replace(options, maximum_trials=0)
    else:
        placement = replace(placement, fixed_references=frozenset(p.reference for p in board.placements))
    result = improve_package_access(baseline, fanout, options=options,
        placement_options=placement, global_options=global_options)
    assert result.board == baseline.board and not result.trials and not result.placement_probes
    assert result.placement_stop_reason == ("disabled" if disabled else "no_legal_unseen_trials")
    if disabled:
        assert not result.placement_constraints
    else:
        assert result.placement_constraints
        assert any("temporary session lock" in reason
                   for item in result.placement_constraints for reason in item.reasons)


def test_named_region_and_orientation_domain_explain_rejected_placement():
    board, placement, fanout, global_options, options, moving, _ = contact_distance_trap()
    # The source pose fits exactly. Every bounded translation violates intent;
    # only the authored 0-degree orientation is legal, so no rotation is guessed.
    board = replace(board, regions=(PlacementRegion("tight-socket",
        BoardOutline.rectangle(2, 3, origin=Point.mm("6.5", "5.5"))),),
        placement_rules=(replace(board.placement_rules[0], region="tight-socket"),))
    baseline = preflight_package_access(board, route_global(board, global_options), fanout, options=options)
    result = improve_package_access(baseline, fanout, options=options,
        placement_options=placement, global_options=global_options)
    assert not result.trials and result.board == baseline.board
    assert result.placement_stop_reason == "no_legal_unseen_trials"
    assert any(item.kind == "orientation_domain" and "0 degrees" in item.reasons[0]
               for item in result.placement_constraints)
    assert any("tight-socket" in reason for item in result.placement_constraints for reason in item.reasons)
    assert len(result.placement_constraints) <= 32


@pytest.mark.parametrize("renamed", (False, True))
def test_local_contact_success_cannot_trade_away_a_critical_route(renamed):
    board, placement, fanout, global_options, options, _, signal = contact_distance_trap(renamed=renamed)
    board = replace(board, net_routing_rules=(NetRoutingRule(signal, RouteKind.CRITICAL,
        max_vias=0, allowed_layers=(CopperLayer.FRONT,), max_length_nm=nm_from_mm("5.5")),))
    baseline = preflight_package_access(board, route_global(board, global_options), fanout, options=options)
    assert not baseline.ready and not baseline.failed_critical_nets
    result = improve_package_access(baseline, fanout, options=options,
        placement_options=placement, global_options=global_options)
    chosen = result.placement_probes[result.trials[0].placement_probe_index - 1]
    assert not chosen.plane_pending_pads  # Local hint looks successful.
    assert result.trials[0].failed_critical_nets == (signal,)
    assert result.trials[0].outcome == "access_not_improved"
    assert result.board == baseline.board and result.source == board
    assert result.critical == baseline.critical and not result.accepted_moves


@pytest.mark.parametrize("fixed_companion", (False, True))
def test_ranked_rigid_unit_rotation_and_fixed_companion_gate(fixed_companion):
    board, placement, fanout, global_options, options, moving, _ = contact_distance_trap()
    companion = Placement("COMPANION", "synthetic-terminal", Point.mm("7.5", 4))
    cluster = RigidPlacementCluster("synthetic-unit", PlacementTarget(moving), (
        RigidPlacementMember(moving, board.placements[0].footprint,
            footprint_geometry_digest(board.footprints[board.placements[0].footprint]), Point(0, 0)),
        RigidPlacementMember(companion.reference, companion.footprint,
            footprint_geometry_digest(board.footprints[companion.footprint]), Point.mm(0, -3))),
        "Synthetic placement regression, not a reusable component", (0, 90))
    board = replace(board, placements=(*board.placements, companion), rigid_clusters=(cluster,),
        placement_rules=(replace(board.placement_rules[0], allowed_orientations=(0, 90)),))
    if fixed_companion:
        placement = replace(placement, fixed_references=placement.fixed_references | {companion.reference})
    options = replace(options, movement_nm=nm_from_mm("0.05"))
    baseline = preflight_package_access(board, route_global(board, global_options), fanout, options=options)
    assert not baseline.ready
    result = improve_package_access(baseline, fanout, options=options,
        placement_options=placement, global_options=global_options)
    if fixed_companion:
        assert not result.trials and not result.placement_probes
        assert result.source == board and result.placement_stop_reason == "no_legal_unseen_trials"
        assert 0 < len(result.placement_constraints) <= 32
        assert any("COMPANION: temporary session lock" in reason
                   for item in result.placement_constraints for reason in item.reasons)
        assert all(companion.reference in item.changed_references
                   for item in result.placement_constraints if item.kind == "rejected_pose")
    else:
        assert result.ready and result.accepted_moves == 1
        assert result.trials[0].changed_references == (moving, companion.reference)
        assert result.source.placements[0].position == board.placements[0].position
        assert result.source.placements[0].rotation_degrees == 90
        assert result.source.placements[-1].position == Point.mm("4.5", 7)
        assert result.source.placements[-1].rotation_degrees == 90
        assert result.source.placements[1:3] == board.placements[1:3]
        assert not hard_findings(result.board)


@pytest.mark.parametrize("renamed", (False, True))
def test_ranked_macro_move_regenerates_only_owned_geometry_at_new_pose(tmp_path, renamed):
    board, placement, fanout, global_options, options, moving, _ = contact_distance_trap(renamed=renamed)
    rail = board.nets[0].name
    companion = Placement("MACRO_PASSIVE", "synthetic-terminal", Point.mm("7.5", 4))
    board = replace(board, placements=(*board.placements, companion),
        nets=(replace(board.nets[0], pads=(*board.nets[0].pads, PadReference(companion.reference, "1"))),
              board.nets[1]),
        zones=(*board.zones, CopperZone("owned-contact", rail, (CopperLayer.INTERNAL_1,),
            PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in (
                ("6.95", "4.95"), ("7.05", "4.95"), ("7.05", "5.05"), ("6.95", "5.05"))))))))
    asset = dict(schema="copperlib-physical-hard-macro/v0.1",
        source="Synthetic routing ownership regression", production_publishable=False, anchor="device",
        members=[dict(reference=ref, footprint=fp, footprint_digest=footprint_geometry_digest(board.footprints[fp]),
            center_nm=center, rotation_degrees="0", edge_clearance_nm=250000)
            for ref, fp, center in (("device", board.placements[0].footprint, [0, 0]),
                                   ("passive", companion.footprint, [0, -3000000]))],
        pad_nets=[["device", "1", "rail"], ["device", "2", "signal"], ["passive", "1", "rail"]],
        isolated_pads=[], tracks=[dict(net="rail", width_nm=250000, layer="F.Cu",
            points=[{"pad": ["device", "1"]}, {"pad": ["passive", "1"]}]),
            dict(net="signal", width_nm=250000, layer="F.Cu",
                 points=[{"pad": ["device", "2"]}, [500000, 1000000]])],
        vias=[dict(net="rail", position_nm=[0, -2000000], size_nm=800000, drill_nm=400000,
                   from_layer="F.Cu", to_layer="B.Cu", technology=None)],
        ports=[dict(name="rail", net="rail", point={"pad": ["device", "1"]}, layer="F.Cu",
                    pads=[["device", "1"], ["passive", "1"]]),
               dict(name="signal", net="signal", point=[500000, 1000000], layer="F.Cu",
                    pads=[["device", "2"]])],
        protected_regions=[dict(id="private", layers=["F.Cu"],
            vertices=[[-400000, -3400000], [400000, -3400000], [400000, -1400000], [-400000, -1400000]],
            block_tracks=True, block_vias=True, block_zones=False)],
        keepouts=[], required_layers=["F.Cu", "B.Cu"], allowed_rotations=[0], internal_clearance_nm=0,
        unresolved=["Synthetic; not vendor geometry or hardware qualification"])
    raw = (json.dumps(asset, sort_keys=True) + "\n").encode()
    path = tmp_path / "synthetic-placement-owner.json"
    path.write_bytes(raw)
    board = bind_hard_macro(board, path, expected_sha256=sha256(raw).hexdigest(), name="owned-unit",
        bindings={"device": moving, "passive": companion.reference},
        net_bindings={"rail": rail, "signal": board.nets[1].name})
    initial_owner = materialize_hard_macros(board)
    assert not hard_findings(initial_owner)
    baseline = preflight_package_access(board, route_global(board, global_options), fanout, options=options)
    assert not baseline.ready and baseline.plane_stitch.pending_pads
    result = improve_package_access(baseline, fanout, options=options,
        placement_options=placement, global_options=global_options)
    assert result.ready and result.accepted_moves == 1
    assert result.trials[0].changed_references == (moving, companion.reference)
    assert result.source.placements[0].position == Point.mm(7, 7)
    assert result.source.placements[-1].position == Point.mm(7, 4)
    owner = materialize_hard_macros(result.source)
    assert Counter(owner.tracks) <= Counter(result.board.tracks)
    assert Counter(owner.vias) <= Counter(result.board.vias)
    assert owner.vias[0].position == Point.mm(7, 5)
    assert initial_owner.vias[0] not in result.board.vias
    assert result.source.hard_macros == board.hard_macros
    validate_hard_macros(result.board)
    assert not result.source.tracks and not hard_findings(result.board)
    assert baseline.source == board and not result.board.zone_fills


def test_probe_option_and_cli_bounds():
    from pcbir.cli import _parser
    for budget in (-1, "16", 0.5, True):
        with pytest.raises(ValueError, match="probe budget"):
            PackageAccessOptions(maximum_placement_probes=budget)
    parser = _parser()
    assert parser.parse_args(["route-board", "board.copper"]).package_placement_probes == 16
    assert parser.parse_args(["route-board", "board.copper", "--package-placement-probes", "0"]).package_placement_probes == 0
    with pytest.raises(SystemExit):
        parser.parse_args(["route-board", "board.copper", "--package-placement-probes", "-1"])


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("hint_fixed", (False, True))
def test_rejected_ordinary_owner_hints_do_not_unlock_fixed_endpoints_or_accept_failed_copper(renamed, hint_fixed):
    board, placement, fanout, global_options, options, moving, _ = contact_distance_trap(renamed=renamed)
    remote, feed = board.placements[2].reference, board.placements[1].reference
    placement = replace(placement, fixed_references={moving, feed} | ({remote} if hint_fixed else set()))
    baseline = preflight_package_access(board, route_global(board, global_options), fanout,
                                        options=replace(options, maximum_pattern_trials=0))
    assert not baseline.ready
    assert not tuple(package_placement_trials(baseline, placement, options.movement_nm))
    # Only the measured identity is a hint; no discarded candidate copper or
    # successful flags are installed by the generator.
    rejected = PackagePatternTrial(1, "critical_first", (PadReference(remote, "1"),),
                                   (), "lost_ordinary_access")
    hinted = replace(baseline, pattern_trials=(rejected,))
    explanations = []
    trials = tuple(package_placement_trials(hinted, placement, options.movement_nm,
                                           constraints=explanations))
    if hint_fixed:
        assert not trials
    else:
        assert trials and {reference for reference, _ in trials} == {remote}
        assert any(item.kind == "pattern_hint" and item.reference == remote for item in explanations)
        assert all(candidate.placements[:2] == board.placements[:2] for _, candidate in trials)
        result = improve_package_access(hinted, fanout,
            options=replace(options, maximum_trials=1, maximum_placement_probes=1),
            placement_options=placement, global_options=global_options)
        assert result.accepted_moves == 0 and result.board == baseline.board
        assert len(result.trials) <= 1 and len(result.placement_probes) <= 1
    assert hinted.source == board and hinted.board == baseline.board
