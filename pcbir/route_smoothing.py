"""Ownership-safe octilinear smoothing of accepted ordinary route chains.

Detailed search runs on a coarse grid with pad-inserted coordinates. Its
accepted copper therefore keeps short 90-degree jogs, same-net hairpins and
detours that per-attempt chamfering cannot remove: a chamfer must fit inside
both legs, and the copper that forced a detour may since have been ripped up.

After the board is otherwise final, this pass walks each owned track chain
between anchors (vias, branches, width changes, immutable copper and interior
contacts) and greedily replaces the longest possible sub-chain by one straight
or one 45-degree-plus-straight connection. A replacement is kept only if it is
never longer, never sharper, and either shorter or less sharp in total; turns
are scored in 45-degree steps beyond 45 degrees. Each new segment is checked
exactly against other nets, keepouts and the outline, and every pad the
removed copper touched must still be touched. A net whose explicit copper
connectivity or via-layer contacts would change keeps its original copper.
No via, immutable, critical or reserved copper is moved or used as input.
New copper of an ordinary net with breakout properties is checked and emitted
as its pieces: the breakout width inside a region, the profile width outside
(plan R1). Width changes stay anchors, so a necked chain is never widened.
"""
from __future__ import annotations

from collections import defaultdict
from dataclasses import replace
from math import isqrt

from .copper_connectivity import (CopperContact, _positive_area_overlap,
                                  copper_contacts_overlap, via_copper_contact)
from .geometry import RoundedConvexShape, point_on_segment, shapes_clear
from .physical import CopperLayer, PhysicalBoard, PhysicalNet, Point, TrackSegment
from .routing_clearance import RoutingClearanceIndex
from .surface_path import _track_inside_board

Vector = tuple[int, int]


def smooth_owned_tracks(
    board: PhysicalBoard, tracks: tuple[TrackSegment, ...],
    clearance: RoutingClearanceIndex, *, rounds: int = 2,
) -> tuple[TrackSegment, ...]:
    """Return owned ``tracks`` with removable bends and detours straightened.

    ``board`` holds every other track and via, including immutable copper of
    the same nets; ``clearance`` must index the complete final copper. New
    segments are added to it so later nets see them; retired copper stays as
    a conservative obstacle. Nets are processed in name order.
    """
    if not tracks:
        return tracks
    zone_nets = {zone.net for zone in board.zones}
    by_net: dict[str, list[TrackSegment]] = defaultdict(list)
    for track in tracks:
        by_net[track.net].append(track)
    nets = {net.name: net for net in board.nets}
    result: list[TrackSegment] = []
    for name in sorted(by_net):
        owned = tuple(by_net[name])
        if name in zone_nets or name not in nets:
            result.extend(owned)
            continue
        smoothed = owned
        for _ in range(rounds):
            candidate = _smooth_net(board, smoothed, clearance)
            if candidate == smoothed:
                break
            smoothed = candidate
        if smoothed != owned and _contacts_preserved(board, nets[name], owned, smoothed):
            for track in set(smoothed) - set(owned):
                clearance.add_track(track)
            result.extend(smoothed)
        else:
            result.extend(owned)
    return tuple(result)


def _smooth_net(board: PhysicalBoard, owned: tuple[TrackSegment, ...],
                clearance: RoutingClearanceIndex) -> tuple[TrackSegment, ...]:
    by_layer: dict[CopperLayer, list[TrackSegment]] = defaultdict(list)
    for track in owned:
        by_layer[track.layer].append(track)
    result: list[TrackSegment] = []
    for layer in sorted(by_layer, key=lambda item: item.value):
        result.extend(_smooth_layer(board, tuple(by_layer[layer]), layer, clearance))
    return tuple(result)


