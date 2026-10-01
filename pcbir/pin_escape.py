"""Shared exact octilinear terminal access and existing fanout verification.

This owns only bounded lead-ins, not package allocation, via generation or
area routing. Callers retain their own candidate and transaction budgets.
"""
from .physical import BoardSide, CopperLayer, PadKind, PadReference, PhysicalBoard, Point, TrackSegment
from .placement import transformed_pad_position
from .routing_clearance import RoutingClearanceIndex
from .surface_path import _track_inside_board


def checked_access_path(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
    start: Point, end: Point, width_nm: int, layer: CopperLayer,
    allow_movable_conflicts: bool = False, *, allow_orthogonal: bool = True,
) -> tuple[TrackSegment, ...] | None:
    """Try both diagonal/straight orders; emit precisely the checked legs."""
    if start == end:
        return ()
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    if not dx or not dy or abs(dx) == abs(dy):
        paths = ((start, end),)
    else:
        diagonal = min(abs(dx), abs(dy))
        sx, sy = (1 if dx > 0 else -1), (1 if dy > 0 else -1)
        paths = (
            (start, Point(start.x_nm + sx * diagonal, start.y_nm + sy * diagonal), end),
            (start, Point(end.x_nm - sx * diagonal, end.y_nm - sy * diagonal), end),
        )
        if allow_orthogonal:
            paths += ((start, Point(start.x_nm, end.y_nm), end),
                      (start, Point(end.x_nm, start.y_nm), end))
    for points in paths:
        tracks = tuple(TrackSegment(net, a, b, width_nm, layer)
                       for a, b in zip(points, points[1:]) if a != b)
        for track in tracks:
            if not _track_inside_board(board, track.start, track.end, width_nm):
                break
            if allow_movable_conflicts:
                if clearance.blocking_track_nets(track)[1]:
                    break
            elif not clearance.can_track(net, track.start, track.end, width_nm, layer):
                break
        else:
            return tracks
    return None


def verified_fanout_path(
    board: PhysicalBoard, pad: PadReference, net: str, anchor: Point,
    clearance: RoutingClearanceIndex,
) -> tuple[TrackSegment, ...] | None:
    """Recognize one/two existing legs and an actual same-net physical via.

    Endpoints must meet exactly on a common terminal layer. Proximity, a via
    alone, an off-layer trace or a claimed anchor is not connectivity evidence.
    Input tracks are returned without rewriting their orientation/identity.
    """
    placement = next(p for p in board.placements if p.reference == pad.component)
    terminal = next(p for p in board.footprints[placement.footprint].pads if p.number == pad.pad)
    start = transformed_pad_position(board, placement, pad.pad)
    vias = tuple(v for v in board.vias if v.net == net and v.position == anchor
                 and v.from_layer in board.stackup.copper_layers
                 and v.to_layer in board.stackup.copper_layers)
    side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
    layers = (side,) if terminal.kind is PadKind.SMD else board.stackup.copper_layers
    for layer in layers:
        via = next((v for v in vias if board.stackup.copper_layers.index(v.from_layer)
                    <= board.stackup.copper_layers.index(layer)
                    <= board.stackup.copper_layers.index(v.to_layer)), None)
        if via is None or not clearance.can_via(net, anchor, via.size_nm,
                                               via.from_layer, via.to_layer, drill_nm=via.drill_nm):
            continue
        tracks = tuple(t for t in board.tracks if t.net == net and t.layer is layer)
        for first in tracks:
            if start not in (first.start, first.end):
                continue
            middle = first.end if first.start == start else first.start
            candidates = ((first,),) if middle == anchor else tuple(
                (first, second) for second in tracks if second != first
                and (second.start == middle and second.end == anchor
                     or second.end == middle and second.start == anchor))
            for path in candidates:
                if all((t.start.x_nm == t.end.x_nm or t.start.y_nm == t.end.y_nm
                        or abs(t.start.x_nm-t.end.x_nm) == abs(t.start.y_nm-t.end.y_nm))
                       and t.width_nm >= board.rules.minimum_track_width_nm
                       and _track_inside_board(board, t.start, t.end, t.width_nm)
                       and clearance.can_track(net, t.start, t.end, t.width_nm, layer)
                       for t in path):
                    return path
    return None
