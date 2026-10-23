"""Exact multi-layer escape of a ball grid by integer programming (escape plan, analytical step 2).

The negotiated router treats a dense ball grid like open board: its result
swings with tiny cost changes and grid paths fail the exact clearance check.
Inside the array the problem is small and regular, so it is solved exactly
here (Bayless et al., ICCAD 2016, solve multi-layer BGA escape with SAT):

* Ball layer: nodes are the cells between four balls (and one ring outside
  the array); an edge crosses the gap between two neighbouring balls and
  holds ``tracks(pitch - land)`` tracks while neither cell beside it holds a
  via (a lattice via sits on that crossing). A ball reaches its four cells.
* Via layers (inner and far surface): nodes are the ball positions (no
  lands there) and one ring outside; an edge crosses the gap between the
  two via sites at its ends and holds ``tracks(pitch - k * via / 2)``
  tracks for ``k`` occupied ends. A ball's own lattice via joins its cell to
  the four nodes around it on every via layer.
* Each signal ball is one commodity of one unit: from the ball to the
  outer ring on the side of the array facing its destination. Edge
  capacities bind all commodities together.

CP-SAT (OR-Tools, optional extra ``exact``) maximises the escaped balls,
then minimises length and layer changes. The result is a per-ball path
(layer and nodes); turning it into copper is a separate step. Track
geometry inside a node (turns, several tracks through one cell) is not
modelled beyond the edge capacities.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Mapping, Sequence

from .escape_capacity import GridGeometry


@dataclass(frozen=True, slots=True)
class EscapePath:
    ball: tuple[int, int]
    net: str
    layer: str
    nodes: tuple[tuple[int, int], ...]     # cells (ball layer) or ball positions (via layers)
    side: str


@dataclass(frozen=True, slots=True)
class ExactEscape:
    status: str
    demand: int
    escaped: int
    seconds: float
    paths: tuple[EscapePath, ...] = ()
    unescaped: tuple[tuple[int, int], ...] = ()
    by_layer: Mapping[str, int] = field(default_factory=dict)

    def report(self) -> dict:
        return {"status": self.status, "demand": self.demand, "escaped": self.escaped,
                "seconds": round(self.seconds, 1), "by_layer": dict(self.by_layer),
                "unescaped": [list(ball) for ball in self.unescaped]}


SIDES = ("left", "right", "top", "bottom")


def solve_exact_escape(geometry: GridGeometry, balls: Mapping[tuple[int, int], tuple[str, str]],
                       dogbones: Mapping[tuple[int, int], tuple[int, int]], vias: set[tuple[int, int]],
                       via_layers: Sequence[str], ball_layer: str, *, time_limit_s: float = 60.0,
                       workers: int = 8, edge_load: Mapping[tuple, int] | None = None,
                       side_cost: int = 30,
                       forbidden: Mapping[tuple[int, int], set] | None = None) -> ExactEscape:
    """Route ``balls`` ((column, row) -> (net, side)) out of the array.

    ``dogbones`` maps a ball to its own via cell; ``vias`` are all occupied
    via cells (every net, the obstacles). Sides are ``left`` (column -1),
    ``right`` (column nx), ``top`` (row -1) and ``bottom`` (row ny), in
    footprint-local grid coordinates. A ball may leave on any side; each
    side away from its own costs ``side_cost`` (about three steps) per
    quarter turn, as the route outside then goes around. ``edge_load``
    (same keys as the edges: (layer, a, b)) is capacity already taken by
    existing copper. ``forbidden`` maps a ball to (layer, node) pairs its
    path may not use (lazy cuts from earlier copper that failed there).
    """
    from time import perf_counter
    from ortools.sat.python import cp_model

    nx, ny = geometry.nx, geometry.ny
    started = perf_counter()
    model = cp_model.CpModel()
    # Graph: (layer, node) with undirected edges and capacities.
    edges: dict[tuple, int] = {}            # (layer, a, b) with a < b -> capacity
    # Ball layer: cells (cx, cy), cx in -1..nx-1, cy in -1..ny-1.
    for cx in range(-1, nx):
        for cy in range(-1, ny):
            for dx, dy in ((1, 0), (0, 1)):
                ox, oy = cx + dx, cy + dy
                if ox > nx - 1 or oy > ny - 1:
                    continue
                # The gap between the two balls this edge passes.
                if dx:   # cells side by side: the balls at (cx + 1, cy) and (cx + 1, cy + 1)
                    pair = ((cx + 1, cy), (cx + 1, cy + 1))
                else:    # cells one above the other: the balls at (cx, cy + 1) and (cx + 1, cy + 1)
                    pair = ((cx, cy + 1), (cx + 1, cy + 1))
                inside = [p for p in pair if 0 <= p[0] < nx and 0 <= p[1] < ny]
                if (cx, cy) in vias or (ox, oy) in vias:
                    capacity = 0
                elif len(inside) == 2:
                    capacity = geometry.tracks(geometry.pitch_nm - geometry.land_nm)
                else:
                    capacity = geometry.tracks(2 * geometry.pitch_nm - geometry.land_nm)
                if capacity:
                    edges[(ball_layer, (cx, cy), (ox, oy))] = capacity
    # Via layers: ball positions (ix, iy), ix in -1..nx, iy in -1..ny.
    for layer in via_layers:
        for ix in range(-1, nx + 1):
            for iy in range(-1, ny + 1):
                for dx, dy in ((1, 0), (0, 1)):
                    ox, oy = ix + dx, iy + dy
                    if ox > nx or oy > ny:
                        continue
                    # The two via sites at the ends of the crossing line.
                    ends = ((ix, iy - 1), (ix, iy)) if dx else ((ix - 1, iy), (ix, iy))
                    occupied = sum(1 for end in ends if end in vias)
                    capacity = geometry.tracks(geometry.pitch_nm - occupied * geometry.via_nm // 2)
                    if capacity:
                        edges[(layer, (ix, iy), (ox, oy))] = capacity

    for key, load in (edge_load or {}).items():
        if key in edges:
            edges[key] = max(0, edges[key] - load)
            if not edges[key]:
                del edges[key]
    turns = {("left", "right"): 2, ("top", "bottom"): 2}

    def turn(a, b):
        return 0 if a == b else turns.get((a, b), turns.get((b, a), 1))

    def outside(layer, node, side):
        x, y = node
        if layer == ball_layer:
            return {"left": x == -1, "right": x == nx - 1, "top": y == -1, "bottom": y == ny - 1}[side]
        return {"left": x == -1, "right": x == nx, "top": y == -1, "bottom": y == ny}[side]

    adjacency: dict[tuple, list[tuple]] = {}
    for (layer, a, b) in edges:
        adjacency.setdefault((layer, a), []).append((layer, b))
        adjacency.setdefault((layer, b), []).append((layer, a))
    flows: dict[tuple, dict] = {}           # ball -> {(u, v) or (u, sink, side): var}
    escaped_vars = {}
    side_penalty = []
    for ball, (net, side) in sorted(balls.items()):
        arcs = {}
        source = ("ball", ball)
        starts = []
        bx, by = ball
        for cell in ((bx - 1, by - 1), (bx, by - 1), (bx - 1, by), (bx, by)):
            if cell in vias and dogbones.get(ball) != cell:
                continue
            starts.append((ball_layer, cell))
        via = dogbones.get(ball)
        if via is not None:
            cx, cy = via
            for layer in via_layers:
                for node in ((cx, cy), (cx + 1, cy), (cx, cy + 1), (cx + 1, cy + 1)):
                    starts.append((layer, node))
        banned = (forbidden or {}).get(ball, set())
        for start in starts:
            if start not in banned:
                arcs[(source, start)] = model.NewBoolVar("")
        for (layer, a, b) in edges:
            for u, v in (((layer, a), (layer, b)), ((layer, b), (layer, a))):
                if v not in banned and u not in banned:
                    arcs[(u, v)] = model.NewBoolVar("")
        sink = ("sink", ball)
        exits = {}
        for node in list(adjacency):
            if node in banned:
                continue
            for exit_side in SIDES:
                if outside(node[0], node[1], exit_side):
                    var = model.NewBoolVar("")
                    arcs[(node, sink, exit_side)] = var
                    exits[var] = turn(side, exit_side)
        side_penalty.extend(side_cost * cost * var for var, cost in exits.items() if cost)
        out = model.NewBoolVar("")
        escaped_vars[ball] = out
        # Conservation.
        nodes = {key[0] for key in arcs} | {key[1] for key in arcs}
        incoming: dict = {node: [] for node in nodes}
        outgoing: dict = {node: [] for node in nodes}
        for key, var in arcs.items():
            outgoing[key[0]].append(var)
            incoming[key[1]].append(var)
        for node in nodes:
            if node == source:
                model.Add(sum(outgoing[node]) == out)
            elif node == sink:
                model.Add(sum(incoming[node]) == out)
            else:
                model.Add(sum(incoming[node]) == sum(outgoing[node]))
                model.Add(sum(outgoing[node]) <= 1)
        flows[ball] = arcs
    for (layer, a, b), capacity in edges.items():
        users = [arcs[(u, v)] for arcs in flows.values()
                 for u, v in (((layer, a), (layer, b)), ((layer, b), (layer, a))) if (u, v) in arcs]
        model.Add(sum(users) <= capacity)
    # Most balls out, then the shortest paths, then the fewest via-layer exits.
    length = [var for arcs in flows.values() for key, var in arcs.items() if key[0][0] != "ball" and key[1][0] != "sink"]
    layer_change = [var for ball, arcs in flows.items() for key, var in arcs.items()
                    if key[0][0] == "ball" and key[1][0] != ball_layer]
    model.Maximize(10_000 * sum(escaped_vars.values()) - 10 * sum(length) - 3 * sum(layer_change)
                   - sum(side_penalty))
    solver = cp_model.CpSolver()
    solver.parameters.max_time_in_seconds = time_limit_s
    solver.parameters.num_search_workers = workers
    status = solver.Solve(model)
    name = solver.StatusName(status)
    if status not in (cp_model.OPTIMAL, cp_model.FEASIBLE):
        return ExactEscape(name, len(balls), 0, perf_counter() - started, unescaped=tuple(sorted(balls)))
    paths, unescaped, by_layer = [], [], {}
    for ball, arcs in flows.items():
        if not solver.Value(escaped_vars[ball]):
            unescaped.append(ball)
            continue
        step = {key[0]: key for key, var in arcs.items() if solver.Value(var)}
        key, route = step[("ball", ball)], []
        while key[1][0] != "sink":
            route.append(key[1])
            key = step[key[1]]
        layer = route[0][0]
        paths.append(EscapePath(ball, balls[ball][0], layer, tuple(n[1] for n in route), key[2]))
        by_layer[layer] = by_layer.get(layer, 0) + 1
    return ExactEscape(name, len(balls), len(paths), perf_counter() - started, tuple(paths),
                       tuple(sorted(unescaped)), by_layer)


def board_exact_escape(board, component: str, *, via_layers: Sequence | None = None,
                       time_limit_s: float = 60.0, supplies_escape: bool = False,
                       only_nets: frozenset[str] | None = None,
                       forbidden: Mapping | None = None) -> ExactEscape:
    """Solve the exact escape of ``component`` on ``board`` with its current vias.

    Signal balls are chosen as in ``escape_capacity.board_certificate``; each
    leaves on the array side whose midpoint points most towards the centroid
    of its net's other pads.
    """
    from math import hypot
    from .escape_capacity import GridGeometry
    from .package_patterns import ball_grid
    from .physical import BoardSide, CopperLayer, Point
    from .placement import transformed_local_point
    from .power_islands import decoupled_rails
    from .routing_layers import dedicated_plane_layers
    from .zone_geometry import deferred_zone_nets

    grid = ball_grid(board, component)
    if grid is None:
        raise ValueError(f"{component} is not a ball grid")
    pose = next(p for p in board.placements if p.reference == component)
    poses = {p.reference: p for p in board.placements}
    side_layer = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
    planes = set(deferred_zone_nets(board)) | {zone.net for zone in board.zones}
    supplies = set(decoupled_rails(board))
    nx, ny, half = len(grid.xs_nm), len(grid.ys_nm), grid.pitch_nm // 2
    local = lambda x, y: transformed_local_point(pose, Point(x, y))
    centre = local((grid.xs_nm[0] + grid.xs_nm[-1]) // 2, (grid.ys_nm[0] + grid.ys_nm[-1]) // 2)
    mid_y, mid_x = (grid.ys_nm[0] + grid.ys_nm[-1]) // 2, (grid.xs_nm[0] + grid.xs_nm[-1]) // 2
    side_points = {"left": local(grid.xs_nm[0], mid_y), "right": local(grid.xs_nm[-1], mid_y),
                   "top": local(mid_x, grid.ys_nm[0]), "bottom": local(mid_x, grid.ys_nm[-1])}
    sites = {}
    for cx in range(-1, nx):
        for cy in range(-1, ny):
            point = local(grid.xs_nm[0] + cx * grid.pitch_nm + half, grid.ys_nm[0] + cy * grid.pitch_nm + half)
            sites[(point.x_nm, point.y_nm)] = (cx, cy)
    tolerance = grid.tolerance_nm + 1
    vias, via_net = set(), {}
    for via in board.vias:
        for (x, y), key in sites.items():
            if abs(via.position.x_nm - x) <= tolerance and abs(via.position.y_nm - y) <= tolerance:
                vias.add(key)
                via_net[key] = via.net
    def pad_point(pad):
        pose_ = poses[pad.component]
        land = next(l for l in board.footprints[pose_.footprint].pads if l.number == pad.pad)
        return transformed_local_point(pose_, land.position)
    rules = {rule.net: rule for rule in board.net_routing_rules}
    balls, dogbones = {}, {}
    for net in board.nets:
        if net.name in planes or (net.name in supplies and not supplies_escape):
            continue
        rule = rules.get(net.name)
        if rule is not None and rule.kind.value != "general":
            continue  # critical routers own these escapes; their copper is load below
        if only_nets is not None and net.name not in only_nets:
            continue
        mine = [pad for pad in net.pads if pad.component == component and pad.pad in grid.balls]
        others = [pad_point(pad) for pad in net.pads if pad.component != component]
        if not mine or not others:
            continue
        tx = sum(p.x_nm for p in others) / len(others) - centre.x_nm
        ty = sum(p.y_nm for p in others) / len(others) - centre.y_nm
        norm = hypot(tx, ty) or 1.0
        def score(side):
            point = side_points[side]
            sx, sy = point.x_nm - centre.x_nm, point.y_nm - centre.y_nm
            return (sx * tx + sy * ty) / ((hypot(sx, sy) or 1.0) * norm)
        side = max(("left", "right", "top", "bottom"), key=score)
        lands = {land.number: land for land in board.footprints[pose.footprint].pads}
        for pad in mine:
            ball = grid.index(lands[pad.pad].position)
            balls[ball] = (net.name, side)
            ix, iy = ball
            for cell in ((ix - 1, iy - 1), (ix, iy - 1), (ix - 1, iy), (ix, iy)):
                if via_net.get(cell) == net.name:
                    dogbones[ball] = cell
                    break
    if via_layers is None:
        dedicated = dedicated_plane_layers(board)
        via_layers = tuple(l for l in board.stackup.copper_layers if l is not side_layer and l not in dedicated)
    geometry = GridGeometry(nx, ny, grid.pitch_nm, grid.land_diameter_nm, board.rules.default_via_size_nm,
                            board.rules.minimum_track_width_nm, board.rules.minimum_clearance_nm)
    # Capacity existing copper already takes: a track across an edge's
    # crossing line takes one track; a via off the lattice near it, all.
    ball_point = lambda ix, iy: local(grid.xs_nm[0] + ix * grid.pitch_nm, grid.ys_nm[0] + iy * grid.pitch_nm)
    cell_point = lambda cx, cy: local(grid.xs_nm[0] + cx * grid.pitch_nm + half, grid.ys_nm[0] + cy * grid.pitch_nm + half)
    crossings = {}
    for cx in range(-1, nx):
        for cy in range(-1, ny):
            for dx, dy in ((1, 0), (0, 1)):
                pair = ((cx + 1, cy), (cx + 1, cy + 1)) if dx else ((cx, cy + 1), (cx + 1, cy + 1))
                crossings[(side_layer.value, (cx, cy), (cx + dx, cy + dy))] = (side_layer, ball_point(*pair[0]), ball_point(*pair[1]))
    for layer in via_layers:
        for ix in range(-1, nx + 1):
            for iy in range(-1, ny + 1):
                for dx, dy in ((1, 0), (0, 1)):
                    ends = ((ix, iy - 1), (ix, iy)) if dx else ((ix - 1, iy), (ix, iy))
                    crossings[(layer.value, (ix, iy), (ix + dx, iy + dy))] = (layer, cell_point(*ends[0]), cell_point(*ends[1]))
    lattice = {(cell_point(*cell).x_nm, cell_point(*cell).y_nm) for cell in vias}
    reach = board.rules.default_via_size_nm / 2 + board.rules.minimum_clearance_nm + board.rules.minimum_track_width_nm / 2
    box_x0 = min(p.x_nm for _, a, b in crossings.values() for p in (a, b))
    box_x1 = max(p.x_nm for _, a, b in crossings.values() for p in (a, b))
    box_y0 = min(p.y_nm for _, a, b in crossings.values() for p in (a, b))
    box_y1 = max(p.y_nm for _, a, b in crossings.values() for p in (a, b))
    near = lambda p: box_x0 - grid.pitch_nm <= p.x_nm <= box_x1 + grid.pitch_nm and box_y0 - grid.pitch_nm <= p.y_nm <= box_y1 + grid.pitch_nm
    tracks = [t for t in board.tracks if near(t.start) or near(t.end)]
    loose = [v for v in board.vias if near(v.position) and not any(
        abs(v.position.x_nm - x) <= tolerance and abs(v.position.y_nm - y) <= tolerance for x, y in lattice)]
    edge_load = {}
    for key, (layer, a, b) in crossings.items():
        load = sum(1 for t in tracks if t.layer is layer and _cross(t.start, t.end, a, b))
        if any(_segment_point_distance(v.position, a, b) < reach for v in loose):
            load = 1_000
        if load:
            edge_load[key] = load
    return solve_exact_escape(geometry, balls, dogbones, vias, tuple(l.value for l in via_layers), side_layer.value,
                              time_limit_s=time_limit_s, edge_load=edge_load, forbidden=forbidden)


def _cross(p, q, a, b) -> bool:
    """Whether segments pq and ab cross properly (not merely touch at an end)."""
    def orient(u, v, w):
        value = (v.x_nm - u.x_nm) * (w.y_nm - u.y_nm) - (v.y_nm - u.y_nm) * (w.x_nm - u.x_nm)
        return (value > 0) - (value < 0)
    return orient(p, q, a) * orient(p, q, b) < 0 and orient(a, b, p) * orient(a, b, q) < 0


def _segment_point_distance(point, a, b) -> float:
    from math import hypot
    ax, ay, bx, by = a.x_nm, a.y_nm, b.x_nm, b.y_nm
    dx, dy = bx - ax, by - ay
    length = dx * dx + dy * dy
    t = 0.0 if not length else max(0.0, min(1.0, ((point.x_nm - ax) * dx + (point.y_nm - ay) * dy) / length))
    return hypot(point.x_nm - (ax + t * dx), point.y_nm - (ay + t * dy))


@dataclass(frozen=True, slots=True)
class EscapeCopper:
    board: object                      # the board with the accepted escape tracks added
    tracks: tuple                      # TrackSegment
    accepted: tuple[str, ...]          # nets whose escape is copper now
    rejected: tuple[str, ...]          # nets whose escape failed the exact check
    solve: ExactEscape
    reasons: Mapping[str, tuple] = field(default_factory=dict)   # net -> (layer, part, blockers)
    failures: Mapping[tuple[int, int], tuple] = field(default_factory=dict)   # ball -> (layer, node) it failed at

    def report(self) -> dict:
        return {**self.solve.report(), "copper_nets": len(self.accepted), "rejected_nets": list(self.rejected),
                "tracks": len(self.tracks), "reasons": {net: list(map(str, why)) for net, why in self.reasons.items()}}


def exact_escape_copper(board, component: str, *, time_limit_s: float = 60.0,
                        via_layers: Sequence | None = None, rounds: int = 6) -> EscapeCopper:
    """Exact escapes as checked copper, re-solving the rejected balls around the accepted copper.

    Each round solves the escape of the balls still without copper on the
    board with all copper accepted so far (as edge load), turns it into
    copper (``_escape_copper_round``) and stops when a round adds nothing.
    """
    current, tracks, accepted, reasons, first = board, [], [], {}, None
    only, forbidden, rejected = None, {}, ()
    for _ in range(max(1, rounds)):
        result = _escape_copper_round(current, component, time_limit_s=time_limit_s,
                                      via_layers=via_layers, only_nets=only, forbidden=forbidden)
        first = first or result.solve
        missing = [ball for ball in result.solve.unescaped]
        if missing:
            reasons.update({f"ball {ball}": ("", "model", ["no escape left after cuts"]) for ball in missing})
        current = result.board
        tracks.extend(result.tracks)
        accepted.extend(result.accepted)
        reasons.update(result.reasons)
        rejected = result.rejected
        if not rejected:
            break
        # Lazy cuts: a ball whose copper failed may not use that node again.
        for ball, cut in result.failures.items():
            forbidden.setdefault(ball, set()).add(cut)
        only = frozenset(rejected)
    return EscapeCopper(current, tuple(tracks), tuple(accepted), tuple(rejected), first, reasons)


def _escape_copper_round(board, component: str, *, time_limit_s: float = 60.0,
                         via_layers: Sequence | None = None,
                         only_nets: frozenset[str] | None = None,
                         forbidden: Mapping | None = None) -> EscapeCopper:
    """Solve ``component``'s exact escape and add it to ``board`` as checked copper.

    Each path becomes a polyline from the ball (ball layer) or its lattice via
    (via layers) through one lane per gap it crosses to a point one pitch
    outside the array. A gap's users get lanes centred on the gap's
    centre line, one track width plus clearance apart. Each escape is checked
    exactly against the board and the escapes accepted before it (shortest
    first); one that fails is left to the router.
    """
    from .package_patterns import ball_grid
    from .physical import CopperLayer, Point, TrackSegment
    from .placement import transformed_local_point
    from .routing_clearance import RoutingClearanceIndex

    solve = board_exact_escape(board, component, time_limit_s=time_limit_s, via_layers=via_layers,
                               only_nets=only_nets, forbidden=forbidden)
    grid = ball_grid(board, component)
    pose = next(p for p in board.placements if p.reference == component)
    pitch, half = grid.pitch_nm, grid.pitch_nm // 2
    x0, y0 = grid.xs_nm[0], grid.ys_nm[0]
    local = lambda x, y: transformed_local_point(pose, Point(int(round(x)), int(round(y))))
    ball_layer = solve.paths[0].layer if False else None
    rules = {rule.net: rule for rule in board.net_routing_rules}
    width_floor = board.rules.minimum_track_width_nm
    step = board.rules.minimum_track_width_nm + board.rules.minimum_clearance_nm
    directions = {"left": (-1, 0), "right": (1, 0), "top": (0, -1), "bottom": (0, 1)}
    layers = {layer.value: layer for layer in board.stackup.copper_layers}
    from .physical import BoardSide
    surface = (CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK).value

    def centre(layer, node):
        x, y = node
        return (x0 + x * pitch + half, y0 + y * pitch + half) if layer == surface else (x0 + x * pitch, y0 + y * pitch)

    def crossing(layer, a, b):
        """Local endpoints of the gap line crossed between nodes a and b."""
        (ax, ay), (bx, by) = sorted((a, b))
        if layer == surface:
            pair = ((ax + 1, ay), (ax + 1, ay + 1)) if bx != ax else ((ax, ay + 1), (ax + 1, ay + 1))
            return [(x0 + px * pitch, y0 + py * pitch) for px, py in pair]
        ends = ((ax, ay - 1), (ax, ay)) if bx != ax else ((ax - 1, ay), (ax, ay))
        return [(x0 + cx * pitch + half, y0 + cy * pitch + half) for cx, cy in ends]

    # Lanes: the users of each gap, in a stable order.
    users: dict[tuple, list] = {}
    for path in solve.paths:
        for a, b in zip(path.nodes, path.nodes[1:]):
            users.setdefault((path.layer, *sorted((a, b))), []).append(path.ball)

    def lane(layer, a, b, ball):
        key = (layer, *sorted((a, b)))
        crowd = sorted(users[key])
        (px, py), (qx, qy) = crossing(layer, a, b)
        mx, my = (px + qx) / 2, (py + qy) / 2
        length = ((qx - px) ** 2 + (qy - py) ** 2) ** 0.5 or 1.0
        offset = (crowd.index(ball) - (len(crowd) - 1) / 2) * step
        return mx + (qx - px) / length * offset, my + (qy - py) / length * offset

    index = RoutingClearanceIndex(board)
    accepted, rejected, tracks = [], [], []
    reasons: dict[str, tuple] = {}
    failures: dict[tuple[int, int], tuple] = {}
    lattice = {}
    for via in board.vias:
        lattice.setdefault(via.net, []).append(via.position)
    for path in sorted(solve.paths, key=lambda p: (len(p.nodes), p.ball)):
        net = path.net
        rule = rules.get(net)
        width = max(width_floor, (rule.breakout_width_nm if rule and rule.breakout_width_nm else width_floor))
        if path.layer == surface:
            bx, by = path.ball
            start = (x0 + bx * pitch, y0 + by * pitch)
        else:
            # Start on the ball's own via (the cell the solver used).
            cells = [(cx, cy) for cx in (path.nodes[0][0] - 1, path.nodes[0][0]) for cy in (path.nodes[0][1] - 1, path.nodes[0][1])]
            candidates = [(x0 + cx * pitch + half, y0 + cy * pitch + half) for cx, cy in cells]
            start = next((c for c in candidates if any(
                abs(local(*c).x_nm - v.x_nm) <= 2_000 and abs(local(*c).y_nm - v.y_nm) <= 2_000
                for v in lattice.get(net, ()))), None)
            if start is None:
                rejected.append(net)
                reasons[net] = (path.layer, "start", ["own via not found"])
                continue
        lanes = [lane(path.layer, a, b, path.ball) for a, b in zip(path.nodes, path.nodes[1:])]
        first, last = centre(path.layer, path.nodes[0]), centre(path.layer, path.nodes[-1])
        dx, dy = directions[path.side]
        if lanes:
            tail = lanes[-1]
        else:
            tail = last
        layer = layers[path.layer]
        # Variants, most guided first: through the first node's centre or
        # straight to the first lane; the exit half a pitch, a quarter or
        # nothing past the last lane (or node) towards the exit side.
        segments = None
        for via_first in (True, False):
            for reach in (half, half // 2, 0):
                points = [start, *([first] if via_first else []), *lanes,
                          (tail[0] + dx * (reach + (half if not lanes else 0)), tail[1] + dy * (reach + (half if not lanes else 0)))]
                trial = [TrackSegment(net, local(*a), local(*b), width, layer)
                         for a, b in zip(points, points[1:]) if a != b]
                if trial and all(index.can_track(net, t.start, t.end, t.width_nm, t.layer) for t in trial):
                    segments = trial
                    break
            if segments:
                break
        if segments is None:
            # Exact local maze from the start to the exit, within the planned
            # corridor (pin_access.local_access_path: legal by construction).
            from .pin_access import local_access_path
            exit_point = (tail[0] + dx * half, tail[1] + dy * half)
            found = local_access_path(board, index, net, local(*start), local(*exit_point), layer, width,
                                      step_nm=25_000, detour_nm=pitch, maximum_states=60_000)
            if found:
                segments = list(found)
        if segments:
            for t in segments:
                index.add_track(t)
            tracks.extend(segments)
            accepted.append(net)
        else:
            rejected.append(net)
            points = [start, first, *lanes, (tail[0] + dx * half, tail[1] + dy * half)]
            segments = [TrackSegment(net, local(*a), local(*b), width, layer) for a, b in zip(points, points[1:]) if a != b]
            position = next(k for k, t in enumerate(segments)
                            if not index.can_track(net, t.start, t.end, t.width_nm, t.layer))
            records = index.track_blocker_records(segments[position])
            what = sorted({f"{r.reason}:{r.identity.split(':')[0]}:{r.net or ''}" for r in records})[:3]
            part = "start" if position == 0 else ("exit" if position == len(segments) - 1 else "middle")
            reasons[net] = (path.layer, part, what)
            # The node the failing segment leaves from (segment k starts at node k - 1 after the start).
            node = path.nodes[min(len(path.nodes) - 1, max(0, position - 1))]
            failures[path.ball] = (path.layer, node)
    from dataclasses import replace
    result = replace(board, tracks=(*board.tracks, *tracks)) if tracks else board
    return EscapeCopper(result, tuple(tracks), tuple(accepted), tuple(rejected), solve, reasons, failures)
