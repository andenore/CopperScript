"""Escape capacity certificate for a ball grid (escape plan E10, analytical).

Before any routing, this answers whether a ball grid's signals can leave the
array at all under the design rules, and where it is tight. The array is
cut by nested rings: a ball in ring r (r outer rings outside it) crosses the
cuts r, r-1, ..., 1 on one layer to leave the field.

* On a layer without lands (inner layers, the far surface) the obstacles
  are the through vias of the via lattice. A track crosses a cut between
  two vias of the cell ring on that cut; ``k`` empty sites between two vias
  leave a gap of ``(k + 1) * pitch - via`` that holds
  ``floor((gap - clearance) / (width + clearance))`` tracks.
* On the ball layer the obstacles are the lands, the dogbone stubs and the
  vias. A track crosses a cut between two neighbouring balls of the ring
  outside it; the gap holds ``floor((pitch - land - clearance) / (width +
  clearance))`` tracks, but only while neither cell beside it holds a via
  (a lattice via sits on that crossing).

A ball with its own dogbone via may leave on any via layer; a ball without
one leaves on the ball layer. A max-flow over (layer, ring) chains whose
links carry the cut capacities gives the most balls that can escape and
the saturated cuts. Plane and supply balls that reach planes or islands
through their via need no channel. This is a necessary condition for
routing (each crossing is counted once; turns, the region outside the array
and parts on the far side are not modelled), so it bounds every router.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Collection, Mapping, Sequence


@dataclass(frozen=True, slots=True)
class GridGeometry:
    nx: int
    ny: int
    pitch_nm: int
    land_nm: int
    via_nm: int
    width_nm: int
    clearance_nm: int

    def tracks(self, gap_nm: int) -> int:
        """Tracks of ``width_nm`` that fit side by side in a copper-free gap."""
        return max(0, (gap_nm - self.clearance_nm) // (self.width_nm + self.clearance_nm))


@dataclass(frozen=True, slots=True)
class CutCapacity:
    layer: str
    ring: int            # the cut between ring ``ring`` and ring ``ring - 1``
    capacity: int
    used: int

    @property
    def saturated(self) -> bool:
        return self.used >= self.capacity


@dataclass(frozen=True, slots=True)
class EscapeCertificate:
    demand: int
    escaped: int
    rings: int
    cuts: tuple[CutCapacity, ...]
    # ring -> (signal balls, escaped)
    by_ring: Mapping[int, tuple[int, int]] = field(default_factory=dict)
    # layer -> balls leaving on it
    by_layer: Mapping[str, int] = field(default_factory=dict)

    @property
    def feasible(self) -> bool:
        return self.escaped == self.demand

    def report(self) -> dict:
        return {"demand": self.demand, "escaped": self.escaped, "feasible": self.feasible,
                "by_ring": {ring: {"signals": total, "escaped": done} for ring, (total, done) in sorted(self.by_ring.items())},
                "by_layer": dict(self.by_layer),
                "cuts": [{"layer": cut.layer, "ring": cut.ring, "capacity": cut.capacity, "used": cut.used,
                          "saturated": cut.saturated} for cut in self.cuts]}


def ring_of(ix: int, iy: int, nx: int, ny: int) -> int:
    return min(ix, iy, nx - 1 - ix, ny - 1 - iy)


def _loop(cells: Collection[tuple[int, int]]) -> list[tuple[int, int]]:
    """The cells of one square ring in order around it (8-neighbour steps)."""
    remaining = set(cells)
    if not remaining:
        return []
    start = min(remaining)
    order, current = [start], start
    remaining.discard(start)
    while remaining:
        x, y = current
        step = next(((x + dx, y + dy) for dx, dy in ((1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (-1, 1), (1, -1), (-1, -1))
                     if (x + dx, y + dy) in remaining), None)
        if step is None:
            break
        order.append(step)
        remaining.discard(step)
        current = step
    return order


def _via_layer_capacity(geometry: GridGeometry, cut: int, vias: Collection[tuple[int, int]]) -> int:
    """Tracks crossing the cell ring of ``cut`` between its vias, on a layer without lands."""
    nx, ny = geometry.nx, geometry.ny
    ring = cut - 1
    cells = [(cx, cy) for cx in range(nx - 1) for cy in range(ny - 1)
             if min(cx, cy, nx - 2 - cx, ny - 2 - cy) == ring]
    loop = _loop(cells)
    if not loop:
        return 0
    occupied = [cell in vias for cell in loop]
    if not any(occupied):
        return geometry.tracks(len(loop) * geometry.pitch_nm)
    first = occupied.index(True)
    total, run = 0, 0
    for k in range(1, len(loop) + 1):
        if occupied[(first + k) % len(loop)]:
            total += geometry.tracks((run + 1) * geometry.pitch_nm - geometry.via_nm)
            run = 0
        else:
            run += 1
    return total


def _ball_layer_capacity(geometry: GridGeometry, cut: int, vias: Collection[tuple[int, int]]) -> int:
    """Tracks crossing between the balls of ring ``cut - 1`` where no via sits beside the gap."""
    nx, ny = geometry.nx, geometry.ny
    ring = cut - 1
    per_gap = geometry.tracks(geometry.pitch_nm - geometry.land_nm)
    total = 0
    for ix in range(nx):
        for iy in range(ny):
            if ring_of(ix, iy, nx, ny) != ring:
                continue
            # The neighbour along the ring to the right and below (each gap once).
            for dx, dy in ((1, 0), (0, 1)):
                jx, jy = ix + dx, iy + dy
                if not (0 <= jx < nx and 0 <= jy < ny) or ring_of(jx, jy, nx, ny) != ring:
                    continue
                beside = ((ix, iy - 1), (ix, iy)) if dx else ((ix - 1, iy), (ix, iy))
                if not any(cell in vias for cell in beside):
                    total += per_gap
    return total


def escape_certificate(geometry: GridGeometry, signals: Collection[tuple[int, int]],
                       dogbones: Collection[tuple[int, int]], vias: Collection[tuple[int, int]],
                       via_layers: Sequence[str], ball_layer: str) -> EscapeCertificate:
    """Most of the ``signals`` balls that can leave the array, by max-flow over the ring cuts.

    ``dogbones`` are the signal balls with a via of their own (they may leave
    on any of ``via_layers``); the others leave on ``ball_layer``. ``vias``
    are the occupied lattice cells (every net), the obstacles of both kinds
    of layer.
    """
    from .escape_plan import _MinCostFlow
    nx, ny = geometry.nx, geometry.ny
    rings = (min(nx, ny) + 1) // 2
    vias = set(vias)
    layers = [ball_layer, *via_layers]
    capacity = {}
    for cut in range(1, rings):
        capacity[(ball_layer, cut)] = _ball_layer_capacity(geometry, cut, vias)
        for layer in via_layers:
            capacity[(layer, cut)] = _via_layer_capacity(geometry, cut, vias)
    balls = sorted(signals)
    node = {("ball", ball): k for k, ball in enumerate(balls)}
    for layer in layers:
        for ring in range(rings):
            node[(layer, ring)] = len(node)
    source, sink = len(node), len(node) + 1
    flow = _MinCostFlow(len(node) + 2)
    entries = {}
    for ball in balls:
        index = node[("ball", ball)]
        flow.add(source, index, 1, 0)
        ring = ring_of(*ball, nx, ny)
        choices = via_layers if ball in dogbones else (ball_layer,)
        entries[ball] = [(layer, flow.add(index, node[(layer, ring)], 1, 0)) for layer in choices]
    links = {}
    for layer in layers:
        for ring in range(rings - 1, 0, -1):
            links[(layer, ring)] = flow.add(node[(layer, ring)], node[(layer, ring - 1)], capacity[(layer, ring)], 0)
        flow.add(node[(layer, 0)], sink, len(balls), 0)
    escaped, _ = flow.run(source, sink)
    cuts = tuple(CutCapacity(layer, ring, capacity[(layer, ring)],
                             flow.edge_flow(node[(layer, ring)], links[(layer, ring)]))
                 for layer in layers for ring in range(1, rings))
    by_ring: dict[int, list[int]] = {}
    by_layer: dict[str, int] = {}
    for ball in balls:
        ring = ring_of(*ball, nx, ny)
        done = [layer for layer, edge in entries[ball] if flow.edge_flow(node[("ball", ball)], edge)]
        counts = by_ring.setdefault(ring, [0, 0])
        counts[0] += 1
        counts[1] += bool(done)
        for layer in done:
            by_layer[layer] = by_layer.get(layer, 0) + 1
    return EscapeCertificate(len(balls), escaped, rings, cuts,
                             {ring: tuple(value) for ring, value in by_ring.items()}, by_layer)


def board_certificate(board, component: str, *, via_layers: Sequence | None = None,
                      supplies_escape: bool = False) -> EscapeCertificate:
    """The certificate of one ball grid on ``board`` with its current vias.

    Signal balls are those of nets without a zone (plane) and, unless
    ``supplies_escape``, not decoupled supply rails (those reach their
    planes or islands through their own via). A via on a lattice cell
    centre of the ball's net is its dogbone. Via layers default to the
    copper layers other than the ball layer that are not dedicated planes.
    """
    from .package_patterns import ball_grid
    from .physical import BoardSide, CopperLayer
    from .placement import transformed_local_point
    from .power_islands import decoupled_rails
    from .routing_layers import dedicated_plane_layers
    from .zone_geometry import deferred_zone_nets
    from .physical import Point

    grid = ball_grid(board, component)
    if grid is None:
        raise ValueError(f"{component} is not a ball grid")
    pose = next(p for p in board.placements if p.reference == component)
    side = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
    planes = set(deferred_zone_nets(board)) | {zone.net for zone in board.zones}
    supplies = set(decoupled_rails(board))
    net_of = {(pad.component, pad.pad): net.name for net in board.nets for pad in net.pads}
    nx, ny, half = len(grid.xs_nm), len(grid.ys_nm), grid.pitch_nm // 2
    lands = {land.number: land for land in board.footprints[pose.footprint].pads if land.number in grid.balls}
    sites = {}
    for cx in range(-1, nx):
        for cy in range(-1, ny):
            point = transformed_local_point(pose, Point(grid.xs_nm[0] + cx * grid.pitch_nm + half,
                                                        grid.ys_nm[0] + cy * grid.pitch_nm + half))
            sites[(point.x_nm, point.y_nm)] = (cx, cy)
    tolerance = grid.tolerance_nm + 1
    vias, via_net = set(), {}
    for via in board.vias:
        for (x, y), key in sites.items():
            if abs(via.position.x_nm - x) <= tolerance and abs(via.position.y_nm - y) <= tolerance:
                vias.add(key)
                via_net[key] = via.net
    signals, dogbones = set(), set()
    for number, land in lands.items():
        net = net_of.get((component, number))
        if net is None or net in planes or (net in supplies and not supplies_escape):
            continue
        ball = grid.index(land.position)
        signals.add(ball)
        ix, iy = ball
        if any(via_net.get((ix - dx, iy - dy)) == net for dx in (0, 1) for dy in (0, 1)):
            dogbones.add(ball)
    if via_layers is None:
        dedicated = dedicated_plane_layers(board)
        via_layers = tuple(layer for layer in board.stackup.copper_layers if layer is not side and layer not in dedicated)
    geometry = GridGeometry(nx, ny, grid.pitch_nm, grid.land_diameter_nm, board.rules.default_via_size_nm,
                            board.rules.minimum_track_width_nm, board.rules.minimum_clearance_nm)
    return escape_certificate(geometry, signals, dogbones, vias, tuple(layer.value for layer in via_layers), side.value)
