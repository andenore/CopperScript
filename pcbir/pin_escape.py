"""Shared exact octilinear terminal access and existing fanout verification.

This owns only bounded lead-ins, not package allocation, via generation or
area routing. Callers retain their own candidate and transaction budgets.
"""
from collections import Counter
from dataclasses import dataclass

from .physical import BoardSide, CopperLayer, PadKind, PadReference, PhysicalBoard, Point, TrackSegment
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .surface_path import _track_inside_board


@dataclass(frozen=True, slots=True)
class RoutingAccess:
    """An owned, layer-specific port reached from an existing pad launch.

    Position is not implicitly a via and cannot grant access to other layers.
    The path must exist as input copper; this descriptor never creates a wire.
    """
    position: Point
    layer: CopperLayer
    launch_position: Point
    path: tuple[TrackSegment, ...]

    def __post_init__(self):
        object.__setattr__(self, "path", tuple(self.path))
        if not self.path:
            raise ValueError("routing access requires an explicit path")


def access_position(access: Point | RoutingAccess) -> Point:
    return access.position if isinstance(access, RoutingAccess) else access


def launch_position(access: Point | RoutingAccess) -> Point:
    return access.launch_position if isinstance(access, RoutingAccess) else access


def verified_routing_access(
    board: PhysicalBoard, pad: PadReference, net: str, access: RoutingAccess,
    clearance: RoutingClearanceIndex,
) -> tuple[TrackSegment, ...] | None:
    """Verify actual terminal, layer transition and every existing port leg.

    A same-surface port remains valid if cleanup removed an unnecessary via.
    Off-surface ports require an actual via spanning both terminal and port.
    """
    from .routing_layers import routing_layers
    rule = next((r for r in board.net_routing_rules if r.net == net), None)
    if access.layer not in routing_layers(board, net, rule):
        return None
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    if Counter(access.path) - Counter(board.tracks):
        return None
    cursor = access.launch_position
    for track in access.path:
        # Inside a breakout region an ordinary net may neck down (plan R1).
        if (track.net != net or track.layer is not access.layer or track.start != cursor
                or track.width_nm < clearance.breakout.required_width_nm(track, width)
                or not (track.start.x_nm == track.end.x_nm or track.start.y_nm == track.end.y_nm
                        or abs(track.end.x_nm-track.start.x_nm) == abs(track.end.y_nm-track.start.y_nm))
                or not _track_inside_board(board, track.start, track.end, track.width_nm)
                or not clearance.can_track(net, track.start, track.end, track.width_nm, track.layer)):
            return None
        cursor = track.end
    if cursor != access.position:
        return None
    pose = next(p for p in board.placements if p.reference == pad.component)
    footprint = board.footprints[pose.footprint]
    group = next((g for g in footprint.internal_pad_groups if pad.pad in g.numbers), None)
    terminals = tuple(p for p in footprint.pads if p.number in (group.numbers if group else (pad.pad,)))
    side = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
    for terminal in terminals:
        path = _verified_land_path(board, terminal, pose, net, access.launch_position, clearance,
                                   require_via=access.layer is not side, target_layer=access.layer,
                                   required_width_nm=width)
        if path is not None:
            return (*path, *access.path)
    return None


def checked_access_path(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
    start: Point, end: Point, width_nm: int, layer: CopperLayer,
    allow_movable_conflicts: bool = False, *, allow_orthogonal: bool = True,
) -> tuple[TrackSegment, ...] | None:
    """Try both diagonal/straight orders; emit precisely the checked legs."""
    return next(checked_access_paths(board, clearance, net, start, end, width_nm,
                                    layer, allow_movable_conflicts,
                                    allow_orthogonal=allow_orthogonal), None)


