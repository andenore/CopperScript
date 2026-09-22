from __future__ import annotations

from dataclasses import replace
import json

from pcbir import (
    BoardOutline,
    CriticalRoutingStatus,
    DetailedRouterOptions,
    DetailedRoutingStatus,
    FootprintPad,
    GlobalRouterOptions,
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
    route_critical_nets,
    route_detailed,
    route_global,
)


def _board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    return PhysicalBoard(
        "DetailedRoute",
        BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(3, 6)),
            Placement("J2", footprint.name, Point.mm(17, 6)),
        ),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
    )


def test_detailed_router_materializes_deterministic_exact_copper() -> None:
    board = _board()
    global_route = route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2"))
    )
    options = DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"))

    first = route_detailed(board, global_route, options)
    second = route_detailed(board, global_route, options)

    assert first == second
    assert first.status is DetailedRoutingStatus.SUCCESS
    assert first.board.tracks
    assert first.board.metadata["detailed_routing"] == "complete"
    assert first.board.vias == ()
    assert any(
        track.start.x_nm != track.end.x_nm and track.start.y_nm != track.end.y_nm
        for track in first.board.tracks
    ) or len(first.board.tracks) < 28
    assert json.loads(first.to_json())["schema"] == "copperscript-detailed-route/v0.1"


def test_detailed_router_preserves_critical_copper() -> None:
    board = replace(
        _board(),
        net_routing_rules=(NetRoutingRule("SIGNAL", RouteKind.CRITICAL),),
    )
    global_route = route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2"))
    )
    critical = route_critical_nets(board, global_route)

    assert critical.status is not CriticalRoutingStatus.FAILED
    result = route_detailed(critical.board, global_route)

    assert result.status is DetailedRoutingStatus.SUCCESS
    assert result.locked_track_count == len(critical.board.tracks)
    assert result.board.tracks == critical.board.tracks


def test_detailed_router_fails_closed_without_a_connected_guide() -> None:
    board = replace(
        _board(),
        keepouts=(
            PlacementKeepout(
                "wall", BoardOutline.rectangle(4, 12, origin=Point.mm(8, 0))
            ),
        ),
    )
    global_route = route_global(
        board,
        GlobalRouterOptions(
            tile_size_nm=nm_from_mm("2"), maximum_iterations=2
        ),
    )

    result = route_detailed(board, global_route)

    assert result.status is DetailedRoutingStatus.PARTIAL
    assert result.metrics.unrouted_net_count == 1
    assert "missing connected global guide" in result.nets[0].diagnostics[0]
    assert result.board.metadata["fabrication_ready"] == "false"
