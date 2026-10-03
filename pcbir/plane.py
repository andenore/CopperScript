"""Conservative surface-pad escapes toward declared copper zones.

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
from .surface_path import surface_path, surface_path_to_via, via_inside_board


@dataclass(frozen=True, slots=True)
class PlaneStitchOptions:
    step_nm: int = nm_from_mm("0.5")
    maximum_radius_nm: int = nm_from_mm("3")
    maximum_contact_radius_nm: int = 0
    maximum_detour_nm: int = 0
    maze_step_nm: int = nm_from_mm("0.1")
    maze_state_budget: int = 12_000
    candidate_bias: str | None = None
    escape_width_nm: int | None = None
    only_pads: frozenset[PadReference] | None = None
    ground_via_in_pad: bool = False
    # Opt-in opposite-side surface zones, e.g. a two-layer rear ground pour.
    # Same-side-only zones are refill intent, not a reason to invent a via.
    include_surface_zones: bool = False

    def __post_init__(self) -> None:
        if self.only_pads is not None:
            object.__setattr__(self, "only_pads", frozenset(self.only_pads))
        if self.candidate_bias not in {None, "east", "south", "west", "north"}:
            raise ValueError("plane-stitch candidate bias must be a cardinal direction")
        if (self.step_nm <= 0 or self.maximum_radius_nm < self.step_nm
                or self.maximum_contact_radius_nm < 0
                or self.maximum_detour_nm < 0
                or self.maze_step_nm <= 0 or self.maze_state_budget <= 0
                or (self.escape_width_nm is not None
                    and self.escape_width_nm < nm_from_mm("0.09"))):
            raise ValueError("plane-stitch search bounds or escape width are invalid")


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


@dataclass(frozen=True, slots=True)
class _PendingContact:
    net: str
    reference: PadReference
    position: Point
    side: CopperLayer
    width_nm: int


def stitch_zone_pads(
    board: PhysicalBoard, options: PlaneStitchOptions | None = None,
) -> PlaneStitchResult:
    """Escape SMD pads to legal through-vias inside their declared zone."""

    if board.hard_macros and not board.materialized_macros:
        raise ValueError("materialize hard macros before plane stitching")
    options = options or PlaneStitchOptions()
    if options.ground_via_in_pad and board.metadata.get("fabrication_profile") != "jlcpcb-six-layer":
        raise ValueError("filled/capped ground via-in-pad requires the JLCPCB six-layer profile")
    if (options.escape_width_nm is not None
            and options.escape_width_nm < max(
                board.rules.minimum_track_width_nm,
                board.rules.minimum_clearance_nm,
            )):
        raise ValueError("plane escape width is below the board rule floor")
    if not board.zones:
        return PlaneStitchResult(board, (), (), 0, 0)
    clearance = RoutingClearanceIndex(board)
    placements = {item.reference: item for item in board.placements}
    net_pads = {net.name: net.pads for net in board.nets}
    rules = {rule.net: rule for rule in board.net_routing_rules}
    added_tracks: list[TrackSegment] = []
    added_vias: list[Via] = []
    targets: list[PadReference] = []
    permitted_pads = {rule.pad for rule in board.via_in_pad_rules}
    pending_lands: list[_PendingContact] = []
    anchors_by_net: dict[str, list[tuple[Point, CopperLayer]]] = {}
    via_size = board.rules.default_via_size_nm
    via_drill = board.rules.default_via_drill_nm
    outer_layers = (board.stackup.copper_layers[0], board.stackup.copper_layers[-1])

    zones_by_net: dict[str, list[CopperZone]] = {}
    processed_internal_groups = set()
    from .hard_macros import macro_owned_pads
    from .drc import explicit_copper_connectivity
    owned_pads = macro_owned_pads(board)
    from .placement import resolved_copper_keepouts
    keepouts = resolved_copper_keepouts(board)
    owner_graph = explicit_copper_connectivity(board) if owned_pads else None
    owned_pending = set()
    for zone in sorted(board.zones, key=lambda item: item.id):
        if (any(layer not in outer_layers for layer in zone.layers)
                or (options.include_surface_zones and any(layer in outer_layers for layer in zone.layers))):
            zones_by_net.setdefault(zone.net, []).append(zone)
    for net, zones in sorted(zones_by_net.items()):
        for reference in sorted(net_pads[net]):
            if options.only_pads is not None and reference not in options.only_pads:
                continue
            placement = placements[reference.component]
            side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
            reference_zones = tuple(z for z in zones if any(layer is not side for layer in z.layers))
            if not reference_zones:
                continue  # A same-side pour needs refill, not an invented through-via.
            footprint = board.footprints[placement.footprint]
            lands = tuple(
                item for item in footprint.pads
                if item.number == reference.pad and item.kind is PadKind.SMD
            )
            if not lands:
                continue
            targets.append(reference)
            if reference in owned_pads:
                # Reuse actual immutable copper, never create a shortcut inside
                # the private region. This proves only a prospective plane
                # contact; external refill still has to prove filled copper.
                nodes = owner_graph.pad_nodes.get(reference, ())
                roots = {owner_graph.roots[node] for node in nodes}
                contacts = set()
                for i, via in enumerate(board.vias):
                    a, b = sorted((board.stackup.copper_layers.index(via.from_layer),
                                   board.stackup.copper_layers.index(via.to_layer)))
                    span = set(board.stackup.copper_layers[a:b+1])
                    if via.net == net and any(span.intersection(zone.layers)
                            and _point_in_zone(via.position, zone.outline)
                            and not any(k.block_zones and span.intersection(k.layers)
                                        and _point_in_zone(via.position, k.outline)
                                        for k in keepouts)
                            for zone in reference_zones):
                        contacts.add(owner_graph.roots[f"via:{i}"])
                if not roots or not roots.issubset(contacts):
                    owned_pending.add(reference)
                continue
            group = next((g for g in footprint.internal_pad_groups if reference.pad in g.numbers), None)
            if group is not None:
                key = (placement.reference, group.numbers)
                if key in processed_internal_groups:
                    continue
                processed_internal_groups.add(key)
                lands = tuple(p for p in footprint.pads if p.number in group.numbers and p.kind is PadKind.SMD)
            rule = rules.get(net)
            width = max(
                rule.width_nm if rule and rule.width_nm is not None else 0,
                options.escape_width_nm or board.rules.default_track_width_nm,
            )
            internal_contact = False
            for pad in lands:
                position = transformed_local_point(placement, pad.position)
                choice = _stitch_land(
                    board, clearance, net, reference_zones, pad, placement, position,
                    side, width, via_size, via_drill, outer_layers,
                    replace(options, ground_via_in_pad=True)
                    if PadReference(reference.component, pad.number) in permitted_pads else options,
                    (*board.tracks, *added_tracks), (*board.vias, *added_vias),
                )
                if choice is None:
                    if group is None:
                        pending_lands.append(_PendingContact(net, reference, position, side, width))
                    continue
                tracks, via = choice
                for track in tracks:
                    added_tracks.append(track)
                    clearance.add_track(track)
                if via is not None:
                    added_vias.append(via)
                    clearance.add_via(via)
                anchors_by_net.setdefault(net, []).append((position, side))
                internal_contact = True
                if group is not None:
                    break  # One verified prospective contact serves this group.
            if group is not None and not internal_contact and lands:
                pending_lands.append(_PendingContact(net, reference,
                    transformed_local_point(placement, lands[0].position), side, width))

    # A blocked land may still reach the plane through an already escaped
    # same-net land. Retry until no further local chain can be proven.
    unresolved = pending_lands
    while unresolved and options.maximum_contact_radius_nm:
        remaining: list[_PendingContact] = []
        for contact in unresolved:
            anchors = sorted(
                (
                    (anchor, side) for anchor, side in anchors_by_net.get(contact.net, ())
                    if side is contact.side
                    and (anchor.x_nm - contact.position.x_nm) ** 2
                        + (anchor.y_nm - contact.position.y_nm) ** 2
                        <= options.maximum_contact_radius_nm ** 2
                ),
                key=lambda item: (
                    (item[0].x_nm - contact.position.x_nm) ** 2
                    + (item[0].y_nm - contact.position.y_nm) ** 2,
                    item[0].x_nm, item[0].y_nm,
                ),
            )
            path = next((
                candidate for anchor, _ in anchors
                if (candidate := surface_path(
                    board, clearance, contact.net, contact.position, anchor,
                    contact.width_nm, contact.side, (*board.tracks, *added_tracks),
                    maximum_detour_nm=options.maximum_detour_nm,
                )) is not None
            ), None)
            if path is None:
                remaining.append(contact)
                continue
            for track in path:
                added_tracks.append(track)
                clearance.add_track(track)
            anchors_by_net.setdefault(contact.net, []).append(
                (contact.position, contact.side)
            )
        if len(remaining) == len(unresolved):
            break
        unresolved = remaining

    pending_refs = {item.reference for item in unresolved} | owned_pending
    # Report every alias of a failed internal group, not just its representative.
    for reference in tuple(pending_refs):
        footprint = board.footprints[placements[reference.component].footprint]
        for group in footprint.internal_pad_groups:
            if reference.pad in group.numbers:
                pending_refs.update(PadReference(reference.component, n) for n in group.numbers)
    stitched = [reference for reference in targets if reference not in pending_refs]
    pending = [reference for reference in targets if reference in pending_refs]

    metadata = dict(board.metadata)
    metadata["plane_stitching"] = "partial" if pending else "pad-escapes-only"
    metadata["fabrication_ready"] = "false"
    filled_count = sum(via.finish == "filled-capped" for via in added_vias)
    if filled_count:
        metadata["via_in_pad_process"] = "filled-capped"
        metadata["via_in_pad_count"] = str(
            int(metadata.get("via_in_pad_count", "0")) + filled_count
        )
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
            committed_tracks, maximum_detour_nm=options.maximum_detour_nm,
        )
        if path is not None:
            return path, None
    pad_bounds = placed_pad_shape(position, pad, placement).bounds
    legal_via_targets: list[Point] = []
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
            net, candidate, via_size, via.from_layer, via.to_layer, via_drill,
        ):
            continue
        if existing is None:
            legal_via_targets.append(candidate)
        path = surface_path(
            board, clearance, net, position, candidate, width, side,
            committed_tracks, maximum_detour_nm=options.maximum_detour_nm,
        )
        if path is not None:
            return path, via
    # A dense package may have no legal straight, elbow, or short lateral
    # escape. Search its small local neighborhood jointly over path and via
    # location before considering a qualified via in the SMD land itself.
    occupied = {via.position for via in committed_vias}
    maze = surface_path_to_via(
        board, clearance, net, position, width, side,
        step_nm=options.maze_step_nm,
        radius_nm=options.maximum_radius_nm,
        via_size_nm=via_size,
        via_drill_nm=via_drill,
        via_layers=outer_layers,
        via_targets=tuple(legal_via_targets),
        state_budget=options.maze_state_budget,
        accept_via=lambda candidate: (
            candidate not in occupied
            and any(_point_in_zone(candidate, zone.outline) for zone in zones)
            and not (
                pad_bounds.min_x - via_size // 2 <= candidate.x_nm
                <= pad_bounds.max_x + via_size // 2
                and pad_bounds.min_y - via_size // 2 <= candidate.y_nm
                <= pad_bounds.max_y + via_size // 2
            )
        ),
    )
    if maze is not None:
        tracks, candidate = maze
        return tracks, Via(
            net, candidate, via_size, via_drill, outer_layers[0], outer_layers[1],
        )
    if options.ground_via_in_pad and net == "GND":
        # A centered, plated-over-filled through via is the last resort. The
        # smaller drill is checked against actual holes, not the ordinary via
        # default; every copper layer and keepout is still checked exactly.
        size = nm_from_mm("0.30")
        drill = nm_from_mm("0.20")
        if (min(pad.size.width_nm, pad.size.height_nm) >= size
                and any(_point_in_zone(position, zone.outline) for zone in zones)
                and via_inside_board(board, position, size)
                and not any(via.position == position for via in committed_vias)
                and clearance.can_via(
                    net, position, size, outer_layers[0], outer_layers[1], drill,
                    check_hole_copper=True,
                    allow_pad_overlap=True,
                    allowed_pad=PadReference(placement.reference,pad.number),
                )):
            return (), Via(
                net, position, size, drill, outer_layers[0], outer_layers[1],
                finish="filled-capped",
            )
    return None


def _candidate_points(position: Point, options: PlaneStitchOptions) -> Iterator[Point]:
    steps = options.maximum_radius_nm // options.step_nm
    preferred = {
        "east": (1, 0), "south": (0, 1),
        "west": (-1, 0), "north": (0, -1),
    }.get(options.candidate_bias, (0, 0))
    for radius in range(1, steps + 1):
        offsets = (
            (dx, dy)
            for dx in range(-radius, radius + 1)
            for dy in range(-radius, radius + 1)
            if max(abs(dx), abs(dy)) == radius
        )
        for dx, dy in sorted(offsets, key=lambda item: (
            item[0] * item[0] + item[1] * item[1],
            -(item[0] * preferred[0] + item[1] * preferred[1]), item,
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