def checked_access_paths(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
    start: Point, end: Point, width_nm: int, layer: CopperLayer,
    allow_movable_conflicts: bool = False, *, allow_orthogonal: bool = True,
):
    """Lazily enumerate checked orders; ordinary access still takes the first."""
    if start == end:
        yield ()
        return
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
        # Each leg is checked and emitted as its pieces: an ordinary breakout
        # net necks down inside its regions (plan R1), any other is one track.
        tracks = tuple(piece for a, b in zip(points, points[1:]) if a != b
                       for piece in clearance.route_pieces(net, a, b, width_nm, layer))
        for track in tracks:
            if not _track_inside_board(board, track.start, track.end, track.width_nm):
                break
            if allow_movable_conflicts:
                if clearance.blocking_track_nets(track)[1]:
                    break
            elif not clearance.can_track(net, track.start, track.end, track.width_nm, layer):
                break
        else:
            yield tracks


def verified_fanout_path(
    board: PhysicalBoard, pad: PadReference, net: str, anchor: Point,
    clearance: RoutingClearanceIndex,
) -> tuple[TrackSegment, ...] | None:
    """Recognize a connected existing launch chain and a same-net physical via.

    Endpoints must meet exactly on a common terminal layer. Proximity, a via
    alone, an off-layer trace or a claimed anchor is not connectivity evidence.
    Input tracks are returned without rewriting their orientation/identity.
    """
    placement = next(p for p in board.placements if p.reference == pad.component)
    footprint = board.footprints[placement.footprint]
    group = next((g for g in footprint.internal_pad_groups if pad.pad in g.numbers), None)
    terminals = tuple(p for p in footprint.pads
                      if p.number in (group.numbers if group else (pad.pad,)))
    for terminal in terminals:
        path = _verified_land_path(board, terminal, placement, net, anchor, clearance)
        if path is not None:
            return path
    return None


def _verified_land_path(board, terminal, placement, net, anchor, clearance, *,
                        require_via=True, target_layer=None, required_width_nm=None):
    start = transformed_local_point(placement, terminal.position)
    vias = tuple(v for v in board.vias if v.net == net and v.position == anchor
                 and v.from_layer in board.stackup.copper_layers
                 and v.to_layer in board.stackup.copper_layers)
    side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
    layers = (side,) if terminal.kind is PadKind.SMD else board.stackup.copper_layers
    for layer in layers:
        via = next((v for v in vias if board.stackup.copper_layers.index(v.from_layer)
                    <= board.stackup.copper_layers.index(layer)
                    <= board.stackup.copper_layers.index(v.to_layer)), None)
        if require_via:
            if (via is None or target_layer is not None and not (
                    board.stackup.copper_layers.index(via.from_layer) <= board.stackup.copper_layers.index(target_layer)
                    <= board.stackup.copper_layers.index(via.to_layer))
                    or not clearance.can_via(net, anchor, via.size_nm,
                                            via.from_layer, via.to_layer, drill_nm=via.drill_nm)):
                continue
        elif target_layer is not None and layer is not target_layer:
            continue
        tracks = tuple(t for t in board.tracks if t.net == net and t.layer is layer)
        from collections import defaultdict, deque
        adjacency = defaultdict(list)
        for track in tracks:
            # A land path of an ordinary breakout net may neck down (plan R1).
            required = (clearance.breakout.required_width_nm(track, required_width_nm)
                        if required_width_nm else 0)
            if ((track.start.x_nm == track.end.x_nm or track.start.y_nm == track.end.y_nm
                 or abs(track.start.x_nm-track.end.x_nm) == abs(track.start.y_nm-track.end.y_nm))
                    and track.width_nm >= max(board.rules.minimum_track_width_nm, required)
                    and _track_inside_board(board, track.start, track.end, track.width_nm)
                    and clearance.can_track(net, track.start, track.end, track.width_nm, layer)):
                adjacency[track.start].append((track.end, track))
                adjacency[track.end].append((track.start, track))
        paths = {start: ()}
        queue = deque((start,))
        while queue:
            point = queue.popleft()
            if point == anchor:
                return paths[point]
            for neighbor, track in adjacency[point]:
                if neighbor not in paths:
                    paths[neighbor] = (*paths[point], track)
                    queue.append(neighbor)
    return None
