"""Conservative surface-pad escapes toward declared inner copper zones.

This stage makes only tracks and vias. It does not invent a zone fill or claim
that the resulting vias are joined by copper. A pinned refill and connected-
copper check remain necessary before a zone net can pass signoff.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from types import MappingProxyType
from typing import Iterator

from .drc import placed_pad_shape
from .physical import (
    BoardSide,
    CopperLayer,
    CopperZone,
    FootprintPad,
    PadKind,
    PadReference,
    PhysicalBoard,
    Placement,
    Point,
    PolygonWithHoles,
    TrackSegment,
    Via,
    nm_from_mm,
)
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .surface_path import surface_path, via_inside_board


@dataclass(frozen=True, slots=True)
class PlaneStitchOptions:
    step_nm: int = nm_from_mm("0.5")
    maximum_radius_nm: int = nm_from_mm("3")
    only_pads: frozenset[PadReference] | None = None

    def __post_init__(self) -> None:
        if self.only_pads is not None:
            object.__setattr__(self, "only_pads", frozenset(self.only_pads))
        if self.step_nm <= 0 or self.maximum_radius_nm < self.step_nm:
            raise ValueError("plane-stitch step and radius must be positive")


@dataclass(frozen=True, slots=True)
class PlaneStitchResult:
    board: PhysicalBoard
    stitched_pads: tuple[PadReference, ...]
    pending_pads: tuple[PadReference, ...]
    added_track_count: int
    added_via_count: int

    @property
    def complete(self) -> bool:
        """All pads reach a prospective plane contact, not verified zone fill."""
        return not self.pending_pads


def stitch_zone_pads(
    board: PhysicalBoard, options: PlaneStitchOptions | None = None,
) -> PlaneStitchResult:
    """Escape SMD pads to legal through-vias inside their declared zone."""

    options = options or PlaneStitchOptions()
    if not board.zones:
        return PlaneStitchResult(board, (), (), 0, 0)
    clearance = RoutingClearanceIndex(board)
    placements = {item.reference: item for item in board.placements}
    net_pads = {net.name: net.pads for net in board.nets}
    rules = {rule.net: rule for rule in board.net_routing_rules}
    added_tracks: list[TrackSegment] = []
    added_vias: list[Via] = []
    stitched: list[PadReference] = []
    pending: list[PadReference] = []
    via_size = board.rules.default_via_size_nm
    via_drill = board.rules.default_via_drill_nm
    outer_layers = (board.stackup.copper_layers[0], board.stackup.copper_layers[-1])

    zones_by_net: dict[str, list[CopperZone]] = {}
    for zone in sorted(board.zones, key=lambda item: item.id):
        if any(layer not in outer_layers for layer in zone.layers):
            zones_by_net.setdefault(zone.net, []).append(zone)
    for net, zones in sorted(zones_by_net.items()):
        for reference in sorted(net_pads[net]):
            if options.only_pads is not None and reference not in options.only_pads:
                continue
            placement = placements[reference.component]
            footprint = board.footprints[placement.footprint]
            lands = tuple(
                item for item in footprint.pads
                if item.number == reference.pad and item.kind is PadKind.SMD
            )
            if not lands:
                continue
            side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
            rule = rules.get(net)
            width = (rule.width_nm if rule and rule.width_nm is not None
                     else board.rules.default_track_width_nm)
            all_lands_stitched = True
            for pad in lands:
                position = transformed_local_point(placement, pad.position)
                choice = _stitch_land(
                    board, clearance, net, zones, pad, placement, position,
                    side, width, via_size, via_drill, outer_layers, options,
                    (*board.tracks, *added_tracks), (*board.vias, *added_vias),
                )
                if choice is None:
                    all_lands_stitched = False
                    continue
                tracks, via = choice
                for track in tracks:
                    added_tracks.append(track)
                    clearance.add_track(track)
                if via is not None:
                    added_vias.append(via)
                    clearance.add_via(via)
            (stitched if all_lands_stitched else pending).append(reference)

    metadata = dict(board.metadata)
    metadata["plane_stitching"] = "partial" if pending else "pad-escapes-only"
    metadata["fabrication_ready"] = "false"
    routed = replace(
        board,
        tracks=(*board.tracks, *added_tracks),
        vias=(*board.vias, *added_vias),
        metadata=MappingProxyType(metadata),
    )
    return PlaneStitchResult(
        routed, tuple(stitched), tuple(pending), len(added_tracks), len(added_vias),
    )


def _stitch_land(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
    zones: list[CopperZone], pad: FootprintPad, placement: Placement, position: Point,
    side: CopperLayer, width: int, via_size: int, via_drill: int,
    outer_layers: tuple[CopperLayer, CopperLayer], options: PlaneStitchOptions,
    committed_tracks: tuple[TrackSegment, ...], committed_vias: tuple[Via, ...],
) -> tuple[tuple[TrackSegment, ...], Via | None] | None:
    """Find one physical land's provisional contact to an inner zone."""

    # Existing fanout and routed vias need not lie on the half-mm grid.
    reusable = sorted(
        (
            via for via in committed_vias
            if via.net == net
            and via.from_layer == outer_layers[0]
            and via.to_layer == outer_layers[1]
            and any(_point_in_zone(via.position, zone.outline) for zone in zones)
            and (via.position.x_nm - position.x_nm) ** 2
                + (via.position.y_nm - position.y_nm) ** 2
                <= options.maximum_radius_nm ** 2
        ),
        key=lambda via: (
            (via.position.x_nm - position.x_nm) ** 2
            + (via.position.y_nm - position.y_nm) ** 2,
            via.position.x_nm, via.position.y_nm,
        ),
    )
    for existing in reusable:
        path = surface_path(
            board, clearance, net, position, existing.position, width, side,
            committed_tracks,
        )
        if path is not None:
            return path, None
    pad_bounds = placed_pad_shape(position, pad, placement).bounds
    for candidate in _candidate_points(position, options):
        if not _point_in_outline(candidate, board.outline.vertices):
            continue
        if not any(_point_in_zone(candidate, zone.outline) for zone in zones):
            continue
        # Through-via-in-pad needs a separately qualified filled/capped
        # assembly process; this prototype always escapes the land.
        if (
            pad_bounds.min_x - via_size // 2 <= candidate.x_nm <= pad_bounds.max_x + via_size // 2
            and pad_bounds.min_y - via_size // 2 <= candidate.y_nm <= pad_bounds.max_y + via_size // 2
        ):
            continue
        coincident = tuple(item for item in committed_vias if item.position == candidate)
        existing = next((item for item in coincident
                         if item.net == net
                         and item.from_layer == outer_layers[0]
                         and item.to_layer == outer_layers[1]), None)
        if coincident and existing is None:
            continue
        via = None if existing else Via(
            net, candidate, via_size, via_drill, outer_layers[0], outer_layers[1],
        )
        if existing is None and not via_inside_board(board, candidate, via_size):
            continue
        if existing is None and not clearance.can_via(
            net, candidate, via_size, via.from_layer, via.to_layer,
        ):
            continue
        path = surface_path(
            board, clearance, net, position, candidate, width, side,
            committed_tracks,
        )
        if path is not None:
            return path, via
    return None


