from __future__ import annotations

from dataclasses import replace

from pcbir import (
    BoardOutline,
    CriticalRoutingStatus,
    FootprintPad,
    GlobalRouterOptions,
    NetRoutingRule,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    RouteKind,
    Size,
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
