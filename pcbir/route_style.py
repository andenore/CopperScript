"""Conservative, exact-clearance 45-degree cleanup of ordinary route corners.

Only degree-two, perpendicular octilinear corners are changed. Ports, vias,
branch contacts and input copper are immutable; differential pairs must use
their joint refinement pipeline instead. An obstacle is a reason to retain a
corner, never a reason to emit an unchecked shortcut. So is a necked-down
corner whose diagonal would leave its breakout region (plan R1).
"""
from collections import defaultdict

from .geometry import RoundedConvexShape, shapes_clear
from .physical import PhysicalBoard, Point, TrackSegment, Via
from .routing_clearance import RoutingClearanceIndex
from .surface_path import _track_inside_board


def chamfer_ordinary_corners(
    board: PhysicalBoard, tracks: tuple[TrackSegment, ...],
    vias: tuple[Via, ...], clearance: RoutingClearanceIndex,
    *, allow_movable_conflicts: bool = False,
) -> tuple[TrackSegment, ...]:
    active = list(tracks)
    # Each input corner is visited once. New diagonal endpoints are not
    # recursively rounded, which keeps geometry and runtime deterministic.
    endpoints = defaultdict(list)
    for index, track in enumerate(tracks):
        endpoints[track.layer, track.start].append(index)
        endpoints[track.layer, track.end].append(index)
    for (layer, corner), indexes in sorted(endpoints.items(),
            key=lambda item: (item[0][0].value, item[0][1].x_nm, item[0][1].y_nm)):
        if len(indexes) != 2:
            continue
        i, j = indexes
        first, second = active[i], active[j]
        if first.net != second.net or first.width_nm != second.width_nm:
            continue
        if corner not in (first.start, first.end) or corner not in (second.start, second.end):
            continue
        a = first.end if first.start == corner else first.start
        b = second.end if second.start == corner else second.start
        dx, dy = a.x_nm-corner.x_nm, a.y_nm-corner.y_nm
        ex, ey = b.x_nm-corner.x_nm, b.y_nm-corner.y_nm
        if dx*ex + dy*ey != 0 or not max(abs(dx), abs(dy)) or not max(abs(ex), abs(ey)):
            continue
        if not ((not dx or not dy or abs(dx) == abs(dy))
                and (not ex or not ey or abs(ex) == abs(ey))):
            continue
        u = ((dx > 0)-(dx < 0), (dy > 0)-(dy < 0))
        v = ((ex > 0)-(ex < 0), (ey > 0)-(ey < 0))
        for divisor in (4, 8, 16):
            trim = min(max(abs(dx), abs(dy)), max(abs(ex), abs(ey))) // divisor
            if trim < first.width_nm:
                continue
            p = Point(corner.x_nm+u[0]*trim, corner.y_nm+u[1]*trim)
            q = Point(corner.x_nm+v[0]*trim, corner.y_nm+v[1]*trim)
            removed = (RoundedConvexShape((p, corner), first.width_nm//2),
                       RoundedConvexShape((corner, q), first.width_nm//2))
            # Never sever a connection in the cut-away corner, including a
            # branch meeting the interior of a segment rather than its end.
            if any(not clearance.pad_copper_clear(shape, (layer,)) for shape in removed):
                continue
            peers = [t for k, t in enumerate(active) if k not in (i, j)] + list(board.tracks)
            if any(t.layer == layer and t.net == first.net
                   and any(not shapes_clear(shape, RoundedConvexShape(
                       (t.start, t.end), t.width_nm//2), 1) for shape in removed)
                   for t in peers):
                continue
            if any(via.net == first.net and any(not shapes_clear(shape,
                    RoundedConvexShape((via.position,), via.size_nm//2), 1)
                    for shape in removed) for via in (*board.vias, *vias)):
                continue
            diagonal = TrackSegment(first.net, p, q, first.width_nm, layer)
            breakout = clearance.breakout
            if (first.net in breakout.ordinary and first.width_nm < breakout.width_nm(first.net)
                    and breakout.region(first.net, (p, q)) is None):
                continue  # A necked-down diagonal must stay inside a breakout region.
            if not _track_inside_board(board, p, q, first.width_nm):
                continue
            if allow_movable_conflicts:
                legal = not clearance.blocking_track_nets(diagonal)[1]
            else:
                legal = clearance.can_track(first.net, p, q, first.width_nm, layer)
            if not legal:
                continue
            active[i] = TrackSegment(first.net, a, p, first.width_nm, layer)
            active[j] = TrackSegment(second.net, q, b, second.width_nm, layer)
            active.append(diagonal)
            break
    return tuple(active)