def _candidate_points(position: Point, options: PlaneStitchOptions) -> Iterator[Point]:
    steps = options.maximum_radius_nm // options.step_nm
    for radius in range(1, steps + 1):
        offsets = (
            (dx, dy)
            for dx in range(-radius, radius + 1)
            for dy in range(-radius, radius + 1)
            if max(abs(dx), abs(dy)) == radius
        )
        for dx, dy in sorted(offsets, key=lambda item: (
            item[0] * item[0] + item[1] * item[1], item,
        )):
            yield Point(
                position.x_nm + dx * options.step_nm,
                position.y_nm + dy * options.step_nm,
            )


def _point_in_zone(point: Point, outline: PolygonWithHoles) -> bool:
    return (
        _point_in_outline(point, outline.outer.vertices)
        and not any(_point_in_outline(point, hole.vertices) for hole in outline.holes)
    )


def _point_in_outline(point: Point, polygon: tuple[Point, ...]) -> bool:
    inside = False
    for index, first in enumerate(polygon):
        second = polygon[(index + 1) % len(polygon)]
        if (first.y_nm > point.y_nm) != (second.y_nm > point.y_nm):
            crossing = (
                (second.x_nm - first.x_nm) * (point.y_nm - first.y_nm)
                / (second.y_nm - first.y_nm) + first.x_nm
            )
            if point.x_nm < crossing:
                inside = not inside
    return inside
