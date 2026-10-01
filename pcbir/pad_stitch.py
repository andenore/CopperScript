"""Connect separate copper lands that represent one logical footprint pad.

KiCad reports disconnected same-number lands even when the electrical IR
correctly treats them as one pin. This conservative closure stage adds bounded,
DRC-checked surface tracks; blocked lands remain explicit pending work.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from .drc import explicit_copper_connectivity

from .physical import (
    BoardSide, CopperLayer, PadKind, PadReference, PhysicalBoard, Point,
    TrackSegment, nm_from_mm,
)
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .surface_path import surface_path


@dataclass(frozen=True, slots=True)
class DuplicatePadStitchResult:
    board: PhysicalBoard
    stitched: tuple[PadReference, ...]
    pending: tuple[PadReference, ...]
    added_track_count: int
    already_connected: tuple[PadReference, ...] = ()


def stitch_duplicate_pads(board: PhysicalBoard) -> DuplicatePadStitchResult:
    """Join disconnected SMD land groups; reuse any existing physical path.

    Connectivity may run through track interiors, pad edges, vias and other
    layers. No surface bridge is demanded when explicit copper already joins
    the lands. Zone-only contacts still require independent filled evidence.
    """

    net_by_pad = {
        reference: net.name for net in board.nets for reference in net.pads
    }
    rules = {rule.net: rule for rule in board.net_routing_rules}
    clearance = RoutingClearanceIndex(board)
    added: list[TrackSegment] = []
    stitched: list[PadReference] = []
    pending: list[PadReference] = []
    already_connected: list[PadReference] = []
    for placement in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[placement.footprint]
        numbers = sorted({pad.number for pad in footprint.pads if pad.number})
        for number in numbers:
            reference = PadReference(placement.reference, number)
            net = net_by_pad.get(reference)
            if net is None:
                continue
            lands = tuple((index, pad) for index, pad in enumerate(footprint.pads)
                          if pad.number == number and pad.kind not in {
                              PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE})
            if len(lands) < 2:
                continue
            current = replace(board, tracks=(*board.tracks, *added))
            graph = explicit_copper_connectivity(current, only_nets=frozenset({net}))
            if graph.pad_connected(reference):
                stitched.append(reference)
                already_connected.append(reference)
                continue
            if any(pad.kind is not PadKind.SMD for _, pad in lands):
                pending.append(reference)
                continue
            positions = {
                f"pad:{placement.reference}.{number}:{index}":
                    transformed_local_point(placement, pad.position)
                for index, pad in lands
            }
            side = (
                CopperLayer.FRONT if placement.side is BoardSide.FRONT
                else CopperLayer.BACK
            )
            rule = rules.get(net)
            if rule and rule.allowed_layers and side not in rule.allowed_layers:
                pending.append(reference)
                continue
            width = (
                rule.width_nm if rule and rule.width_nm is not None
                else board.rules.default_track_width_nm
            )
            width = max(width, board.rules.minimum_track_width_nm)
            local_tracks: list[TrackSegment] = []
            while not graph.pad_connected(reference):
                groups: dict[str, list[Point]] = {}
                for node, position in positions.items():
                    groups.setdefault(graph.roots[node], []).append(position)
                proposals = sorted(
                    (
                        (abs(start.x_nm - end.x_nm)
                         + abs(start.y_nm - end.y_nm), start, end)
                        for first, starts in groups.items()
                        for second, ends in groups.items() if first < second
                        for start in starts for end in ends
                    ),
                    key=lambda item: (
                        item[0], item[1].x_nm, item[1].y_nm,
                        item[2].x_nm, item[2].y_nm,
                    ),
                )
                chosen = None
                for _, start, end in proposals:
                    path = surface_path(
                        board, clearance, net, start, end, width, side,
                        (*board.tracks, *added, *local_tracks),
                        maximum_detour_nm=nm_from_mm(3),
                    )
                    if path:
                        candidate = replace(board, tracks=(
                            *board.tracks, *added, *local_tracks, *path))
                        candidate_graph = explicit_copper_connectivity(
                            candidate, only_nets=frozenset({net}))
                        if len({candidate_graph.roots[node] for node in positions}) < len(groups):
                            chosen = path, candidate_graph
                            break
                if chosen is None:
                    pending.append(reference)
                    added.extend(local_tracks)
                    break
                tracks, graph = chosen
                for track in tracks:
                    local_tracks.append(track)
                    clearance.add_track(track)
            else:
                stitched.append(reference)
                added.extend(local_tracks)
    routed = replace(board, tracks=(*board.tracks, *added)) if added else board
    return DuplicatePadStitchResult(
        routed, tuple(stitched), tuple(pending), len(added), tuple(already_connected),
    )
