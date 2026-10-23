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
    split: tuple[str, ...] = ()     # rails left without islands: one island could not connect them

    def report(self) -> dict:
        return {"layer": self.layer.value, "rails": list(self.rails), "islands": len(self.zones),
                "skipped": list(self.skipped), "split": list(self.split)}


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


def plan_ball_grid_islands(board: PhysicalBoard, layer: CopperLayer, rails: tuple[str, ...] | None = None, *,
                           gap_nm: int = nm_from_mm("0.3"), pitch_nm: int = nm_from_mm("0.05"),
                           extend_nm: int = nm_from_mm("12"), corridor_nm: int = nm_from_mm("0.8"),
                           pad_reach_nm: int = nm_from_mm("0.6"), core_nm: int = nm_from_mm("0.5")) -> PowerPartition:
    """Split-plane islands under ball grids from the via-site matrix, extended to each rail's other pads.

    Under the array: each cell of a ball grid's via lattice (the square
    around a cell centre, one pitch wide; the ring just outside the array
    included) is seeded by the rail whose via sits on that centre, or, for a
    rail ball without a via yet, by its outward cell. Every other cell goes
    to the nearest seed, so the field is divided among the rails and other
    nets' vias are holes in the fill (the FX10 kit's PWR1 layer).

    Outside it: every other pad of a rail within ``extend_nm`` (its ferrite
    bead, bulk and decoupling capacitors) is joined by a ``corridor_nm`` wide
    corridor, routed on the raster from the pad to the nearest copper of the
    rail through the host plane's area, keeping ``core_nm`` from other rails'
    pads and the gap from their copper; nearest pads first, so later ones can
    join earlier corridors. The kit's islands likewise reach those parts.

    Touching same-rail copper forms one island; a rail may get several, which
    the router joins as plane pieces. Islands keep ``gap_nm`` from each other
    and from the host plane, which they are carved out of by priority. Rails
    whose routing rule excludes ``layer`` get none.
    """
    from collections import deque
    from .package_patterns import ball_grid
    from .placement import transformed_local_point
    from .plane_estimate import _label
    from .unified_router import _polygon_inside, _segment_distance

    rails = tuple(rails) if rails is not None else ball_grid_rails(board)
    rules = {rule.net: rule for rule in board.net_routing_rules}
    excluded = tuple(rail for rail in rails if rules.get(rail) is not None and rules[rail].allowed_layers
                     and layer not in rules[rail].allowed_layers)
    rails = tuple(rail for rail in rails if rail not in excluded)
    host = [zone for zone in board.zones if layer in zone.layers]
    if not rails or not host:
        return PowerPartition(layer, (), (), excluded)
    rail_index = {rail: index for index, rail in enumerate(rails)}
    net_of = {(pad.component, pad.pad): net.name for net in board.nets for pad in net.pads}
    poses = {pose.reference: pose for pose in board.placements}

    # Board raster: the cells a rail may fill (the host plane's area).
    vertices = board.outline.vertices
    x0 = min(p.x_nm for p in vertices); y0 = min(p.y_nm for p in vertices)
    x1 = max(p.x_nm for p in vertices); y1 = max(p.y_nm for p in vertices)
    rx, ry = int((x1 - x0) // pitch_nm) + 1, int((y1 - y0) // pitch_nm) + 1
    px, py = np.meshgrid(x0 + pitch_nm * (np.arange(rx) + 0.5), y0 + pitch_nm * (np.arange(ry) + 0.5))
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
    owner = np.full(px.shape, -1, dtype=np.int32)

    def window(cx, cy, half):
        return (max(0, int((cy - half - y0) // pitch_nm)), min(ry - 1, int((cy + half - y0) // pitch_nm)),
                max(0, int((cx - half - x0) // pitch_nm)), min(rx - 1, int((cx + half - x0) // pitch_nm)))

    def paint(cx, cy, half, value):
        j0, j1, i0, i1 = window(cx, cy, half)
        owner[j0:j1 + 1, i0:i1 + 1] = value

    # Under each ball grid: the via-site matrix.
    for pose in sorted(board.placements, key=lambda p: p.reference):
        grid = ball_grid(board, pose.reference)
        if grid is None:
            continue
        nx, ny, half = len(grid.xs_nm), len(grid.ys_nm), grid.pitch_nm // 2

        def centre(cx, cy):
            return transformed_local_point(pose, Point(grid.xs_nm[0] + cx * grid.pitch_nm + half,
                                                       grid.ys_nm[0] + cy * grid.pitch_nm + half))
        cells: dict[tuple[int, int], int] = {}
        sites = {(cx, cy): centre(cx, cy) for cx in range(-1, nx) for cy in range(-1, ny)}
        tolerance = grid.tolerance_nm + 1
        for via in board.vias:
            if via.net not in rail_index:
                continue
            for key, point in sites.items():
                if abs(via.position.x_nm - point.x_nm) <= tolerance and abs(via.position.y_nm - point.y_nm) <= tolerance:
                    cells.setdefault(key, rail_index[via.net])
        for land in sorted((p for p in board.footprints[pose.footprint].pads if p.number in grid.balls),
                           key=lambda p: p.number):
            net = net_of.get((pose.reference, land.number))
            if net not in rail_index:
                continue
            ix, iy = grid.index(land.position)
            if any(cells.get((ix - dx, iy - dy)) == rail_index[net] for dx in (0, 1) for dy in (0, 1)):
                continue  # its via already marks a cell of the rail
            sx = -1 if 2 * ix < nx - 1 else 1
            sy = -1 if 2 * iy < ny - 1 else 1
            cells.setdefault((ix - (sx < 0), iy - (sy < 0)), rail_index[net])
        if not cells:
            continue
        seeds = sorted(cells.items())
        for key in sites:
            if key not in cells:
                cells[key] = min(seeds, key=lambda item: ((item[0][0] - key[0]) ** 2 + (item[0][1] - key[1]) ** 2,
                                                          item[1], item[0]))[1]
        for key, value in cells.items():
            paint(sites[key].x_nm, sites[key].y_nm, half, value)
        for (cx, cy), value in cells.items():
            for dx, dy in ((1, 1), (1, -1)):
                if cells.get((cx + dx, cy + dy)) == value:
                    a, b = sites[(cx, cy)], sites[(cx + dx, cy + dy)]
                    paint((a.x_nm + b.x_nm) // 2, (a.y_nm + b.y_nm) // 2, half // 2, value)
    owner[~allowed] = -1

    # Outside the arrays: corridors from each rail's other pads to its copper.
    pads: dict[int, list[Point]] = {index: [] for index in rail_index.values()}
    for net in board.nets:
        if net.name not in rail_index:
            continue
        for pad in net.pads:
            pose = poses[pad.component]
            land = next((item for item in board.footprints[pose.footprint].pads if item.number == pad.pad), None)
            if land is not None:
                pads[rail_index[net.name]].append(transformed_local_point(pose, land.position))

    def cell(point):
        return (min(ry - 1, max(0, int((point.y_nm - y0) // pitch_nm))),
                min(rx - 1, max(0, int((point.x_nm - x0) // pitch_nm))))

    def disc(point, radius):
        j0, j1, i0, i1 = window(point.x_nm, point.y_nm, radius)
        inside = np.hypot(px[j0:j1 + 1, i0:i1 + 1] - point.x_nm, py[j0:j1 + 1, i0:i1 + 1] - point.y_nm) <= radius
        return (slice(j0, j1 + 1), slice(i0, i1 + 1)), inside

    cores = np.full(px.shape, -1, dtype=np.int32)
    for index, points in pads.items():
        for point in points:
            where, inside = disc(point, core_nm)
            region = cores[where]
            region[inside & (region != -1) & (region != index)] = -2
            region[inside & (region == -1)] = index
    kg = max(1, int(np.ceil(gap_nm / pitch_nm)))
    kw = max(1, int(corridor_nm // 2 // pitch_nm))
    band = [(dy, dx) for dy in range(-kw, kw + 1) for dx in range(-kw, kw + 1) if dx * dx + dy * dy <= kw * kw]
    reach = int(extend_nm // pitch_nm)
    for index in sorted(pads):
        mine = owner == index
        if not mine.any():
            continue
        others = (owner >= 0) & ~mine
        grown = others.copy()  # other rails' copper plus the gap
        for dy in range(-kg, kg + 1):
            for dx in range(-kg, kg + 1):
                if dx * dx + dy * dy <= kg * kg:
                    grown |= np.roll(np.roll(others, dy, axis=0), dx, axis=1)
        blocked = ~allowed | grown | ((cores != -1) & (cores != index))
        js, columns = np.nonzero(mine)

        def distance(point):
            j, i = cell(point)
            return int(np.min(np.abs(js - j) + np.abs(columns - i)))
        for point in sorted(pads[index], key=lambda p: (distance(p), p.x_nm, p.y_nm)):
            start = cell(point)
            if owner[start] == index or distance(point) > reach:
                continue
            j0, j1 = max(0, start[0] - reach), min(ry - 1, start[0] + reach)
            i0, i1 = max(0, start[1] - reach), min(rx - 1, start[1] + reach)
            previous = {start: None}
            queue = deque([start])
            found = None
            while queue:
                j, i = queue.popleft()
                if owner[j, i] == index:
                    found = (j, i)
                    break
                for dj, di in ((0, 1), (1, 0), (0, -1), (-1, 0)):
                    nj, ni = j + dj, i + di
                    if j0 <= nj <= j1 and i0 <= ni <= i1 and (nj, ni) not in previous and (
                            owner[nj, ni] == index or not blocked[nj, ni]):
                        previous[(nj, ni)] = (j, i)
                        queue.append((nj, ni))
            if found is None:
                continue  # walled in by other rails: the router joins this pad with a track
            step = found
            while step is not None:
                j, i = step
                for dy, dx in band:
                    nj, ni = j + dy, i + dx
                    if 0 <= nj < ry and 0 <= ni < rx and not blocked[nj, ni] and owner[nj, ni] == -1:
                        owner[nj, ni] = index
                step = previous[step]
            where, inside = disc(point, pad_reach_nm)
            region = owner[where]
            region[inside & (region == -1) & ~blocked[where]] = index

    # Gaps between different owners (and the host plane), then islands.
    k = max(1, int(np.ceil(gap_nm / 2 / pitch_nm)))
    kept = owner.copy()
    for dy in range(-k, k + 1):
        for dx in range(-k, k + 1):
            if dx * dx + dy * dy > k * k:
                continue
            shifted = np.full(owner.shape, -2, dtype=np.int32)
            source = owner[max(0, -dy):ry - max(0, dy), max(0, -dx):rx - max(0, dx)]
            shifted[max(0, dy):ry - max(0, -dy), max(0, dx):rx - max(0, -dx)] = source
            kept[shifted != owner] = -1
    priority = max(zone.priority for zone in host) + 1
    zones, used = [], set()
    for index, rail in enumerate(rails):
        mask = kept == index
        if not mask.any():
            continue
        labels, _ = _label(mask)
        for count, label in enumerate(sorted(set(np.unique(labels[labels > 0]).tolist()))):
            outline = _outline(_slit_holes(labels == label), x0, y0, pitch_nm)
            if outline is None or len(outline.outer.vertices) < 3:
                continue
            used.add(rail)
            zones.append(CopperZone(f"bga-power-{rail}-{layer.value}-{count}", rail, (layer,), outline,
                                    priority=priority, pad_connection=ZoneConnection.SOLID,
                                    island_policy=IslandPolicy.REMOVE_ALL))
    return PowerPartition(layer, tuple(zones), tuple(r for r in rails if r in used),
                          tuple(r for r in rails if r not in used) + excluded)


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
                       single_island: bool = True, **options) -> PowerPartition:
    """Split-plane islands for ``rails`` on ``layer`` (see ``_partition``).

    With ``single_island`` a rail that would get more than one island keeps
    none and is planned again without it: as a plane net its pads could then
    only tap pieces that do not connect them, while as an ordinary net the
    router still joins them with tracks. Interleaved rails under a ball grid
    cannot all be connected on one layer; the kit layout uses several layers.
    """
    rails = tuple(rails) if rails is not None else decoupled_rails(board)
    split: list[str] = []
    while True:
        partition = _partition(board, layer, tuple(r for r in rails if r not in split), **options)
        counts: dict[str, int] = {}
        for zone in partition.zones:
            counts[zone.net] = counts.get(zone.net, 0) + 1
        more = [rail for rail, count in counts.items() if count > 1]
        if not single_island or not more:
            break
        split += sorted(more)
    return PowerPartition(partition.layer, partition.zones, partition.rails,
                          partition.skipped + tuple(split), tuple(split))


def _partition(board: PhysicalBoard, layer: CopperLayer, rails: tuple[str, ...] | None = None, *,
                       reach_nm: int = nm_from_mm("1.6"), gap_nm: int = nm_from_mm("0.3"),
                       corridor_nm: int = nm_from_mm("1.6"), link_nm: int = nm_from_mm("12"),
                       pitch_nm: int = nm_from_mm("0.1"), connected: bool = True,
                       link_width_nm: int = nm_from_mm("0.8"), core_nm: int = nm_from_mm("0.5")) -> PowerPartition:
    """Islands for ``rails`` on ``layer``; ``connected`` joins each rail's pads first.

    With ``connected`` every spanning-tree edge of a rail's pads and vias
    (up to ``link_nm``) is routed on the raster as a ``link_width_nm`` wide
    corridor that keeps ``core_nm`` away from other rails' pads and earlier
    rails' corridors, and claimed before the nearest-pad split. Without it a
    rail whose pads are interleaved with other rails' (a ball grid's supply
    balls) falls apart into pieces that do not connect its pads; reference
    split planes (the FX10 kit's PWR1) give each rail one connected island.
    A rail whose routing rule excludes ``layer`` gets no island.
    """
    from .placement import transformed_local_point
    from .unified_router import _polygon_inside, _segment_distance

    rails = tuple(rails) if rails is not None else decoupled_rails(board)
    rules = {rule.net: rule for rule in board.net_routing_rules}
    excluded = tuple(rail for rail in rails if rules.get(rail) is not None and rules[rail].allowed_layers
                     and layer not in rules[rail].allowed_layers)
    rails = tuple(rail for rail in rails if rail not in excluded)
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
    if connected:
        claimed = _link_corridors(seeds, used, rails, allowed, x0, y0, pitch_nm, link_nm, link_width_nm, core_nm)
        owner = np.where(claimed >= 0, claimed, owner)
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
            # Zone outlines without holes (the KiCad backend writes none): a
            # slit from each hole to the outside keeps the island one piece.
            outline = _outline(_slit_holes(labels == label), x0, y0, pitch_nm)
            if outline is None or len(outline.outer.vertices) < 3:
                continue
            zones.append(CopperZone(
                f"auto-power-{rail}-{layer.value}-{count}", rail, (layer,),
                outline, priority=priority,
                pad_connection=ZoneConnection.SOLID, island_policy=IslandPolicy.REMOVE_ALL))
    return PowerPartition(layer, tuple(zones), tuple(used), tuple(skipped) + excluded)


def _link_corridors(seeds, used, rails, allowed, x0, y0, pitch, link, width, core) -> np.ndarray:
    """Raster owner (rail index or -1) of the corridors joining each rail's seeds."""
    from collections import deque
    ny, nx = allowed.shape
    cell = lambda point: (min(ny - 1, max(0, int((point.y_nm - y0) // pitch))),
                          min(nx - 1, max(0, int((point.x_nm - x0) // pitch))))
    cores = np.full(allowed.shape, -1, dtype=np.int32)
    kc = max(1, int(core // pitch))
    disc = [(dy, dx) for dy in range(-kc, kc + 1) for dx in range(-kc, kc + 1) if dx * dx + dy * dy <= kc * kc]
    for index, rail in enumerate(rails):
        if rail not in used:
            continue
        for point in seeds[rail]:
            j, i = cell(point)
            for dy, dx in disc:
                if 0 <= j + dy < ny and 0 <= i + dx < nx:
                    cores[j + dy, i + dx] = index if cores[j + dy, i + dx] in (-1, index) else -2
    claimed = np.full(allowed.shape, -1, dtype=np.int32)
    kw = max(1, int(width // 2 // pitch))
    band = [(dy, dx) for dy in range(-kw, kw + 1) for dx in range(-kw, kw + 1) if dx * dx + dy * dy <= kw * kw]
    margin = int(nm_from_mm("3") // pitch)
    for index, rail in enumerate(rails):
        if rail not in used:
            continue
        for a, b in _spanning_edges(seeds[rail], link):
            (ja, ia), (jb, ib) = cell(a), cell(b)
            j0, j1 = max(0, min(ja, jb) - margin), min(ny - 1, max(ja, jb) + margin)
            i0, i1 = max(0, min(ia, ib) - margin), min(nx - 1, max(ia, ib) + margin)
            def free(j, i):
                return (allowed[j, i] and cores[j, i] in (-1, index) and claimed[j, i] in (-1, index))
            previous = {(ja, ia): None}
            queue = deque([(ja, ia)])
            while queue and (jb, ib) not in previous:
                j, i = queue.popleft()
                for dy, dx in ((0, 1), (1, 0), (0, -1), (-1, 0), (1, 1), (1, -1), (-1, 1), (-1, -1)):
                    nj, ni = j + dy, i + dx
                    if j0 <= nj <= j1 and i0 <= ni <= i1 and (nj, ni) not in previous and (
                            free(nj, ni) or (nj, ni) == (jb, ib)):
                        previous[(nj, ni)] = (j, i)
                        queue.append((nj, ni))
            if (jb, ib) not in previous:
                continue  # no corridor around the other rails here; the pads stay apart
            step = (jb, ib)
            while step is not None:
                j, i = step
                for dy, dx in band:
                    if 0 <= j + dy < ny and 0 <= i + dx < nx and free(j + dy, i + dx):
                        claimed[j + dy, i + dx] = index
                step = previous[step]
    return claimed


def _slit_holes(mask: np.ndarray) -> np.ndarray:
    """``mask`` with a one-cell slit from each enclosed hole up to the outside."""
    from .plane_estimate import _label
    mask = mask.copy()
    while True:
        labels, _ = _label(~mask)
        border = set(np.unique(np.concatenate((labels[0], labels[-1], labels[:, 0], labels[:, -1]))).tolist())
        holes = sorted(set(np.unique(labels[labels > 0]).tolist()) - border)
        if not holes:
            return mask
        ys, xs = np.nonzero(labels == holes[0])
        j, i = int(ys.min()), int(xs[ys.argmin()])
        j -= 1
        while j >= 0 and mask[j, i]:
            mask[j, i] = False
            j -= 1


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
