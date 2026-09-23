from __future__ import annotations

from dataclasses import replace
from hashlib import sha256

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
)


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
    assert first.nets[0].nets == ("USB_DM", "USB_DP")
    assert first.locked_tracks
    assert first.nets[0].coupled_length_nm > 0
    assert len(first.nets[0].uncoupled_lengths_nm) == 2
    assert first.board.tracks == first.locked_tracks
    assert first.board.metadata["detailed_routing"] == "partial"
    assert "field-solver" in first.nets[0].assumptions[0]


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
    rules = tuple(replace(rule, maximum_uncoupled_length_nm=nm_from_mm("1")) for rule in board.net_routing_rules)
    board = replace(board, net_routing_rules=rules)
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    result = route_critical_nets(board, guides)
    assert result.status is CriticalRoutingStatus.FAILED
    assert "uncoupled length" in result.nets[0].diagnostics[-1]


def test_pair_uses_bounded_local_tuning_and_binds_external_evidence() -> None:
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
    assert pair.tuned_length_nm > 0
    assert pair.evidence_digests == (evidence,)
    assert not any("field-solver" in item for item in pair.assumptions)


def test_pair_transition_generates_bounded_return_via() -> None:
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
    result = route_critical_nets(board, guides)
    pair = result.nets[0]
    assert pair.return_via_count == 1
    assert sum(via.net == "GND" for via in result.locked_vias) == 1


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
