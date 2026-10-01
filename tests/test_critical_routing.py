from __future__ import annotations

from dataclasses import replace
from hashlib import sha256
import pytest

from pcbir import (
    BoardOutline,
    CriticalRoutingStatus,
    CopperLayer,
    FootprintPad,
    GlobalRouterOptions,
    GlobalViaProposal,
    NetRoutingRule,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    RouteKind,
    Size,
    Stackup,
    nm_from_mm,
    route_critical_nets,
    route_global,
    TrackSegment, CopperKeepout, PolygonRing, PolygonWithHoles,
    DetailedRouterOptions, route_detailed, run_physical_drc,
)
from pcbir.critical import _route_pair, _tune_pair


def _pair_board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/two-pad-pair",
        (
            FootprintPad("1", Point.mm(0, "-0.5"), Size.mm("0.4", "0.4")),
            FootprintPad("2", Point.mm(0, "0.5"), Size.mm("0.4", "0.4")),
        ),
        Size.mm(1, 2),
    )
    return PhysicalBoard(
        "CriticalPair",
        BoardOutline.rectangle(40, 25),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(5, 12)),
            Placement("J2", footprint.name, Point.mm(35, 12)),
        ),
        (
            PhysicalNet("USB_DP", (PadReference("J1", "1"), PadReference("J2", "1"))),
            PhysicalNet("USB_DM", (PadReference("J1", "2"), PadReference("J2", "2"))),
        ),
        net_routing_rules=(
            NetRoutingRule(
                "USB_DP",
                RouteKind.DIFFERENTIAL,
                priority=100,
                width_nm=nm_from_mm("0.25"),
                differential_partner="USB_DM",
                pair_gap_nm=nm_from_mm("0.2"),
                max_skew_nm=nm_from_mm("10"),
                maximum_uncoupled_length_nm=nm_from_mm("10"),
                target_impedance_ohms=90,
            ),
            NetRoutingRule(
                "USB_DM",
                RouteKind.DIFFERENTIAL,
                priority=100,
                width_nm=nm_from_mm("0.25"),
                differential_partner="USB_DP",
                pair_gap_nm=nm_from_mm("0.2"),
                max_skew_nm=nm_from_mm("10"),
                maximum_uncoupled_length_nm=nm_from_mm("10"),
                target_impedance_ohms=90,
            ),
        ),
    )


def test_critical_pair_is_routed_as_locked_exact_copper() -> None:
    board = _pair_board()
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))

    first = route_critical_nets(board, guides)
    second = route_critical_nets(board, guides)

    assert first == second
    assert first.status is CriticalRoutingStatus.WARNING
    assert len(first.nets) == 1
    assert first.nets[0].connected
    assert first.nets[0].strategy == "aligned_pair"
    assert first.nets[0].nets == ("USB_DM", "USB_DP")
    assert first.locked_tracks
    assert first.nets[0].coupled_length_nm > 0
    assert len(first.nets[0].uncoupled_lengths_nm) == 2
    assert first.board.tracks == first.locked_tracks
    assert first.board.metadata["detailed_routing"] == "partial"
    assert "field-solver" in first.nets[0].assumptions[0]
    assert not {finding.code for finding in run_physical_drc(first.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET",
    }
    assert all(track.start.x_nm == track.end.x_nm or track.start.y_nm == track.end.y_nm
               or abs(track.start.x_nm - track.end.x_nm) == abs(track.start.y_nm - track.end.y_nm)
               for track in first.locked_tracks)


def test_critical_single_net_enforces_length_budget() -> None:
    board = _pair_board()
    critical = replace(
        board,
        net_routing_rules=(
            NetRoutingRule(
                "USB_DP",
                RouteKind.CRITICAL,
                width_nm=nm_from_mm("0.25"),
                max_length_nm=nm_from_mm("1"),
            ),
        ),
    )
    guides = route_global(critical, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))

    result = route_critical_nets(critical, guides)

    assert result.status is CriticalRoutingStatus.FAILED
    assert "exceeds" in result.nets[0].diagnostics[0]


def test_pair_rules_must_be_symmetric() -> None:
    board = replace(_pair_board(), net_routing_rules=(_pair_board().net_routing_rules[0],))
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))

    result = route_critical_nets(board, guides)

    assert result.status is CriticalRoutingStatus.FAILED
    assert "symmetric" in result.nets[0].diagnostics[0]


