"""Routing-layer policy shared by global, fanout, and detailed routers.

An inner copper zone covering most of the board is treated as a dedicated
plane for its net. Through-vias may cross it, but foreign-net tracks may not
split it into narrow islands. Small/local pours and surface zones do not
reserve an entire routing layer.
"""

from __future__ import annotations

from .physical import CopperLayer, NetRoutingRule, PhysicalBoard, Point


def _area_twice(vertices: tuple[Point, ...]) -> int:
    return abs(sum(
        first.x_nm * second.y_nm - second.x_nm * first.y_nm
        for first, second in zip(vertices, (*vertices[1:], vertices[0]))
    ))


def dedicated_plane_layers(board: PhysicalBoard) -> dict[CopperLayer, str]:
    """Find unambiguous, nearly board-wide inner-layer plane intentions."""

    board_area = _area_twice(board.outline.vertices)
    if not board_area:
        return {}
    result: dict[CopperLayer, str] = {}
    for layer in board.stackup.copper_layers[1:-1]:
        zones = tuple(zone for zone in board.zones if layer in zone.layers)
        if len({zone.net for zone in zones}) != 1:
            continue
        if any(
            not zone.outline.holes
            and 5 * _area_twice(zone.outline.outer.vertices) >= 4 * board_area
            for zone in zones
        ):
            result[layer] = zones[0].net
    return result


def routing_layers(
    board: PhysicalBoard, net: str, rule: NetRoutingRule | None = None,
) -> tuple[CopperLayer, ...]:
    """Return legal track layers, preserving explicit restrictions and planes."""

    selected = tuple(rule.allowed_layers) if rule and rule.allowed_layers else board.stackup.copper_layers
    planes = dedicated_plane_layers(board)
    return tuple(layer for layer in selected if layer not in planes or planes[layer] == net)
