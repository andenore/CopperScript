from __future__ import annotations

from dataclasses import replace
import json

import pytest

from pcbir import (
    BoardOutline,
    CopperLayer,
    FootprintPad,
    GlobalRouterOptions,
    GlobalRoutingStatus,
    NetRoutingRule,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    PlacementKeepout,
    Point,
    RouteKind,
    Size,
    nm_from_mm,
    route_global,
)


def _two_terminal_board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    return PhysicalBoard(
        "GlobalRoute",
        BoardOutline.rectangle(40, 30),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(5, 15)),
            Placement("J2", footprint.name, Point.mm(35, 15)),
        ),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
    )


def test_global_router_is_deterministic_and_produces_guides_not_copper() -> None:
    board = _two_terminal_board()
    options = GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"))

    first = route_global(board, options)
    second = route_global(board, options)

    assert first == second
    assert first.status is GlobalRoutingStatus.SUCCESS
    assert first.metrics.unrouted_net_count == 0
    assert first.metrics.total_overflow == 0
    assert first.routes[0].segments
    assert board.tracks == ()
    assert board.vias == ()
    assert json.loads(first.to_json())["schema"] == "copperscript-global-route/v0.1"


def test_global_router_reports_an_impossible_keepout_cut() -> None:
    board = replace(
        _two_terminal_board(),
        keepouts=(
            PlacementKeepout(
                "wall",
                BoardOutline.rectangle(10, 30, origin=Point.mm(15, 0)),
            ),
        ),
    )

    result = route_global(
        board,
        GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"), maximum_iterations=2),
    )

    assert result.status is GlobalRoutingStatus.UNREACHABLE
    assert result.metrics.unrouted_net_count == 1
    assert "no capacity-graph path" in result.routes[0].diagnostics[0]


def test_global_router_reports_physical_capacity_overflow() -> None:
    board = replace(
        _two_terminal_board(),
        net_routing_rules=(
            NetRoutingRule(
                "SIGNAL",
                RouteKind.POWER,
                width_nm=nm_from_mm("4.8"),
                clearance_nm=nm_from_mm("0.5"),
                allowed_layers=(CopperLayer.FRONT,),
                max_vias=0,
            ),
        ),
    )

    result = route_global(
        board,
        GlobalRouterOptions(tile_size_nm=nm_from_mm("5"), maximum_iterations=3),
    )

    assert result.status is GlobalRoutingStatus.OVERFLOW
    assert result.metrics.total_overflow > 0
    assert result.hotspots
    assert all(segment.layer is CopperLayer.FRONT for segment in result.routes[0].segments)
    assert result.routes[0].vias == ()


def test_routing_rules_validate_net_layer_and_partner_references() -> None:
    board = _two_terminal_board()

    with pytest.raises(ValueError, match="unknown net"):
        replace(board, net_routing_rules=(NetRoutingRule("MISSING"),))
    with pytest.raises(ValueError, match="unknown partner"):
        replace(
            board,
            net_routing_rules=(
                NetRoutingRule(
                    "SIGNAL",
                    RouteKind.DIFFERENTIAL,
                    differential_partner="MISSING",
                    pair_gap_nm=nm_from_mm("0.2"),
                ),
            ),
        )
