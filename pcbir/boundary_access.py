"""Bounded ordinary package-to-boundary capacity witnesses, not area routes.

Dogbones only prove access to a via. This stage verifies a connected launch,
then jointly allocates exact-clearance paths beyond a conservative package
collar on permitted signal layers. Witness copper is provisional and never
silently added to the board or passed off as end-to-end connectivity.
"""
from __future__ import annotations

from dataclasses import dataclass
from heapq import heappop, heappush
from math import isqrt

from .drc import placed_pad_shape
from .escape_assignment import EscapeAssignmentOptions
from .geometry import Bounds, bounds
from .physical import CopperLayer, PhysicalBoard, Point, TrackSegment, nm_from_mm
from .pin_escape import checked_access_paths
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .surface_path import _track_inside_board


@dataclass(frozen=True, slots=True)
class BoundaryAccessOptions:
    collar_margin_nm: int = nm_from_mm("0.25")
    port_step_nm: int = nm_from_mm("0.5")
    refinement_step_nm: int = nm_from_mm("0.1")
    maximum_ports_per_edge: int = 8
    maximum_refined_ports_per_edge: int = 32
    maximum_detour_nm: int = nm_from_mm("2")
    maze_escapes: bool = True
    maze_state_budget: int = 12_000
    assignment_options: EscapeAssignmentOptions = EscapeAssignmentOptions()
    # Prefer ports on a collar edge facing the net's nearest other-component
    # terminal before layer rank (R18); off keeps rank-first ordering.
    destination_ports: bool = False

    def __post_init__(self):
        if min(self.collar_margin_nm, self.port_step_nm, self.refinement_step_nm,
               self.maximum_ports_per_edge, self.maximum_refined_ports_per_edge, self.maze_state_budget) <= 0:
            raise ValueError("boundary access bounds must be positive")
        if self.refinement_step_nm > self.port_step_nm or self.maximum_detour_nm < 0:
            raise ValueError("boundary access refinement/detour bounds are invalid")


@dataclass(frozen=True, slots=True)
class PackageCollar:
    reference: str
    bounds: Bounds


