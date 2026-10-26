from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperLayer, FootprintPad, GlobalRouterOptions,
                   NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
                   PhysicalNet, Placement, Point, Size, RouteKind, TrackSegment,
                   nm_from_mm, route_global, route_critical_nets)
from pcbir.physical import BoardSide, DecouplingLink, DesignRules
from pcbir.fanout import route_fanout
from pcbir.critical import CriticalRoutingStatus, _fingerprint
from pcbir.critical_reuse import prepare_single_critical_reuse, validate_single_critical_reuse
from pcbir.drc import explicit_copper_connectivity
from pcbir.bypass_links import verified_bypass_links as _verified_bypass_links


@pytest.fixture(scope="module")
def routed():
    terminal = PhysicalFootprint("terminal", (
        FootprintPad("1", Point(0, 0), Size.mm("0.4", "0.4")),), Size.mm(1, 1))
    capacitor = PhysicalFootprint("capacitor", (
        FootprintPad("1", Point(0, 0), Size.mm("0.4", "0.4")),
        FootprintPad("2", Point.mm(1, 0), Size.mm("0.4", "0.4"))), Size.mm(2, 1))
    board = PhysicalBoard("Reuse", BoardOutline.rectangle(20, 18),
        {terminal.name: terminal, capacitor.name: capacitor},
        (Placement("L", terminal.name, Point.mm(5, 8)),
         Placement("R", terminal.name, Point.mm(15, 8)),
         Placement("U", terminal.name, Point.mm(7, 10)),
         Placement("C", capacitor.name, Point.mm(7, 6)),
         Placement("BL", terminal.name, Point.mm(5, 14)),
         # BAD's far terminal is on the back and its profile forbids vias:
         # a hard failure (a length miss alone would only warn).
         Placement("BR", terminal.name, Point.mm(15, 14), side=BoardSide.BACK)),
        (PhysicalNet("CLK", (PadReference("L", "1"), PadReference("R", "1"))),
         PhysicalNet("VDD", (PadReference("U", "1"), PadReference("C", "1"))),
         PhysicalNet("GND", (PadReference("C", "2"),)),
         PhysicalNet("BAD", (PadReference("BL", "1"), PadReference("BR", "1")))),
        rules=DesignRules(minimum_track_width_nm=100000, minimum_clearance_nm=100000,
                          default_via_size_nm=450000, default_via_drill_nm=200000,
                          minimum_hole_clearance_nm=200000),
        net_routing_rules=(NetRoutingRule("CLK", RouteKind.CLOCK,
            width_nm=150000, allowed_layers=(CopperLayer.FRONT, CopperLayer.BACK), max_vias=2,
            max_length_nm=nm_from_mm(30)), NetRoutingRule("BAD", RouteKind.CLOCK,
            width_nm=150000, allowed_layers=(CopperLayer.FRONT,), max_vias=0,
            max_length_nm=nm_from_mm(3))),
        decoupling_links=(DecouplingLink(PadReference("C", "1"), PadReference("U", "1"),
                                       PadReference("C", "2"), "VDD", "GND"),))
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=1000000, maximum_iterations=1))
    ordinary = route_fanout(board, only_nets=frozenset({"VDD"}))
    donor = route_critical_nets(board, guides, reserved_accesses=ordinary)
    assert donor.status is CriticalRoutingStatus.FAILED
    assert next(n for n in donor.nets if n.nets == ("CLK",)).connected, donor.to_json()
    assert not next(n for n in donor.nets if n.nets == ("BAD",)).connected
    return board, guides, ordinary, donor


def changed_donor(donor, **kwargs):
    result = replace(donor, **kwargs)
    return replace(result, routing_fingerprint=_fingerprint(result.global_routing_fingerprint,
        result.locked_tracks, result.locked_vias, result.nets))


def test_failed_sibling_does_not_discard_verified_single_owner(routed):
    board, guides, ordinary, donor = routed
    prepared = prepare_single_critical_reuse(board, guides, donor, reserved_accesses=ordinary)
    assert prepared.rejection_reason == ""
    assert [c.net for c in prepared.candidates] == ["CLK"]
    candidate = prepared.candidates[0]
    result, tracks, vias = validate_single_critical_reuse(
        board, candidate, ordinary.board.tracks, ordinary.board.vias)
    assert result.connected and not result.diagnostics
    assert result.strategy == "revalidated_single_net"
    assert tracks == candidate.tracks and vias == candidate.vias
    combined = replace(board, tracks=(*ordinary.board.tracks, *tracks),
                       vias=(*ordinary.board.vias, *vias))
    assert explicit_copper_connectivity(combined).net_connected(board.nets[0])
    assert _verified_bypass_links(combined, {"VDD"}) == frozenset(board.decoupling_links)
    assert donor.board.tracks == donor.locked_tracks
    assert ordinary.board.tracks == (*board.tracks, *ordinary.created_tracks)


