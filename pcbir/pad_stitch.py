"""Connect separate copper lands that represent one logical footprint pad.

KiCad reports disconnected same-number lands even when the electrical IR
correctly treats them as one pin. This conservative closure stage adds only
straight, DRC-checked tracks; blocked lands remain explicit pending work.
"""

from __future__ import annotations

from dataclasses import dataclass, replace

from .physical import (
    BoardSide, CopperLayer, PadKind, PadReference, PhysicalBoard, Point,
    TrackSegment,
)
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex


@dataclass(frozen=True, slots=True)
class DuplicatePadStitchResult:
    board: PhysicalBoard
    stitched: tuple[PadReference, ...]
    pending: tuple[PadReference, ...]
    added_track_count: int


def stitch_duplicate_pads(board: PhysicalBoard) -> DuplicatePadStitchResult:
    """Bridge same-number SMD lands where exact clearance allows it."""

    net_by_pad = {
        reference: net.name for net in board.nets for reference in net.pads
    }
    rules = {rule.net: rule for rule in board.net_routing_rules}
    clearance = RoutingClearanceIndex(board)
    added: list[TrackSegment] = []
    stitched: list[PadReference] = []
    pending: list[PadReference] = []
    for placement in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[placement.footprint]
        numbers = sorted({pad.number for pad in footprint.pads if pad.number})
        for number in numbers:
            reference = PadReference(placement.reference, number)
            net = net_by_pad.get(reference)
            if net is None:
                continue
            lands = tuple(pad for pad in footprint.pads if pad.number == number)
            if len(lands) < 2:
                continue
            if any(pad.kind is not PadKind.SMD for pad in lands):
                pending.append(reference)
                continue
            positions = sorted(
                {
                    transformed_local_point(placement, pad.position)
                    for pad in lands
                },
                key=lambda point: (point.x_nm, point.y_nm),
            )
            if len(positions) < 2:
                continue
            side = (
                CopperLayer.FRONT if placement.side is BoardSide.FRONT
                else CopperLayer.BACK
            )
            rule = rules.get(net)
            width = (
                rule.width_nm if rule and rule.width_nm is not None
                else board.rules.default_track_width_nm
            )
            connected = {positions[0]}
            remaining = set(positions[1:])
            local_tracks: list[TrackSegment] = []
            while remaining:
                proposals = sorted(
                    (
                        (abs(start.x_nm - end.x_nm)
                         + abs(start.y_nm - end.y_nm), start, end)
                        for start in connected for end in remaining
                    ),
                    key=lambda item: (
                        item[0], item[1].x_nm, item[1].y_nm,
                        item[2].x_nm, item[2].y_nm,
                    ),
                )
                chosen: tuple[TrackSegment | None, Point] | None = None
                for _, start, end in proposals:
                    if any(
                        track.net == net and track.layer is side
                        and {track.start, track.end} == {start, end}
                        for track in (*board.tracks, *added)
                    ):
                        chosen = None, end
                        break
                    if clearance.can_track(net, start, end, width, side):
                        chosen = TrackSegment(net, start, end, width, side), end
                        break
                if chosen is None:
                    pending.append(reference)
                    added.extend(local_tracks)
                    break
                track, end = chosen
                if track is not None:
                    local_tracks.append(track)
                    clearance.add_track(track)
                connected.add(end)
                remaining.remove(end)
            else:
                stitched.append(reference)
                added.extend(local_tracks)
    routed = replace(board, tracks=(*board.tracks, *added)) if added else board
    return DuplicatePadStitchResult(
        routed, tuple(stitched), tuple(pending), len(added),
    )