def test_pair_enforces_measured_uncoupled_budget() -> None:
    board = _pair_board()
    rules = tuple(replace(rule, maximum_uncoupled_length_nm=nm_from_mm("0.5")) for rule in board.net_routing_rules)
    board = replace(board, net_routing_rules=rules)
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    result = route_critical_nets(board, guides)
    assert result.status is CriticalRoutingStatus.FAILED
    assert any("uncoupled length" in item for item in result.nets[0].diagnostics)
    assert result.locked_tracks == result.locked_vias == ()


def test_symmetric_pair_needs_no_tuning_and_binds_external_evidence() -> None:
    board = _pair_board()
    evidence = sha256(b"field solver report").hexdigest()
    rules = tuple(
        replace(rule, max_skew_nm=nm_from_mm("0.1"),
                maximum_uncoupled_length_nm=nm_from_mm("20"),
                tuning_amplitude_limit_nm=nm_from_mm("2"),
                impedance_evidence_digest=evidence)
        for rule in board.net_routing_rules
    )
    board = replace(board, net_routing_rules=rules)
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    result = route_critical_nets(board, guides)
    pair = result.nets[0]
    assert pair.connected
    assert pair.skew_nm <= nm_from_mm("0.1")
    assert pair.tuned_length_nm == 0
    assert pair.evidence_digests == (evidence,)
    assert not any("field-solver" in item for item in pair.assumptions)


def test_pair_rejects_overlapping_return_via_and_floating_transition() -> None:
    board = _pair_board()
    board = replace(
        board,
        nets=(*board.nets, PhysicalNet("GND", ())),
        net_routing_rules=tuple(
            replace(rule, require_return_vias=True, return_via_net="GND",
                    maximum_return_via_distance_nm=nm_from_mm("1"))
            for rule in board.net_routing_rules
        ),
    )
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    pair_name = min(rule.net for rule in board.net_routing_rules)
    routes = tuple(
        replace(route, vias=(GlobalViaProposal(route.net, Point.mm(20, 12),
                                               CopperLayer.FRONT,
                                               CopperLayer.BACK,
                                               "test-via"),))
        if route.net == pair_name else route
        for route in guides.routes
    )
    guides = replace(guides, routes=routes)
    proposed, _, proposed_vias = _route_pair(
        board, *board.net_routing_rules, {route.net: route for route in guides.routes},
    )
    assert proposed.return_via_count == 1
    assert sum(via.net == "GND" for via in proposed_vias) == 1
    result = route_critical_nets(board, guides)
    pair = result.nets[0]
    assert result.status is CriticalRoutingStatus.FAILED
    assert pair.candidate_rejected
    assert pair.return_via_count == 0
    assert any("DRC-SHORT" in item for item in pair.diagnostics)
    assert result.locked_tracks == result.locked_vias == ()


def test_tuning_is_bounded_but_requires_candidate_geometry_validation() -> None:
    first = [TrackSegment("A", Point.mm(5, 5), Point.mm(15, 5), nm_from_mm("0.25"), CopperLayer.FRONT)]
    second = [TrackSegment("B", Point.mm(5, 8), Point.mm(17, 8), nm_from_mm("0.25"), CopperLayer.FRONT)]
    assert _tune_pair(first, second, nm_from_mm("0.1"), nm_from_mm("0.4")) == nm_from_mm("0.8")
    assert max(track.end.y_nm for track in first) == nm_from_mm("5.4")


@pytest.mark.parametrize("obstacle", ("pad", "keepout", "edge", "pair_clearance"))
def test_pair_rejects_unsafe_exact_geometry_atomically(obstacle) -> None:
    board = _pair_board()
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    if obstacle == "pad":
        blocker = PhysicalFootprint(
            "blocker", (FootprintPad("1", Point(0, 0), Size.mm(1, 1)),), Size.mm(1, 1),
        )
        board = replace(board, footprints={**board.footprints, "blocker": blocker},
                        placements=(*board.placements, Placement("X", "blocker", Point.mm(20, 12))))
    elif obstacle == "keepout":
        wall = CopperKeepout("wall", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
            Point.mm(19, 10), Point.mm(21, 10), Point.mm(21, 14), Point.mm(19, 14),
        ))))
        board = replace(board, copper_keepouts=(wall,))
    elif obstacle == "edge":
        board = replace(board, outline=BoardOutline.rectangle(40, "12.3"))
    else:
        board = replace(board, net_routing_rules=tuple(
            replace(rule, pair_gap_nm=nm_from_mm("0.05")) for rule in board.net_routing_rules))
    result = route_critical_nets(board, guides)
    assert result.status is CriticalRoutingStatus.FAILED
    assert result.nets[0].candidate_rejected
    assert result.nets[0].diagnostics
    assert result.board.tracks == result.locked_tracks == ()
    assert result.board.vias == result.locked_vias == ()