@pytest.mark.parametrize("change", ["global", "source", "fingerprint", "prefix", "prefix_count"])
def test_preparation_rejects_stale_source_guides_and_prefix(routed, change):
    board, guides, ordinary, donor = routed
    if change == "global":
        guides = replace(guides, routing_fingerprint="stale")
    elif change == "source":
        board = replace(board, rules=replace(board.rules, minimum_clearance_nm=300000))
    elif change == "fingerprint":
        donor = replace(donor, routing_fingerprint="stale")
    elif change == "prefix":
        ordinary = replace(ordinary, created_tracks=())
    else:
        donor = changed_donor(donor, reserved_track_count=donor.reserved_track_count + 1)
    result = prepare_single_critical_reuse(board, guides, donor, reserved_accesses=ordinary)
    assert not result.candidates
    assert result.rejection_reason


@pytest.mark.parametrize("change", ["count", "length", "macro", "returns", "ambiguous", "pair"])
def test_ambiguous_or_non_generated_owner_is_not_reused(routed, change):
    board, guides, ordinary, donor = routed
    clock = next(n for n in donor.nets if n.nets == ("CLK",))
    if change == "count":
        clock = replace(clock, track_count=clock.track_count + 1)
    elif change == "length":
        clock = replace(clock, lengths_nm=(clock.lengths_nm[0] + 1,))
    elif change == "macro":
        clock = replace(clock, strategy="immutable_hard_macro")
    elif change == "returns":
        clock = replace(clock, return_via_count=1)
    elif change == "pair":
        clock = replace(clock, nets=("CLK", "BAD"))
    nets = tuple(clock if n.nets == ("CLK",) else n for n in donor.nets)
    if change == "ambiguous":
        nets = (*nets, clock)
    altered = changed_donor(donor, nets=nets)
    result = prepare_single_critical_reuse(board, guides, altered, reserved_accesses=ordinary)
    assert not result.candidates