def _smooth_layer(board: PhysicalBoard, owned: tuple[TrackSegment, ...], layer: CopperLayer,
                  clearance: RoutingClearanceIndex) -> list[TrackSegment]:
    net = owned[0].net
    fixed_tracks = tuple(t for t in board.tracks if t.net == net and t.layer is layer)
    order = board.stackup.copper_layers
    vias = tuple(v for v in board.vias if v.net == net
                 and min(order.index(v.from_layer), order.index(v.to_layer))
                 <= order.index(layer)
                 <= max(order.index(v.from_layer), order.index(v.to_layer)))
    incident: dict[Point, list[int]] = defaultdict(list)
    for index, track in enumerate(owned):
        if track.start == track.end:
            return list(owned)
        incident[track.start].append(index)
        incident[track.end].append(index)
    via_shapes = tuple(RoundedConvexShape((v.position,), v.size_nm // 2) for v in vias)
    fixed_shapes = tuple(RoundedConvexShape((t.start, t.end), t.width_nm // 2) for t in fixed_tracks)
    anchors: set[Point] = set()
    for point, indexes in incident.items():
        shape = RoundedConvexShape((point,), owned[indexes[0]].width_nm // 2)
        if (len(indexes) != 2 or len({owned[i].width_nm for i in indexes}) != 1
                or any(not shapes_clear(shape, other, 1) for other in (*via_shapes, *fixed_shapes))):
            anchors.add(point)
    protected: set[int] = set()
    for index, track in enumerate(owned):
        if _interior_contact(track, fixed_tracks):
            protected.add(index)
            anchors.update((track.start, track.end))
    chains = _chains(owned, incident, anchors, protected)
    if not chains:
        return list(owned)
    used = {index for _, indexes in chains for index in indexes}
    result = [track for index, track in enumerate(owned) if index not in used]
    for points, indexes in chains:
        width = owned[indexes[0]].width_nm
        external = (_external_vector(points[0], indexes[0], owned, incident, fixed_tracks),
                    _external_vector(points[-1], indexes[-1], owned, incident, fixed_tracks))
        members = set(indexes)
        others = tuple(t for i, t in enumerate(owned) if i not in members) + fixed_tracks
        simplified = _simplify(board, net, layer, width, points, external, clearance, others, vias)
        if simplified == points:
            result.extend(owned[index] for index in indexes)
        else:
            result.extend(_pieces(clearance, net, layer, width, simplified))
    return result


def _pieces(clearance: RoutingClearanceIndex, net: str, layer: CopperLayer, width: int,
            points) -> list[TrackSegment]:
    """New copper along ``points``; an ordinary breakout net necks down near its lands."""
    breakout = clearance.breakout
    outside = max(width, breakout.width_nm(net)) if net in breakout.ordinary else width
    return [piece for a, b in zip(points, points[1:])
            for piece in clearance.route_pieces(net, a, b, outside, layer)]


def _chains(owned, incident, anchors, protected):
    """Maximal unprotected owned paths through unanchored degree-two vertices."""
    seen: set[int] = set()
    chains = []
    for start in sorted(anchors, key=lambda p: (p.x_nm, p.y_nm)):
        for first in sorted(incident.get(start, ()),
                            key=lambda i: (owned[i].start.x_nm, owned[i].start.y_nm,
                                           owned[i].end.x_nm, owned[i].end.y_nm)):
            if first in seen or first in protected:
                continue
            points, indexes = [start], []
            index, point = first, start
            while True:
                seen.add(index)
                indexes.append(index)
                track = owned[index]
                point = track.end if track.start == point else track.start
                points.append(point)
                if point in anchors:
                    break
                following = [i for i in incident[point] if i != index]
                if len(following) != 1 or following[0] in seen or following[0] in protected:
                    break
                index = following[0]
            if len(points) >= 3:
                chains.append((tuple(points), tuple(indexes)))
    return chains


def _interior_contact(track, fixed_tracks) -> bool:
    """Protect a track that immutable copper touches away from its ends.

    Contact through a shared end vertex, or an end resting on another
    centre line, is an end contact and is represented by the chain anchors.
    Pad and via contacts are checked per replacement; contacts among owned
    copper are covered by the per-net connectivity comparison.
    """
    ends = (track.start, track.end)
    shape = RoundedConvexShape(ends, track.width_nm // 2)
    middle = _middle(track)
    for other in fixed_tracks:
        other_shape = RoundedConvexShape((other.start, other.end), other.width_nm // 2)
        if shapes_clear(shape, other_shape, 1):
            continue
        at_end = ({other.start, other.end} & set(ends)
                  or any(point_on_segment(p, other.start, other.end) for p in ends))
        if not at_end or any(point_on_segment(p, track.start, track.end) and p not in ends
                             for p in (other.start, other.end)):
            return True
        if middle is not None and not shapes_clear(middle, other_shape, 1):
            return True
    return False


def _middle(track: TrackSegment) -> RoundedConvexShape | None:
    """The track centre line without two widths at each end, if any remains."""
    dx, dy = track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm
    length = isqrt(dx * dx + dy * dy)
    cut = 2 * track.width_nm
    if length <= 2 * cut:
        return None
    return RoundedConvexShape((
        Point(track.start.x_nm + dx * cut // length, track.start.y_nm + dy * cut // length),
        Point(track.end.x_nm - dx * cut // length, track.end.y_nm - dy * cut // length),
    ), track.width_nm // 2)


def _external_vector(point, index, owned, incident, fixed_tracks) -> Vector | None:
    """The one other same-net track leaving a degree-two chain end, if any."""
    others = [owned[i] for i in incident.get(point, ()) if i != index]
    others += [t for t in fixed_tracks if point in (t.start, t.end)]
    if len(others) != 1:
        return None
    other = others[0]
    far = other.end if other.start == point else other.start
    return (far.x_nm - point.x_nm, far.y_nm - point.y_nm)


def turn_score(first: Vector | None, second: Vector | None) -> int:
    """0 up to 45 degrees, then 1 per further 45-degree step (vectors leave one vertex)."""
    if first is None or second is None:
        return 0
    dot = first[0] * second[0] + first[1] * second[1]
    if dot < 0:
        return 0
    if dot == 0:
        return 1
    if first[0] * second[1] - first[1] * second[0] == 0:
        return 3
    return 2


def _vector(a: Point, b: Point) -> Vector:
    return (b.x_nm - a.x_nm, b.y_nm - a.y_nm)


def _length(points) -> int:
    return sum(isqrt((b.x_nm - a.x_nm) ** 2 + (b.y_nm - a.y_nm) ** 2) for a, b in zip(points, points[1:]))


def _connections(a: Point, b: Point) -> list[tuple[Point, ...]]:
    dx, dy = b.x_nm - a.x_nm, b.y_nm - a.y_nm
    if not dx and not dy:
        return []
    if not dx or not dy or abs(dx) == abs(dy):
        return [(a, b)]
    sx, sy = (dx > 0) - (dx < 0), (dy > 0) - (dy < 0)
    diagonal = min(abs(dx), abs(dy))
    return [(a, Point(a.x_nm + sx * diagonal, a.y_nm + sy * diagonal), b),
            (a, Point(b.x_nm - sx * diagonal, b.y_nm - sy * diagonal), b)]


def _scores(points, incoming: Vector | None, outgoing: Vector | None) -> list[int]:
    back = [incoming, *(_vector(points[k], points[k - 1]) for k in range(1, len(points)))]
    ahead = [_vector(points[k], points[k + 1]) for k in range(len(points) - 1)] + [outgoing]
    return [turn_score(first, second) for first, second in zip(back, ahead)]


def _simplify(board, net, layer, width, points, external, clearance, others, vias) -> tuple[Point, ...]:
    """Greedy farthest-reach replacement along one chain, never worse locally."""
    result = [points[0]]
    incoming = external[0]  # vector from the current vertex back along kept copper
    i = 0
    last = len(points) - 1
    while i < last:
        chosen = None
        for j in range(last, i + 1, -1):
            outgoing = _vector(points[j], points[j + 1]) if j < last else external[1]
            old = points[i:j + 1]
            old_scores = _scores(old, incoming, outgoing)
            old_length = _length(old)
            for candidate in _connections(points[i], points[j]):
                new_scores = _scores(candidate, incoming, outgoing)
                new_length = _length(candidate)
                if (new_length > old_length or max(new_scores) > max(old_scores)
                        or (sum(new_scores), new_length) >= (sum(old_scores), old_length)):
                    continue
                if _legal(board, net, layer, width, old, candidate, clearance, others, vias):
                    chosen = (j, candidate)
                    break
            if chosen is not None:
                break
        if chosen is None:
            result.append(points[i + 1])
            incoming = _vector(points[i + 1], points[i])
            i += 1
            continue
        j, candidate = chosen
        result.extend(candidate[1:])
        incoming = _vector(candidate[-1], candidate[-2])
        i = j
    return tuple(result)


def _legal(board, net, layer, width, old, candidate, clearance, others, vias) -> bool:
    """Exact clearance for new copper; every pad touched by retired copper stays touched."""
    pieces = _pieces(clearance, net, layer, width, candidate)
    if not all(_track_inside_board(board, t.start, t.end, t.width_nm)
               and clearance.can_track(net, t.start, t.end, t.width_nm, layer)
               for t in pieces):
        return False
    new = [RoundedConvexShape((t.start, t.end), t.width_nm // 2) for t in pieces]
    retired = [RoundedConvexShape((a, b), width // 2) for a, b in zip(old, old[1:])]
    kept = [RoundedConvexShape((t.start, t.end), t.width_nm // 2) for t in others]
    via_shapes = [RoundedConvexShape((v.position,), v.size_nm // 2) for v in vias]
    lost = _touched_pads(clearance, retired, layer) - _touched_pads(clearance, new, layer)
    if lost and lost - _touched_pads(clearance, kept + via_shapes, layer):
        return False
    # A via keeps a track on this layer if retired copper provided one.
    return all(not _touching_via(via, retired, layer) or _touching_via(via, new, layer)
               or _touching_via(via, kept, layer)
               for via in (via_copper_contact(v, board.stackup.copper_layers) for v in vias))


def _touching_via(via: CopperContact, shapes, layer: CopperLayer) -> bool:
    return any(copper_contacts_overlap(via, CopperContact("track", via.net, (layer,), shape))
               for shape in shapes)


def _touched_pads(clearance: RoutingClearanceIndex, shapes, layer: CopperLayer) -> set[int]:
    touched = set()
    for shape in shapes:
        for item in clearance._overlapping_objects(shape, (layer,)):
            if item.is_pad and _positive_area_overlap(shape, item.shape):
                touched.add(id(item))
    return touched


def _contacts_preserved(board: PhysicalBoard, net: PhysicalNet,
                        before: tuple[TrackSegment, ...], after: tuple[TrackSegment, ...]) -> bool:
    """No new explicit-copper islands, and every via keeps each layer's track contact."""
    from .drc import explicit_copper_connectivity

    def islands(tracks):
        candidate = replace(board, tracks=(*board.tracks, *tracks))
        connectivity = explicit_copper_connectivity(candidate, only_nets=frozenset((net.name,)))
        roots = {connectivity.roots[node] for pad in net.pads
                 for node in connectivity.pad_nodes.get(pad, ())}
        return len(roots) + sum(not connectivity.pad_nodes.get(pad) for pad in net.pads)

    if islands(after) > islands(before):
        return False
    fixed = tuple(t for t in board.tracks if t.net == net.name)
    before, after = (*before, *fixed), (*after, *fixed)
    for via in (v for v in board.vias if v.net == net.name):
        contact = via_copper_contact(via, board.stackup.copper_layers)

        def touched(tracks):
            return {t.layer for t in tracks
                    if copper_contacts_overlap(contact, CopperContact("track", t.net, (t.layer,),
                        RoundedConvexShape((t.start, t.end), t.width_nm // 2)))}
        if not touched(before) <= touched(after):
            return False
    return True