def test_subset_repair_cannot_reroute_or_prune_a_locked_pair() -> None:
    board = _pair_board()
    guides = route_global(board)
    critical = route_critical_nets(board, guides)
    assert critical.nets[0].connected
    repaired = route_detailed(
        critical.board, guides, DetailedRouterOptions(maximum_passes=1),
        only_nets=frozenset({"USB_DP", "USB_DM"}),
        fanout_accesses={PadReference("J1", "1"): Point.mm(6, 12)},
    )
    assert repaired.nets == ()
    assert repaired.board.tracks == critical.locked_tracks
    assert repaired.board.vias == critical.locked_vias


def test_single_ended_critical_net_repairs_stale_guide_with_exact_clearance() -> None:
    base = _pair_board()
    board = replace(base, net_routing_rules=(NetRoutingRule(
        "USB_DP", RouteKind.CRITICAL, allowed_layers=(CopperLayer.FRONT,), max_vias=0,
    ),))
    guides = route_global(board)
    blocker = PhysicalFootprint(
        "blocker", (FootprintPad("1", Point(0, 0), Size.mm(2, 2)),), Size.mm(2, 2),
    )
    board = replace(board, footprints={**board.footprints, "blocker": blocker},
                    placements=(*board.placements, Placement("X", "blocker", Point.mm(20, "11.5"))))
    result = route_critical_nets(board, guides)
    assert result.nets[0].connected
    assert result.nets[0].strategy == "exact_single_net"
    assert result.locked_tracks
    assert result.locked_vias == ()
    assert not any(finding.code in {"DRC-SHORT", "DRC-CLEARANCE"}
                   for finding in run_physical_drc(result.board).findings)


def test_later_single_critical_route_cannot_cut_a_reserved_pair() -> None:
    base = _pair_board()
    terminal = PhysicalFootprint(
        "terminal", (FootprintPad("1", Point(0, 0), Size.mm("0.4", "0.4")),), Size.mm(1, 1),
    )
    board = replace(
        base, footprints={**base.footprints, "terminal": terminal},
        placements=(*base.placements, Placement("S1", "terminal", Point.mm(20, 5)),
                    Placement("S2", "terminal", Point.mm(20, 20))),
        nets=(*base.nets, PhysicalNet("CROSS", (PadReference("S1", "1"), PadReference("S2", "1")))),
        net_routing_rules=(*base.net_routing_rules, NetRoutingRule(
            "CROSS", RouteKind.CRITICAL, allowed_layers=(CopperLayer.FRONT,), max_vias=0)),
    )
    guides = route_global(board)
    result = route_critical_nets(board, guides)
    pair = route_critical_nets(base, route_global(base))
    assert result.nets[0].connected
    assert tuple(track for track in result.locked_tracks if track.net != "CROSS") == pair.locked_tracks
    assert not any(finding.code in {"DRC-SHORT", "DRC-CLEARANCE"}
                   for finding in run_physical_drc(result.board).findings)
    assert result.nets[1].strategy == "exact_single_net"


def test_critical_transition_without_special_technology_is_through_via() -> None:
    base = _pair_board()
    board = replace(
        base,
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )),
        net_routing_rules=(NetRoutingRule("USB_DP", RouteKind.CRITICAL),),
    )
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    guides = replace(guides, routes=tuple(
        replace(route, vias=(GlobalViaProposal(
            route.net, Point.mm(20, 12), CopperLayer.FRONT,
            CopperLayer.INTERNAL_1, "test-via",
        ),)) if route.net == "USB_DP" else route
        for route in guides.routes
    ))

    result = route_critical_nets(board, guides)

    assert result.locked_vias
    assert all(
        via.from_layer is CopperLayer.FRONT and via.to_layer is CopperLayer.BACK
        for via in result.locked_vias
    )