def test_current_earlier_copper_can_reject_prepared_candidate_atomically(routed):
    board, guides, ordinary, donor = routed
    candidate = prepare_single_critical_reuse(board, guides, donor,
        reserved_accesses=ordinary).candidates[0]
    # Deliberately cross the actual candidate; keep the ordinary prefix intact.
    segment = candidate.tracks[0]
    middle = Point((segment.start.x_nm + segment.end.x_nm) // 2,
                   (segment.start.y_nm + segment.end.y_nm) // 2)
    blocker = TrackSegment("BAD", Point(middle.x_nm - 500000, middle.y_nm - 500000),
                           Point(middle.x_nm + 500000, middle.y_nm + 500000),
                           200000, segment.layer)
    committed = (*ordinary.board.tracks, blocker)
    result, tracks, vias = validate_single_critical_reuse(board, candidate, committed,
                                                        ordinary.board.vias)
    assert not result.connected and result.candidate_rejected
    assert not tracks and not vias
    assert committed == (*ordinary.board.tracks, blocker)
    assert donor.locked_tracks == donor.board.tracks


def test_current_budget_or_geometry_change_invalidates_binding(routed):
    board, guides, ordinary, donor = routed
    candidate = prepare_single_critical_reuse(board, guides, donor,
        reserved_accesses=ordinary).candidates[0]
    tighter = replace(board, net_routing_rules=(
        replace(board.net_routing_rules[0], max_length_nm=1000000), board.net_routing_rules[1]))
    result, tracks, vias = validate_single_critical_reuse(tighter, candidate,
                                                        ordinary.board.tracks, ordinary.board.vias)
    assert not result.connected and not tracks and not vias
    moved = replace(candidate, tracks=tuple(replace(t, width_nm=t.width_nm + 1)
                                           for t in candidate.tracks))
    result, tracks, vias = validate_single_critical_reuse(board, moved,
                                                        ordinary.board.tracks, ordinary.board.vias)
    assert not result.connected and not tracks and not vias


def test_fresh_budget_measurement_rejects_over_limit_donor(routed):
    board, guides, ordinary, donor = routed
    # Even a structurally coherent donor with matching changed source fields
    # cannot turn its old connected flag into current budget evidence.
    tighter = replace(board, net_routing_rules=(
        replace(board.net_routing_rules[0], max_length_nm=1000000), board.net_routing_rules[1]))
    prefix = replace(ordinary, board=replace(ordinary.board, net_routing_rules=tighter.net_routing_rules))
    altered = changed_donor(donor, board=replace(donor.board, net_routing_rules=tighter.net_routing_rules))
    result = prepare_single_critical_reuse(tighter, guides, altered, reserved_accesses=prefix)
    assert not result.candidates
    assert "exceeds" in result.rejected[0][1]


def test_formal_api_retains_generated_clock_while_failed_sibling_routes_normally(routed):
    board, guides, ordinary, donor = routed
    retained = route_critical_nets(board, guides, retained_critical=donor,
                                   retained_accesses=ordinary)
    clock = next(n for n in retained.nets if n.nets == ("CLK",))
    assert donor.status is CriticalRoutingStatus.FAILED
    assert retained.status is CriticalRoutingStatus.FAILED
    assert clock.connected and clock.strategy == "revalidated_single_net"
    assert not next(n for n in retained.nets if n.nets == ("BAD",)).connected
    assert tuple(t for t in retained.locked_tracks if t.net == "CLK") == tuple(
        t for t in donor.locked_tracks if t.net == "CLK")
    # Clean routing deliberately lacks ordinary copper. Regeneration must
    # restore the real source feed, rather than accept only escaped pads.
    rebuilt = route_fanout(retained.board, only_nets=frozenset({"VDD"}))
    assert _verified_bypass_links(rebuilt.board, {"VDD"}) == frozenset(board.decoupling_links)
    assert not rebuilt.decoupling_pending
    assert board.tracks == () and board.vias == ()
    assert donor.board.tracks == donor.locked_tracks


def test_formal_api_preserves_exact_current_ordinary_prefix(routed):
    board, guides, ordinary, donor = routed
    retained = route_critical_nets(board, guides, reserved_accesses=ordinary,
                                   retained_critical=donor, retained_accesses=ordinary)
    assert retained.locked_tracks[:len(ordinary.board.tracks)] == ordinary.board.tracks
    assert retained.locked_vias[:len(ordinary.board.vias)] == ordinary.board.vias
    assert next(n for n in retained.nets if n.nets == ("CLK",)).strategy == "revalidated_single_net"
    assert _verified_bypass_links(retained.board, {"VDD"}) == frozenset(board.decoupling_links)


def test_formal_api_stale_donor_falls_back_without_adopting_copper(routed):
    board, guides, ordinary, donor = routed
    stale = replace(donor, routing_fingerprint="stale")
    result = route_critical_nets(board, guides, reserved_accesses=ordinary,
                                retained_critical=stale, retained_accesses=ordinary)
    clock = next(n for n in result.nets if n.nets == ("CLK",))
    assert clock.connected and clock.strategy != "revalidated_single_net"
    assert result.locked_tracks[:len(ordinary.board.tracks)] == ordinary.board.tracks
    assert _verified_bypass_links(result.board, {"VDD"}) == frozenset(board.decoupling_links)
    assert stale.board == donor.board
    assert stale.routing_fingerprint == "stale"


def test_generated_suffix_cannot_alias_original_prefix_occurrence(routed):
    board, guides, ordinary, donor = routed
    repeated = ordinary.created_tracks[0]
    tracks = (*donor.locked_tracks, repeated)
    altered = changed_donor(donor, locked_tracks=tracks,
                            board=replace(donor.board, tracks=tracks))
    result = prepare_single_critical_reuse(board, guides, altered, reserved_accesses=ordinary)
    assert not result.candidates
    assert "aliases" in result.rejection_reason


def test_source_owned_net_is_not_reclassified_as_generated(routed):
    board, guides, ordinary, donor = routed
    fixed = next(t for t in donor.locked_tracks if t.net == "CLK")
    source = replace(board, tracks=(fixed,))
    prefix = replace(ordinary, board=replace(ordinary.board, tracks=(fixed, *ordinary.board.tracks)))
    tracks = (fixed, *donor.locked_tracks)
    altered = changed_donor(donor, locked_tracks=tracks,
                            board=replace(donor.board, tracks=tracks))
    result = prepare_single_critical_reuse(source, guides, altered, reserved_accesses=prefix)
    assert not result.candidates