def package_collar(board: PhysicalBoard, reference: str, margin_nm: int) -> PackageCollar:
    """World-axis bounds of transformed courtyard/body AND actual copper lands.

    Axis bounds are conservative at 45 degrees and on the back side. They are
    a routing hand-off region, not a new physical keepout or electrical datum.
    """
    if margin_nm < 0:
        raise ValueError("package collar margin cannot be negative")
    pose = next((p for p in board.placements if p.reference == reference), None)
    if pose is None:
        raise ValueError(f"unknown package collar component {reference!r}")
    footprint = board.footprints[pose.footprint]
    local = footprint.courtyard or tuple(Point(x, y)
        for x in (-footprint.body_size.width_nm // 2, footprint.body_size.width_nm // 2)
        for y in (-footprint.body_size.height_nm // 2, footprint.body_size.height_nm // 2))
    regions = [bounds(transformed_local_point(pose, p) for p in local)]
    regions.extend(placed_pad_shape(transformed_local_point(pose, p.position), p, pose).bounds
                   for p in footprint.pads)
    area = Bounds(min(b.min_x for b in regions), min(b.min_y for b in regions),
                  max(b.max_x for b in regions), max(b.max_y for b in regions))
    return PackageCollar(reference, area.expanded(margin_nm))


def _ports(start: Point, area: Bounds, extension: int, step: int, limit: int):
    # Always include the exact projection: sampling is local to the anchor,
    # never a board-global grid which can miss a narrow legal launch window.
    for edge in ("left", "right", "top", "bottom"):
        vertical = edge in {"left", "right"}
        coordinate = start.y_nm if vertical else start.x_nm
        low, high = (area.min_y, area.max_y) if vertical else (area.min_x, area.max_x)
        coordinate = max(low, min(high, coordinate))
        offsets = [0]
        for i in range(1, limit):
            offsets.extend((-i * step, i * step))
        values = tuple(dict.fromkeys(max(low, min(high, coordinate + d)) for d in offsets))[:limit]
        for value in values:
            if edge == "left":
                point = Point(min(area.min_x, start.x_nm) - extension, value)
            elif edge == "right":
                point = Point(max(area.max_x, start.x_nm) + extension, value)
            elif edge == "top":
                point = Point(value, min(area.min_y, start.y_nm) - extension)
            else:
                point = Point(value, max(area.max_y, start.y_nm) + extension)
            yield edge, point


def _maze_path(board, clearance, net, start, width, layer, area, extension, options):
    """Octilinear A* to any collar edge, without inventing a via or grid snap.

    The anchor-local lattice, heading state and exact capsule checks apply to
    the emitted geometry. Search bounds/states are independent of CSP budgets.
    Only 45-degree heading changes are generated; collinear edges are merged.
    """
    step = options.refinement_step_nm
    goal = Bounds(min(area.min_x, start.x_nm)-extension, min(area.min_y, start.y_nm)-extension,
                  max(area.max_x, start.x_nm)+extension, max(area.max_y, start.y_nm)+extension)
    search = goal.expanded(options.maximum_detour_nm)
    edge_distances = dict(left=start.x_nm-goal.min_x, right=goal.max_x-start.x_nm,
                          top=start.y_nm-goal.min_y, bottom=goal.max_y-start.y_nm)
    maximum_length = min(edge_distances.values()) + options.maximum_detour_nm
    eligible_edges = {edge for edge, distance in edge_distances.items() if distance <= maximum_length}
    directions = ((1,0), (1,1), (0,1), (-1,1), (-1,0), (-1,-1), (0,-1), (1,-1))
    def point(x, y):
        return Point(start.x_nm+x*step, start.y_nm+y*step)
    def heuristic(p):
        remaining = dict(left=p.x_nm-goal.min_x, right=goal.max_x-p.x_nm,
                         top=p.y_nm-goal.min_y, bottom=goal.max_y-p.y_nm)
        distance = max(0, min(remaining[edge] for edge in eligible_edges))
        return 10 * ((distance+step-1)//step)
    queue = [(heuristic(start), 0, 0, 0, -1)]
    best = {(0,0,-1): 0}
    parent = {(0,0,-1): None}
    lengths = {(0,0,-1): 0}
    diagonal_length = isqrt(2*step*step)
    if diagonal_length*diagonal_length < 2*step*step:
        diagonal_length += 1
    # Different arrival headings revisit identical undirected copper edges.
    # The domain board is immutable: cache only this layer/search's exact
    # predicate, never reuse it across different geometry or reservations.
    legal_edges = {}
    states = 0
    while queue and states < options.maze_state_budget:
        _, cost, x, y, heading = heappop(queue)
        state = x, y, heading
        if best.get(state) != cost:
            continue
        states += 1
        p = point(x, y)
        edge = next((edge for edge, reached in (
            ('left', p.x_nm <= goal.min_x), ('right', p.x_nm >= goal.max_x),
            ('top', p.y_nm <= goal.min_y), ('bottom', p.y_nm >= goal.max_y))
            if reached and edge in eligible_edges), None)
        if edge is not None and (x or y):
            nodes = []
            cursor = state
            while cursor is not None:
                nodes.append(point(cursor[0], cursor[1]))
                cursor = parent[cursor]
            nodes.reverse()
            turns = [nodes[0]]
            for i in range(1, len(nodes)-1):
                a,b,c = nodes[i-1:i+2]
                if (b.x_nm-a.x_nm)*(c.y_nm-b.y_nm) != (b.y_nm-a.y_nm)*(c.x_nm-b.x_nm):
                    turns.append(b)
            turns.append(nodes[-1])
            path = tuple(TrackSegment(net, a, b, width, layer) for a,b in zip(turns, turns[1:]))
            return (edge, path), states
        for direction, (dx,dy) in enumerate(directions):
            if heading != -1 and (direction-heading) % 8 not in {0,1,7}:
                continue
            q = point(x+dx, y+dy)
            if not search.min_x <= q.x_nm <= search.max_x or not search.min_y <= q.y_nm <= search.max_y:
                continue
            ends = ((x,y), (x+dx,y+dy))
            key = min(ends), max(ends)
            if key not in legal_edges:
                # Search the same source-authorized width pieces that will be
                # emitted. A preferred-width probe can falsely close a narrow
                # package channel whose declared breakout neckdown is legal.
                pieces = clearance.route_pieces(net, p, q, width, layer)
                legal_edges[key] = bool(pieces) and all(
                    _track_inside_board(board, t.start, t.end, t.width_nm)
                    and clearance.can_track(net, t.start, t.end, t.width_nm, layer)
                    for t in pieces)
            if not legal_edges[key]:
                continue
            next_state = x+dx, y+dy, direction
            next_length = lengths[state] + (diagonal_length if dx and dy else step)
            if next_length > maximum_length:
                continue
            next_cost = cost + (14 if dx and dy else 10) + (3 if heading not in {-1,direction} else 0)
            if next_cost < best.get(next_state, 10**30):
                best[next_state], parent[next_state] = next_cost, state
                lengths[next_state] = next_length
                heappush(queue, (next_cost+heuristic(q), next_cost, x+dx, y+dy, direction))
    return None, states


def surface_escapes(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str, start: Point,
    layer: CopperLayer, area: Bounds, options: BoundaryAccessOptions | None = None,
    *, refined: bool = False,
) -> tuple[tuple[TrackSegment, ...], ...]:
    """Checked paths from a pad centre beyond its package collar on one layer.

    A pin whose net may not change layer gets no dogbone; it escapes on its own
    surface (plan R13). Ports, path orders and the maze are those of boundary
    witnesses: ``refined`` uses the fine port lattice and adds the maze, whose
    legs are re-cut and re-checked as routed pieces. Shortest paths first.
    """
    options = options or BoundaryAccessOptions()
    rule = next((r for r in board.net_routing_rules if r.net == net), None)
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    extension = max(options.collar_margin_nm, (width + 1) // 2 +
                    max(board.rules.minimum_clearance_nm, rule.clearance_nm or 0 if rule else 0))
    step, limit = ((options.refinement_step_nm, options.maximum_refined_ports_per_edge) if refined
                   else (options.port_step_nm, options.maximum_ports_per_edge))
    endpoints = tuple(_ports(start, area, extension, step, limit))
    nearest = min(abs(p.x_nm-start.x_nm) + abs(p.y_nm-start.y_nm) for _, p in endpoints)
    paths = []
    for _, end in endpoints:
        if abs(end.x_nm-start.x_nm) + abs(end.y_nm-start.y_nm) <= nearest + options.maximum_detour_nm:
            paths.extend(path for path in checked_access_paths(
                board, clearance, net, start, end, width, layer, allow_orthogonal=False) if path)
    if refined and options.maze_escapes:
        found, _ = _maze_path(board, clearance, net, start, width, layer, area, extension, options)
        if found is not None:
            pieces = tuple(piece for track in found[1]
                           for piece in clearance.route_pieces(net, track.start, track.end, width, layer))
            if all(_track_inside_board(board, t.start, t.end, t.width_nm)
                   and clearance.can_track(net, t.start, t.end, t.width_nm, layer) for t in pieces):
                paths.append(pieces)
    return tuple(sorted(dict.fromkeys(paths), key=lambda path: (
        sum(max(abs(t.end.x_nm-t.start.x_nm), abs(t.end.y_nm-t.start.y_nm)) for t in path),
        len(path), path[-1].end.x_nm, path[-1].end.y_nm)))


