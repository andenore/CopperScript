"""Bounded ordinary package-to-boundary capacity witnesses, not area routes.

Dogbones only prove access to a via. This stage verifies a connected launch,
then jointly allocates exact-clearance paths beyond a conservative package
collar on permitted signal layers. Witness copper is provisional and never
silently added to the board or passed off as end-to-end connectivity.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, replace
from heapq import heappop, heappush

from .drc import placed_pad_shape, run_physical_drc
from .escape_assignment import (EscapeAssignmentOptions, EscapeAssignmentReport,
                                EscapeCandidate, improve_escape_assignment)
from .fanout import FanoutResult
from .geometry import Bounds, bounds
from .physical import CopperLayer, PadReference, PhysicalBoard, Point, RouteKind, TrackSegment, nm_from_mm
from .pin_escape import checked_access_paths, verified_fanout_path
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers, signal_layer_preferences
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


@dataclass(frozen=True, slots=True)
class BoundaryPort:
    pad: PadReference
    position: Point
    layer: CopperLayer
    edge: str
    path: tuple[TrackSegment, ...]


@dataclass(frozen=True, slots=True)
class BoundaryPinAnalysis:
    pad: PadReference
    candidate_count: int
    diagnostic: str = ""
    maze_search_states: int = 0
    maze_candidate_count: int = 0


@dataclass(frozen=True, slots=True)
class BoundaryAccessResult:
    collars: tuple[PackageCollar, ...]
    ports: tuple[BoundaryPort, ...]
    pending_pads: tuple[PadReference, ...]
    pin_analysis: tuple[BoundaryPinAnalysis, ...]
    assignment: EscapeAssignmentReport
    native_accepted: bool
    options: BoundaryAccessOptions = BoundaryAccessOptions()

    @property
    def ready(self) -> bool:
        return self.native_accepted and not self.pending_pads


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
    directions = ((1,0), (1,1), (0,1), (-1,1), (-1,0), (-1,-1), (0,-1), (1,-1))
    def point(x, y):
        return Point(start.x_nm+x*step, start.y_nm+y*step)
    def heuristic(p):
        distance = max(0, min(p.x_nm-goal.min_x, goal.max_x-p.x_nm,
                              p.y_nm-goal.min_y, goal.max_y-p.y_nm))
        return 10 * ((distance+step-1)//step)
    queue = [(heuristic(start), 0, 0, 0, -1)]
    best = {(0,0,-1): 0}
    parent = {(0,0,-1): None}
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
        edge = ("left" if p.x_nm <= goal.min_x else "right" if p.x_nm >= goal.max_x
                else "top" if p.y_nm <= goal.min_y else "bottom" if p.y_nm >= goal.max_y else None)
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
                legal_edges[key] = (_track_inside_board(board, p, q, width)
                                    and clearance.can_track(net, p, q, width, layer))
            if not legal_edges[key]:
                continue
            next_state = x+dx, y+dy, direction
            next_cost = cost + (14 if dx and dy else 10) + (3 if heading not in {-1,direction} else 0)
            if next_cost < best.get(next_state, 10**30):
                best[next_state], parent[next_state] = next_cost, state
                heappush(queue, (next_cost+heuristic(q), next_cost, x+dx, y+dy, direction))
    return None, states


def analyze_boundary_access(
    board: PhysicalBoard, fanout: FanoutResult, options: BoundaryAccessOptions | None = None,
) -> BoundaryAccessResult:
    """Prove compatible local paths for allocated ordinary exits, or fail closed.

    The board includes all current critical/macro/selected-plane reservations.
    All candidates use exact board-edge, hole, keepout, track and via-obstacle
    checks. A failed bounded domain is *not* a proof of physical impossibility.
    No additional vias are generated and dedicated planes cannot carry signals.
    """
    options = options or BoundaryAccessOptions()
    # A witness cannot certify stale poses/rules or missing owned copper.
    for field in ("outline", "footprints", "placements", "nets", "rules", "stackup",
                  "net_routing_rules", "hard_macros", "materialized_macros"):
        if getattr(board, field) != getattr(fanout.board, field):
            raise ValueError("boundary access source/fanout geometry is stale")
    if (Counter(fanout.created_tracks) - Counter(board.tracks)
            or Counter(fanout.created_vias) - Counter(board.vias)):
        raise ValueError("boundary access requires all owned launch copper")
    required = tuple(sorted(fanout.accesses))
    collars = {ref: package_collar(board, ref, options.collar_margin_nm)
               for ref in sorted({p.component for p in required})}
    clearance = RoutingClearanceIndex(board)
    rules = {r.net: r for r in board.net_routing_rules}
    net_by_pad = {p: n.name for n in board.nets for p in n.pads}
    if any(p not in net_by_pad or (net_by_pad[p] in rules and
           rules[net_by_pad[p]].kind is not RouteKind.GENERAL) for p in required):
        raise ValueError("boundary witnesses require ordinary connected fanout pins")
    ranks, _ = signal_layer_preferences(board)
    candidates: dict[PadReference, dict[EscapeCandidate, BoundaryPort]] = {}
    diagnostics = {}
    maze_states, maze_counts, maze_cache = {}, {}, {}

    def generate(pad, step, limit, *, maze=False):
        net, start = net_by_pad[pad], fanout.accesses[pad]
        rule = rules.get(net)
        width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
        layers = board.stackup.copper_layers
        via = next((v for v in board.vias if v.net == net and v.position == start), None)
        if via is None or verified_fanout_path(board, pad, net, start, clearance) is None:
            diagnostics[pad] = "unverified connected launch"
            return ()
        span = range(layers.index(via.from_layer), layers.index(via.to_layer) + 1)
        allowed = tuple(l for l in routing_layers(board, net, rule) if layers.index(l) in span)
        area = collars[pad.component].bounds
        extension = max(options.collar_margin_nm, (width + 1) // 2 +
                        max(board.rules.minimum_clearance_nm, rule.clearance_nm or 0 if rule else 0))
        endpoints = tuple(_ports(start, area, extension, step, limit))
        nearest = min(abs(p.x_nm-start.x_nm) + abs(p.y_nm-start.y_nm) for _, p in endpoints)
        choices = candidates.setdefault(pad, {})
        for layer in sorted(allowed, key=lambda l: (ranks[l], layers.index(l))):
            for edge, end in endpoints:
                if abs(end.x_nm-start.x_nm) + abs(end.y_nm-start.y_nm) > nearest + options.maximum_detour_nm:
                    continue
                for path in checked_access_paths(board, clearance, net, start, end, width, layer,
                                                 allow_orthogonal=False):
                    if path:
                        candidate = path, None
                        choices[candidate] = BoundaryPort(pad, end, layer, edge, path)
        if maze and options.maze_escapes:
            if pad not in maze_cache:
                additions, expanded = [], 0
                for layer in sorted(allowed, key=lambda l: (ranks[l], layers.index(l))):
                    found, states = _maze_path(board, clearance, net, start, width, layer, area, extension, options)
                    expanded += states
                    if found is not None:
                        edge, path = found
                        additions.append(BoundaryPort(pad, path[-1].end, layer, edge, path))
                maze_cache[pad] = tuple(additions)
                maze_states[pad], maze_counts[pad] = expanded, len(additions)
            for port in maze_cache[pad]:
                choices[(port.path, None)] = port
        return tuple(sorted(choices, key=lambda c: (
            ranks[c[0][0].layer],
            sum(max(abs(t.end.x_nm-t.start.x_nm), abs(t.end.y_nm-t.start.y_nm))
                for t in c[0]), len(c[0]), layers.index(c[0][0].layer),
            c[0][-1].end.x_nm, c[0][-1].end.y_nm)))

    domains = {p: generate(p, options.port_step_nm, options.maximum_ports_per_edge) for p in required}
    def expand(pad):
        return generate(pad, options.refinement_step_nm, options.maximum_refined_ports_per_edge, maze=True)
    # Refine empty domains too: the assignment engine only repairs nonempty
    # domains blocked by other selected pins.
    for pad in required:
        if not domains[pad] and pad not in diagnostics:
            domains[pad] = generate(pad, options.refinement_step_nm, options.maximum_refined_ports_per_edge)
            if not domains[pad]:
                domains[pad] = expand(pad)
    ordered = tuple(sorted(required, key=lambda p: (len(domains[p]), p)))
    selected = {}
    index = RoutingClearanceIndex(board)
    for pad in ordered:
        for i, (path, _) in enumerate(domains[pad]):
            if all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in path):
                selected[pad] = i
                for t in path:
                    index.add_track(t, locked=True)
                break
    greedy = dict(selected)
    selected, assignment = improve_escape_assignment(board, ordered, domains, selected,
                                                       expand, options.assignment_options)

    def materialize(selection):
        index = RoutingClearanceIndex(board)
        accepted, tracks = [], []
        for pad in ordered:
            if pad not in selection:
                continue
            path, _ = domains[pad][selection[pad]]
            if not all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in path):
                return (), False
            accepted.append(candidates[pad][(path, None)])
            tracks.extend(path)
            for t in path:
                index.add_track(t, locked=True)
        # Ignore only expected open/area-incomplete findings, never geometry.
        witness = replace(board, tracks=(*board.tracks, *tracks))
        native = not any(f.severity.value == "error" and f.code not in
                         {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
                         for f in run_physical_drc(witness).findings)
        return tuple(accepted), native

    ports, native = materialize(selected)
    if not native:
        assignment = replace(assignment, native_accepted=False)
        ports, native = materialize(greedy)
    if not native:
        ports = ()
    accepted = {p.pad for p in ports}
    pending = tuple(p for p in required if p not in accepted)
    analysis = tuple(BoundaryPinAnalysis(p, len(domains[p]),
        ("whole boundary witness proposal rejected by native DRC" if not native else diagnostics.get(p) or (
         "bounded boundary domain exhausted" if not domains[p]
         else "compatible channel allocation not found within budget")) if p in pending else "",
        maze_states.get(p, 0), maze_counts.get(p, 0)) for p in required)
    return BoundaryAccessResult(tuple(collars.values()), ports, pending, analysis, assignment, native, options)
