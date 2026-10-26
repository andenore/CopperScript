"""Fast raster estimate of zone fill and plane-net connectivity.

KiCad's native refill stays the authority for sign-off. This estimate runs
anywhere, in about a second, so the benchmark ladder and planes-first power
planning can see plane-net opens without KiCad. It fills each zone layer on a
raster: zone outline, inside board material with edge clearance, minus foreign
copper inflated by clearance, minus higher-priority foreign zones and
zone-blocking keepouts. Connected fill regions then join the copper of their
net (pads, tracks, vias) that touches them.

The result is an estimate. Thin necks below the raster pitch can split or
join regions, and thermal reliefs count as connected.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

from .physical import CopperLayer, PadKind, PhysicalBoard, ZoneConnection, nm_from_mm


@dataclass(frozen=True, slots=True)
class PlaneEstimate:
    pitch_nm: int
    net_groups: dict          # net -> number of disconnected groups of its pads
    opens: int                # sum over zone nets of (groups - 1)
    unfilled_zones: tuple[str, ...]
    # net -> pads outside the net's largest connected group ("REF.PAD"), i.e. where to look
    open_pads: dict = field(default_factory=dict)

    def report(self) -> dict:
        return {"pitch_mm": self.pitch_nm / 1_000_000, "opens": self.opens,
                "open_nets": sorted(net for net, groups in self.net_groups.items() if groups > 1),
                "open_pads": {net: list(pads) for net, pads in sorted(self.open_pads.items())},
                "unfilled_zones": list(self.unfilled_zones)}


def _label(mask: np.ndarray) -> tuple[np.ndarray, int]:
    """4-connected component labels via row runs and union-find."""
    labels = np.zeros(mask.shape, dtype=np.int32)
    parent: list[int] = [0]

    def find(a: int) -> int:
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a

    previous: list[tuple[int, int, int]] = []
    for y in range(mask.shape[0]):
        row = mask[y]
        if not row.any():
            previous = []
            continue
        padded = np.concatenate(([False], row, [False]))
        edges = np.flatnonzero(padded[1:] != padded[:-1])
        runs = []
        j = 0
        for start, end in zip(edges[0::2], edges[1::2]):
            label = len(parent)
            parent.append(label)
            while j < len(previous) and previous[j][1] <= start:
                j += 1
            k = j
            while k < len(previous) and previous[k][0] < end:
                a, b = find(label), find(previous[k][2])
                if a != b:
                    parent[max(a, b)] = min(a, b)
                k += 1
            labels[y, start:end] = label
            runs.append((start, end, label))
        previous = runs
    roots = np.array([find(i) for i in range(len(parent))], dtype=np.int32)
    return roots[labels], len(parent)


def _distance_to_spine(spine, px, py):
    from .unified_router import _spine_distance
    return _spine_distance(spine, px, py)


def estimate_plane_connectivity(board: PhysicalBoard, *, pitch_nm: int = nm_from_mm("0.05")) -> PlaneEstimate:
    from .drc import explicit_copper_connectivity, non_plated_holes, placed_pad_shape
    from .placement import transformed_local_point
    from .unified_router import _polygon_inside, _segment_distance

    zones = tuple(board.zones)
    zone_nets = {zone.net for zone in zones}
    if not zones:
        return PlaneEstimate(pitch_nm, {}, 0, ())
    vertices = board.outline.vertices
    x0 = min(p.x_nm for p in vertices); y0 = min(p.y_nm for p in vertices)
    x1 = max(p.x_nm for p in vertices); y1 = max(p.y_nm for p in vertices)
    nx = int((x1 - x0) // pitch_nm) + 1
    ny = int((y1 - y0) // pitch_nm) + 1
    xs = x0 + pitch_nm * (np.arange(nx) + 0.5)
    ys = y0 + pitch_nm * (np.arange(ny) + 0.5)
    px, py = np.meshgrid(xs, ys)
    material = _polygon_inside(vertices, px, py)
    for cutout in board.outline.cutouts:
        material &= ~_polygon_inside(cutout.vertices, px, py)
    minimum_clearance = board.rules.minimum_clearance_nm
    half = pitch_nm / 2
    # Cells at least ``minimum_clearance + half`` from every outline edge. A
    # cell nearer than that to an edge lies in the edge's box grown by that
    # distance, so each edge is tested in its own window only.
    edge_reach = minimum_clearance + half
    edge_ok = np.ones(px.shape, dtype=bool)
    for ring in (vertices, *(c.vertices for c in board.outline.cutouts)):
        for i in range(len(ring)):
            a, b = ring[i], ring[(i + 1) % len(ring)]
            i0 = max(0, int((min(a.x_nm, b.x_nm) - edge_reach - x0) // pitch_nm))
            i1 = min(nx - 1, int((max(a.x_nm, b.x_nm) + edge_reach - x0) // pitch_nm) + 1)
            j0 = max(0, int((min(a.y_nm, b.y_nm) - edge_reach - y0) // pitch_nm))
            j1 = min(ny - 1, int((max(a.y_nm, b.y_nm) + edge_reach - y0) // pitch_nm) + 1)
            if i0 > i1 or j0 > j1:
                continue
            edge_ok[j0:j1 + 1, i0:i1 + 1] &= _segment_distance(
                px[j0:j1 + 1, i0:i1 + 1], py[j0:j1 + 1, i0:i1 + 1],
                float(a.x_nm), float(a.y_nm), float(b.x_nm), float(b.y_nm)) >= edge_reach

    # Copper objects: (net, layers, spine, radius, kind, pad_kind)
    objects = []
    for placement in board.placements:
        footprint = board.footprints[placement.footprint]
        for pad in footprint.pads:
            if pad.kind is PadKind.APERTURE:
                continue
            position = transformed_local_point(placement, pad.position)
            shape = placed_pad_shape(position, pad, placement)
            if pad.kind is PadKind.SMD:
                layers = (CopperLayer.FRONT if placement.side.value == "front" else CopperLayer.BACK,)
            else:
                layers = tuple(board.stackup.copper_layers)
            objects.append((("pad", placement.reference, pad.number), layers, shape.spine,
                            shape.radius_nm, pad.kind))
    for index, track in enumerate(board.tracks):
        objects.append((("track", index), (track.layer,), (track.start, track.end), track.width_nm // 2, None))
    layer_order = list(board.stackup.copper_layers)
    for index, via in enumerate(board.vias):
        a, b = layer_order.index(via.from_layer), layer_order.index(via.to_layer)
        layers = tuple(layer_order[min(a, b):max(a, b) + 1])
        objects.append((("via", index), layers, (via.position,), via.size_nm // 2, None))
    net_of_pad = {(pad.component, pad.pad): net.name for net in board.nets for pad in net.pads}

    def net_of(identity):
        kind = identity[0]
        if kind == "pad":
            return net_of_pad.get((identity[1], identity[2]))
        if kind == "track":
            return board.tracks[identity[1]].net
        return board.vias[identity[1]].net

    graph = explicit_copper_connectivity(board, only_nets=frozenset(zone_nets))
    parent: dict[str, str] = {}

    def find(a: str) -> str:
        parent.setdefault(a, a)
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a

    def union(a: str, b: str) -> None:
        a, b = find(a), find(b)
        if a != b:
            parent[max(a, b)] = min(a, b)

    def node_name(identity):
        if identity[0] == "pad":
            return None  # pads map through the explicit graph below
        return f"{identity[0]}:{identity[1]}"

    for key, root in graph.roots.items():
        union(key, f"root:{root}")

    def window(spine, reach):
        xs_ = [p.x_nm for p in spine]; ys_ = [p.y_nm for p in spine]
        i0 = max(0, int((min(xs_) - reach - x0) // pitch_nm)); i1 = min(nx - 1, int((max(xs_) + reach - x0) // pitch_nm) + 1)
        j0 = max(0, int((min(ys_) - reach - y0) // pitch_nm)); j1 = min(ny - 1, int((max(ys_) + reach - y0) // pitch_nm) + 1)
        return i0, i1, j0, j1

    unfilled = []
    for zone in sorted(zones, key=lambda z: (-z.priority, z.id)):
        clearance = max(zone.clearance_nm or 0, minimum_clearance)
        outer = _polygon_inside(zone.outline.outer.vertices, px, py)
        for hole in zone.outline.holes:
            outer &= ~_polygon_inside(hole.vertices, px, py)
        for layer in zone.layers:
            fill = outer & material & edge_ok
            for identity, layers, spine, radius, pad_kind in objects:
                if layer not in layers:
                    continue
                if net_of(identity) == zone.net:
                    continue
                reach = radius + clearance + half
                i0, i1, j0, j1 = window(spine, reach)
                if i0 > i1 or j0 > j1:
                    continue
                sub = _distance_to_spine(spine, px[j0:j1 + 1, i0:i1 + 1], py[j0:j1 + 1, i0:i1 + 1]) < reach
                fill[j0:j1 + 1, i0:i1 + 1] &= ~sub
            for _, hole in non_plated_holes(board):
                reach = hole.radius_nm + board.rules.minimum_hole_clearance_nm + half
                i0, i1, j0, j1 = window(hole.spine, reach)
                if i0 <= i1 and j0 <= j1:
                    fill[j0:j1 + 1, i0:i1 + 1] &= ~(_distance_to_spine(
                        hole.spine, px[j0:j1 + 1, i0:i1 + 1], py[j0:j1 + 1, i0:i1 + 1]) < reach)
            for other in zones:
                if other.net != zone.net and other.priority > zone.priority and layer in other.layers:
                    region = _polygon_inside(other.outline.outer.vertices, px, py)
                    fill &= ~region
            for keepout in board.copper_keepouts:
                if keepout.block_zones and layer in keepout.layers:
                    fill &= ~_polygon_inside(keepout.outline.outer.vertices, px, py)
            if not fill.any():
                unfilled.append(f"{zone.id}@{layer.value}")
                continue
            labels, _ = _label(fill)
            for identity, layers, spine, radius, pad_kind in objects:
                if layer not in layers or net_of(identity) != zone.net:
                    continue
                if identity[0] == "pad" and zone.pad_connection is ZoneConnection.NONE:
                    continue
                reach = radius + half
                i0, i1, j0, j1 = window(spine, reach)
                if i0 > i1 or j0 > j1:
                    continue
                touch = _distance_to_spine(spine, px[j0:j1 + 1, i0:i1 + 1], py[j0:j1 + 1, i0:i1 + 1]) <= reach
                hit = np.unique(labels[j0:j1 + 1, i0:i1 + 1][touch & (labels[j0:j1 + 1, i0:i1 + 1] > 0)])
                if identity[0] == "pad":
                    nodes = [key for key in graph.pad_nodes.get(_pad_ref(identity), ())]
                    names = [f"root:{graph.roots[key]}" for key in nodes] or [f"pad:{identity[1]}.{identity[2]}"]
                else:
                    names = [node_name(identity)]
                for label in hit:
                    region = f"fill:{zone.id}:{layer.value}:{int(label)}"
                    for name in names:
                        union(name, region)
    groups = {}
    open_pads = {}
    for net in board.nets:
        if net.name not in zone_nets or len(net.pads) < 2:
            continue
        members: dict[str, list[str]] = {}
        for pad in net.pads:
            nodes = graph.pad_nodes.get(pad, ())
            names = [f"root:{graph.roots[key]}" for key in nodes] or [f"pad:{pad.component}.{pad.pad}"]
            members.setdefault(find(names[0]), []).append(f"{pad.component}.{pad.pad}")
        groups[net.name] = len(members)
        if len(members) > 1:
            largest = max(members.values(), key=lambda pads: (len(pads), sorted(pads)))
            open_pads[net.name] = tuple(sorted(p for pads in members.values() if pads is not largest
                                               for p in pads))
    opens = sum(count - 1 for count in groups.values())
    return PlaneEstimate(pitch_nm, groups, opens, tuple(unfilled), open_pads)


def _pad_ref(identity):
    from .physical import PadReference
    return PadReference(identity[1], identity[2])
