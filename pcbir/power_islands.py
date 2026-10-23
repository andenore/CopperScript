"""Automatic split-plane islands for supply rails (routing refactor plan, P3).

Given one plane layer (usually a ground layer that may be shared), each
supply rail gets copper islands around its own pads and vias. Islands are
carved out of the layer's existing plane by giving them a higher zone
priority, so the plane's own fill keeps everything else. The router then
treats each rail as a plane net and only taps its pads into the islands.

The partition is a raster Voronoi split: a cell belongs to the nearest rail
whose pad lies within ``reach``; cells reached by no rail stay with the host
plane. Neighbouring islands keep ``gap`` apart. Islands without a pad or via
of their rail are dropped.

This is deliberately simple and deterministic. It does not prove current
capacity or return paths; the plane estimate and native refill report the
resulting connectivity.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from .physical import (CopperLayer, CopperZone, IslandPolicy, PhysicalBoard, Point, PolygonRing,
                       PolygonWithHoles, ZoneConnection, nm_from_mm)


@dataclass(frozen=True, slots=True)
class PowerPartition:
    layer: CopperLayer
    zones: tuple[CopperZone, ...]
    rails: tuple[str, ...]
    skipped: tuple[str, ...]

    def report(self) -> dict:
        return {"layer": self.layer.value, "rails": list(self.rails), "islands": len(self.zones),
                "skipped": list(self.skipped)}


def decoupled_rails(board: PhysicalBoard) -> tuple[str, ...]:
    """Supply nets that have decoupling capacitors and no zone of their own."""
    zoned = {zone.net for zone in board.zones}
    returns = {link.return_net for link in board.decoupling_links}
    return tuple(sorted({link.net for link in board.decoupling_links} - zoned - returns))


def ball_grid_rails(board: PhysicalBoard) -> tuple[str, ...]:
    """Decoupled rails with a pad on a full ball grid: their balls need plane access.

    Under a dense ball grid a rail routed as tracks crosses the dogbone via
    columns the signals escape through; an island under the array gives each
    supply ball a via straight into its rail (the FX10 kit's split PWR1 layer).
    """
    from .package_patterns import ball_grid
    balls = set()
    for pose in board.placements:
        grid = ball_grid(board, pose.reference)
        if grid is not None:
            balls |= {(pose.reference, number) for number in grid.balls}
    pads = {net.name: net.pads for net in board.nets}
    return tuple(rail for rail in decoupled_rails(board)
                 if any((pad.component, pad.pad) in balls for pad in pads.get(rail, ())))


def auto_power_layer(board: PhysicalBoard) -> CopperLayer | None:
    """A plane layer that can host supply islands without losing a reference.

    Only a net with two or more dedicated plane layers gives one up; the one
    farthest from the front copper (the usual dense side) is chosen.
    """
    from .routing_layers import dedicated_plane_layers
    planes = dedicated_plane_layers(board)
    by_net: dict[str, list[CopperLayer]] = {}
    for layer, net in planes.items():
        by_net.setdefault(net, []).append(layer)
    order = board.stackup.copper_layers
    candidates = [max(layers, key=order.index) for layers in by_net.values() if len(layers) >= 2]
    return max(candidates, key=order.index) if candidates else None


def plan_power_islands(board: PhysicalBoard, layer: CopperLayer, rails: tuple[str, ...] | None = None, *,
                       reach_nm: int = nm_from_mm("1.6"), gap_nm: int = nm_from_mm("0.3"),
                       corridor_nm: int = nm_from_mm("1.6"), link_nm: int = nm_from_mm("12"),
                       pitch_nm: int = nm_from_mm("0.1")) -> PowerPartition:
    from .placement import transformed_local_point
    from .unified_router import _polygon_inside, _segment_distance

    rails = tuple(rails) if rails is not None else decoupled_rails(board)
    host = [zone for zone in board.zones if layer in zone.layers]
    if not rails or not host:
        return PowerPartition(layer, (), (), tuple(rails))
    vertices = board.outline.vertices
    x0 = min(p.x_nm for p in vertices); y0 = min(p.y_nm for p in vertices)
    x1 = max(p.x_nm for p in vertices); y1 = max(p.y_nm for p in vertices)
    nx = int((x1 - x0) // pitch_nm) + 1
    ny = int((y1 - y0) // pitch_nm) + 1
    xs = x0 + pitch_nm * (np.arange(nx) + 0.5)
    ys = y0 + pitch_nm * (np.arange(ny) + 0.5)
    px, py = np.meshgrid(xs, ys)
    allowed = np.zeros(px.shape, dtype=bool)
    for zone in host:
        inside = _polygon_inside(zone.outline.outer.vertices, px, py)
        for hole in zone.outline.holes:
            inside &= ~_polygon_inside(hole.vertices, px, py)
        allowed |= inside
    edge = np.full(px.shape, np.inf)
    for i in range(len(vertices)):
        a, b = vertices[i], vertices[(i + 1) % len(vertices)]
        edge = np.minimum(edge, _segment_distance(px, py, float(a.x_nm), float(a.y_nm), float(b.x_nm), float(b.y_nm)))
    allowed &= edge > board.rules.minimum_clearance_nm + pitch_nm
    for keepout in board.copper_keepouts:
        if keepout.block_zones and layer in keepout.layers:
            allowed &= ~_polygon_inside(keepout.outline.outer.vertices, px, py)

    poses = {p.reference: p for p in board.placements}
    seeds: dict[str, list[Point]] = {rail: [] for rail in rails}
    for net in board.nets:
        if net.name not in seeds:
            continue
        for pad in net.pads:
            pose = poses[pad.component]
            for land in board.footprints[pose.footprint].pads:
                if land.number == pad.pad:
                    seeds[net.name].append(transformed_local_point(pose, land.position))
    for via in board.vias:
        if via.net in seeds:
            seeds[via.net].append(via.position)
    best = np.full(px.shape, np.inf)
    owner = np.full(px.shape, -1, dtype=np.int32)
    used, skipped = [], []
    for index, rail in enumerate(rails):
        points = seeds.get(rail) or []
        if not points:
            skipped.append(rail)
            continue
        used.append(rail)
        # Skeleton: every pad, plus a corridor along each short spanning-tree
        # edge so a rail's nearby pads share one island. Corridor distance is
        # scaled so a corridor of width ``corridor`` competes like a pad disc.
        scale = reach_nm / max(1, corridor_nm / 2)
        shapes = [(p, p, 1.0) for p in points] + [(a, b, scale) for a, b in _spanning_edges(points, link_nm)]
        for a, b, factor in shapes:
            ax, ay, bx, by = a.x_nm, a.y_nm, b.x_nm, b.y_nm
            i0 = max(0, int((min(ax, bx) - reach_nm - x0) // pitch_nm)); i1 = min(nx - 1, int((max(ax, bx) + reach_nm - x0) // pitch_nm) + 1)
            j0 = max(0, int((min(ay, by) - reach_nm - y0) // pitch_nm)); j1 = min(ny - 1, int((max(ay, by) + reach_nm - y0) // pitch_nm) + 1)
            if i0 > i1 or j0 > j1:
                continue
            d = factor * _segment_distance(px[j0:j1 + 1, i0:i1 + 1], py[j0:j1 + 1, i0:i1 + 1],
                                           float(ax), float(ay), float(bx), float(by))
            win_best = best[j0:j1 + 1, i0:i1 + 1]
            take = (d < reach_nm) & (d < win_best)
            win_best[take] = d[take]
            owner[j0:j1 + 1, i0:i1 + 1][take] = index
    owner[~allowed] = -1
    # Keep a gap: a cell survives only if every neighbour within gap/2 has the same owner.
    k = max(1, int(np.ceil(gap_nm / 2 / pitch_nm)))
    kept = owner.copy()
    for dy in range(-k, k + 1):
        for dx in range(-k, k + 1):
            if dx * dx + dy * dy > k * k:
                continue
            shifted = np.full(owner.shape, -2, dtype=np.int32)
            ys_src = slice(max(0, -dy), owner.shape[0] - max(0, dy))
            ys_dst = slice(max(0, dy), owner.shape[0] - max(0, -dy))
            xs_src = slice(max(0, -dx), owner.shape[1] - max(0, dx))
            xs_dst = slice(max(0, dx), owner.shape[1] - max(0, -dx))
            shifted[ys_dst, xs_dst] = owner[ys_src, xs_src]
            kept[(shifted != owner)] = -1
    priority = max(zone.priority for zone in host) + 1
    zones = []
    from .plane_estimate import _label
    for index, rail in enumerate(rails):
        if rail not in used:
            continue
        mask = kept == index
        if not mask.any():
            continue
        labels, _ = _label(mask)
        seed_cells = set()
        for point in seeds[rail]:
            i = int((point.x_nm - x0) // pitch_nm); j = int((point.y_nm - y0) // pitch_nm)
            if 0 <= i < nx and 0 <= j < ny and labels[j, i] > 0:
                seed_cells.add(int(labels[j, i]))
        for count, label in enumerate(sorted(seed_cells)):
            outline = _outline(labels == label, x0, y0, pitch_nm)
            if outline is None or len(outline.outer.vertices) < 3:
                continue
            zones.append(CopperZone(
                f"auto-power-{rail}-{layer.value}-{count}", rail, (layer,),
                outline, priority=priority,
                pad_connection=ZoneConnection.SOLID, island_policy=IslandPolicy.REMOVE_ALL))
    return PowerPartition(layer, tuple(zones), tuple(used), tuple(skipped))


def _spanning_edges(points: list[Point], limit: int) -> list[tuple[Point, Point]]:
    """Prim's minimum spanning tree over distinct points; edges up to ``limit``."""
    unique = sorted({(p.x_nm, p.y_nm) for p in points})
    if len(unique) < 2:
        return []
    inside = {0}
    best = {i: (abs(complex(unique[i][0] - unique[0][0], unique[i][1] - unique[0][1])), 0)
            for i in range(1, len(unique))}
    edges = []
    while best:
        i = min(best, key=lambda k: (best[k][0], k))
        length, j = best.pop(i)
        inside.add(i)
        if length <= limit:
            edges.append((Point(*unique[i]), Point(*unique[j])))
        for k in best:
            d = abs(complex(unique[k][0] - unique[i][0], unique[k][1] - unique[i][1]))
            if d < best[k][0]:
                best[k] = (d, i)
    return edges


def _outline(mask: np.ndarray, x0: int, y0: int, pitch: int) -> PolygonWithHoles | None:
    """Outer boundary of one 4-connected raster region as a simplified ring.

    Interior holes are kept as polygon holes.
    """
    ny, nx = mask.shape
    padded = np.zeros((ny + 2, nx + 2), dtype=bool)
    padded[1:-1, 1:-1] = mask
    # Directed boundary edges with the region on the left (counter-clockwise in
    # math axes; KiCad y points down, orientation is irrelevant for a zone).
    edges: dict[tuple[int, int], list[tuple[int, int]]] = {}
    ys, xs = np.nonzero(padded[1:-1, 1:-1])
    for y, x in zip(ys.tolist(), xs.tolist()):
        if not padded[y, x + 1]:      # up neighbour (row y-1 in padded coordinates y)
            edges.setdefault((x, y), []).append((x + 1, y))
        if not padded[y + 2, x + 1]:  # down
            edges.setdefault((x + 1, y + 1), []).append((x, y + 1))
        if not padded[y + 1, x]:      # left
            edges.setdefault((x, y + 1), []).append((x, y))
        if not padded[y + 1, x + 2]:  # right
            edges.setdefault((x + 1, y), []).append((x + 1, y + 1))
    loops = []
    while edges:
        start = next(iter(edges))
        loop = [start]
        current = start
        previous = None
        while True:
            targets = edges.get(current)
            if not targets:
                break
            if len(targets) > 1 and previous is not None:
                # Pinch vertex: turn towards the region so diagonal neighbours
                # stay separate loops (the labels are 4-connected).
                dx, dy = current[0] - previous[0], current[1] - previous[1]
                order = [(current[0] - dy, current[1] + dx), (current[0] + dx, current[1] + dy),
                         (current[0] + dy, current[1] - dx)]
                nxt = next(t for t in order if t in targets)
                targets.remove(nxt)
            else:
                nxt = targets.pop()
            if not targets:
                del edges[current]
            previous = current
            if nxt == start:
                break
            loop.append(nxt)
            current = nxt
        if len(loop) >= 4:
            loops.append(loop)
    if not loops:
        return None
    def area(loop):
        return sum(a[0] * b[1] - b[0] * a[1] for a, b in zip(loop, loop[1:] + loop[:1])) / 2
    def ring(loop):
        n = len(loop)
        corners = [loop[i] for i in range(n)
                   if (loop[i][0] - loop[i - 1][0]) * (loop[(i + 1) % n][1] - loop[i][1])
                   - (loop[i][1] - loop[i - 1][1]) * (loop[(i + 1) % n][0] - loop[i][0]) != 0]
        return PolygonRing(tuple(Point(x0 + x * pitch, y0 + y * pitch) for x, y in corners))
    loops.sort(key=lambda loop: abs(area(loop)), reverse=True)
    # The largest loop is the outline; loops of opposite orientation bound
    # holes, which stay open so islands nested inside do not overlap.
    sign = area(loops[0]) > 0
    return PolygonWithHoles(ring(loops[0]), tuple(ring(loop) for loop in loops[1:] if (area(loop) > 0) != sign))
