"""Deterministic, optional component-level SMD escape planning.

This is physical copper, not electrical intent. Each escape is checked as a
track/via pair before reservation; unsuccessful pads are reported explicitly.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from types import MappingProxyType
from typing import Mapping

from .drc import placed_pad_shape, run_physical_drc
from .geometry import point_in_polygon
from .physical import (
    BoardSide, CopperLayer, PadKind, PadReference, PhysicalBoard, Point,
    RouteKind, TrackSegment, Via, nm_from_mm,
)
from .placement import transformed_pad_position
from .routing_clearance import RoutingClearanceIndex
from .routing_vias import physical_via_span


@dataclass(frozen=True, slots=True)
class FanoutOptions:
    minimum_component_pads: int = 12
    maximum_neighbor_distance_nm: int = nm_from_mm("1.5")
    step_nm: int = nm_from_mm("0.5")
    maximum_radius_nm: int = nm_from_mm("3")

    def __post_init__(self) -> None:
        if min(self.minimum_component_pads, self.maximum_neighbor_distance_nm,
               self.step_nm, self.maximum_radius_nm) <= 0:
            raise ValueError("fanout options must be positive")


@dataclass(frozen=True, slots=True)
class FanoutResult:
    board: PhysicalBoard
    accesses: Mapping[PadReference, Point]
    pending_pads: tuple[PadReference, ...]
    added_track_count: int
    added_via_count: int
    created_vias: tuple[Via, ...] = ()


def route_fanout(
    board: PhysicalBoard, options: FanoutOptions | None = None,
) -> FanoutResult:
    """Escape only crowded ordinary-net SMD pads to legal through-vias."""

    options = options or FanoutOptions()
    if len(board.stackup.copper_layers) < 2:
        return FanoutResult(board, MappingProxyType({}), (), 0, 0)
    clearance = RoutingClearanceIndex(board)
    net_by_pad = {pad: net.name for net in board.nets for pad in net.pads
                  if len(net.pads) >= 2}
    rules = {item.net: item for item in board.net_routing_rules}
    zone_nets = {zone.net for zone in board.zones}
    pads: list[tuple[int, PadReference, Point, object, object]] = []
    for placement in board.placements:
        footprint = board.footprints[placement.footprint]
        if len(footprint.pads) < options.minimum_component_pads:
            continue
        surface = [pad for pad in footprint.pads if pad.kind is PadKind.SMD]
        for pad in surface:
            reference = PadReference(placement.reference, pad.number)
            net = net_by_pad.get(reference)
            if net is None or net in zone_nets:
                continue
            rule = rules.get(net)
            if rule is not None and rule.kind is not RouteKind.GENERAL:
                continue
            position = transformed_pad_position(board, placement, pad.number)
            neighbor = min((abs(position.x_nm - other.x_nm) + abs(position.y_nm - other.y_nm)
                            for other_pad in surface if other_pad.number != pad.number
                            for other in (transformed_pad_position(
                                board, placement, other_pad.number),)), default=10**18)
            if neighbor > options.maximum_neighbor_distance_nm:
                continue
            pads.append((neighbor, reference, position, placement, pad))

    tracks: list[TrackSegment] = []
    vias: list[Via] = []
    accesses: dict[PadReference, Point] = {}
    pending: list[PadReference] = []
    via_size = board.rules.default_via_size_nm
    via_drill = board.rules.default_via_drill_nm
    for _, reference, position, placement, pad in sorted(
        pads, key=lambda item: (item[0], item[1])
    ):
        net = net_by_pad[reference]
        side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
        rule = rules.get(net)
        allowed = rule.allowed_layers if rule and rule.allowed_layers else board.stackup.copper_layers
        if side not in allowed or len(allowed) < 2:
            pending.append(reference)
            continue
        other_layer = next((layer for layer in allowed if layer is not side), None)
        span = physical_via_span(board, side, other_layer) if other_layer else None
        if span is None:
            pending.append(reference)
            continue
        width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
        bounds = placed_pad_shape(position, pad, placement).bounds
        choice: tuple[TrackSegment, Via | None] | None = None
        for candidate in _candidates(position, placement.position, options):
            margin = via_size // 2 + board.rules.minimum_clearance_nm
            if not point_in_polygon(candidate, board.outline.vertices):
                continue
            if _distance_to_outline(candidate, board.outline.vertices) < margin:
                continue
            # Never silently require an unqualified via-in-pad process.
            if (bounds.min_x - via_size // 2 <= candidate.x_nm <= bounds.max_x + via_size // 2
                    and bounds.min_y - via_size // 2 <= candidate.y_nm <= bounds.max_y + via_size // 2):
                continue
            track = TrackSegment(net, position, candidate, width, side)
            coincident = tuple(item for item in (*board.vias, *vias)
                               if item.position == candidate)
            existing = next((item for item in coincident
                             if item.net == net and item.from_layer == span[0]
                             and item.to_layer == span[1]), None)
            if coincident and existing is None:
                continue
            via = None if existing else Via(net, candidate, via_size, via_drill, *span)
            if (clearance.can_track(net, track.start, track.end, width, side)
                    and (existing is not None or clearance.can_via(
                        net, candidate, via_size, span[0], span[1]))):
                choice = track, via
                break
        if choice is None:
            pending.append(reference)
            continue
        track, via = choice
        tracks.append(track)
        accesses[reference] = track.end
        clearance.add_track(track, locked=True)
        if via is not None:
            vias.append(via)
            clearance.add_via(via, locked=True)

    if not tracks:
        return FanoutResult(board, MappingProxyType({}), tuple(pending), 0, 0)
    routed = replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias))
    # Geometry queries are the fast gate; native DRC is the final transactional
    # gate. A new manufacturing violation rejects the whole fanout proposal.
    fatal = {"DRC-SHORT", "DRC-CLEARANCE", "DRC-BOARD-EDGE", "DRC-HOLE-CLEARANCE",
             "DRC-DRILL-SPACING", "DRC-COPPER-KEEPOUT", "DRC-VIA-SPAN",
             "DRC-TRACK-WIDTH"}
    before = run_physical_drc(board)
    after = run_physical_drc(routed)
    before_count = {code: sum(item.code == code for item in before.findings) for code in fatal}
    if any(sum(item.code == code for item in after.findings) > before_count[code]
           for code in fatal):
        return FanoutResult(board, MappingProxyType({}),
                            tuple(item[1] for item in pads), 0, 0)
    return FanoutResult(routed, MappingProxyType(accesses), tuple(pending),
                        len(tracks), len(vias), tuple(vias))


def _candidates(position: Point, center: Point, options: FanoutOptions):
    directions = ((1, 0), (-1, 0), (0, 1), (0, -1),
                  (1, 1), (1, -1), (-1, 1), (-1, -1))
    directions = sorted(directions, key=lambda pair: (
        -(pair[0] * (position.x_nm - center.x_nm)
          + pair[1] * (position.y_nm - center.y_nm)), pair,
    ))
    for step in range(1, options.maximum_radius_nm // options.step_nm + 1):
        for dx, dy in directions:
            yield Point(position.x_nm + dx * step * options.step_nm,
                        position.y_nm + dy * step * options.step_nm)


def _distance_to_outline(point: Point, vertices: tuple[Point, ...]) -> float:
    from .geometry import point_segment_distance_squared
    return min(float(point_segment_distance_squared(point, first, second)) ** 0.5
               for first, second in zip(vertices, (*vertices[1:], vertices[0])))
