"""Unified negotiated-congestion detailed router (routing refactor plan, P5).

Every routed net competes for one shared grid. Paths may overlap during
search at a price: a present-congestion cost that grows each iteration and a
history cost that accumulates where conflicts persist (PathFinder). Only
*fixed* copper is immovable: pads, existing (locked) tracks and vias,
keepouts and macro reservations.

Legality works in two layers:

* A numpy raster of node legality per width class, built once per board from
  the exact obstacle model in :class:`RoutingClearanceIndex`. It drives the
  search and is deliberately a little conservative (``raster_margin_nm``).
* An exact commit check with :class:`RoutingClearanceIndex` for every track
  and via that is written. A net that fails it is not written; its offending
  nodes are penalised and it is retried.

The result is a :class:`DetailedRoutingResult`, so the pipeline can use this
router in place of :func:`pcbir.detailed.route_detailed`.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from heapq import heappop, heappush
from math import ceil, floor, hypot, sqrt
from time import perf_counter
from types import MappingProxyType
from typing import Iterable, Mapping

import numpy as np

from .detailed import (
    DetailedNetResult,
    DetailedRoutingMetrics,
    DetailedRoutingResult,
    DetailedRoutingStatus,
    _fingerprint,
)
from .geometry import RoundedConvexShape
from .physical import (
    CopperLayer,
    NetRoutingRule,
    PadKind,
    PadReference,
    PhysicalBoard,
    Point,
    RouteKind,
    TrackSegment,
    Via,
    nm_from_mm,
)
from .progress import ProgressCallback, emit
from .routing import GlobalRoutingResult
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers, signal_layer_preferences

_FOREIGN = -2  # Obstacle owned by another net, by several nets or by none.
_FREE = -1
_SQRT2 = sqrt(2.0)
# 8 planar moves: (dx, dy, length in pitches).
_MOVES = ((1, 0, 1.0), (-1, 0, 1.0), (0, 1, 1.0), (0, -1, 1.0),
          (1, 1, _SQRT2), (1, -1, _SQRT2), (-1, 1, _SQRT2), (-1, -1, _SQRT2))


@dataclass(frozen=True, slots=True)
class UnifiedRouterOptions:
    pitch_nm: int = nm_from_mm("0.25")
    maximum_iterations: int = 40
    present_factor: float = 0.6
    present_growth: float = 1.5
    history_increment: float = 0.8
    via_cost: float = 6.0          # in pitch lengths
    wrong_way_cost: float = 0.5    # added per pitch against a layer's preferred heading
    outer_layer_cost: float = 0.0  # added per pitch on surface layers
    attach_radius_nm: int = nm_from_mm("1.5")
    # Near its own pads a net may narrow to the board minimum track width.
    neck_radius_nm: int = nm_from_mm("1.2")
    attach_candidates: int = 6
    window_margin_nm: int = nm_from_mm("2.5")
    maximum_expansions: int = 400_000
    commit_rounds: int = 4
    bga_fanout: bool = True
    raster_margin_nm: int | None = None
    time_budget_s: float | None = None

    def __post_init__(self) -> None:
        if self.pitch_nm <= 0 or self.maximum_iterations <= 0:
            raise ValueError("unified router pitch and iterations must be positive")


# ---------------------------------------------------------------------------
# Geometry helpers (numpy, nanometres as float64)


def _segment_distance(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    length2 = dx * dx + dy * dy
    if length2 == 0:
        return np.hypot(px - ax, py - ay)
    t = np.clip(((px - ax) * dx + (py - ay) * dy) / length2, 0.0, 1.0)
    return np.hypot(px - (ax + t * dx), py - (ay + t * dy))


def _spine_distance(spine: tuple[Point, ...], px, py):
    """Distance from points to a convex point/segment/polygon spine (0 inside)."""
    xs = [float(p.x_nm) for p in spine]
    ys = [float(p.y_nm) for p in spine]
    if len(spine) == 1:
        return np.hypot(px - xs[0], py - ys[0])
    if len(spine) == 2:
        return _segment_distance(px, py, xs[0], ys[0], xs[1], ys[1])
    best = None
    inside_pos = np.ones(np.broadcast(px, py).shape, dtype=bool)
    inside_neg = inside_pos.copy()
    for i in range(len(spine)):
        ax, ay, bx, by = xs[i], ys[i], xs[(i + 1) % len(xs)], ys[(i + 1) % len(ys)]
        d = _segment_distance(px, py, ax, ay, bx, by)
        best = d if best is None else np.minimum(best, d)
        cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax)
        inside_pos &= cross >= 0
        inside_neg &= cross <= 0
    return np.where(inside_pos | inside_neg, 0.0, best)


def _polygon_inside(vertices: tuple[Point, ...], px, py):
    """Even-odd point-in-polygon for arrays of points."""
    inside = np.zeros(np.broadcast(px, py).shape, dtype=bool)
    n = len(vertices)
    for i in range(n):
        a, b = vertices[i], vertices[(i + 1) % n]
        ax, ay, bx, by = float(a.x_nm), float(a.y_nm), float(b.x_nm), float(b.y_nm)
        crosses = (ay > py) != (by > py)
        with np.errstate(divide="ignore", invalid="ignore"):
            x_at = ax + (py - ay) * (bx - ax) / (by - ay)
        inside ^= crosses & (px < x_at)
    return inside


# ---------------------------------------------------------------------------
# Grid and rasters


class _Grid:
    def __init__(self, board: PhysicalBoard, pitch: int) -> None:
        vertices = board.outline.vertices
        min_x = min(p.x_nm for p in vertices)
        min_y = min(p.y_nm for p in vertices)
        max_x = max(p.x_nm for p in vertices)
        max_y = max(p.y_nm for p in vertices)
        self.pitch = pitch
        self.x0 = floor(min_x / pitch) * pitch
        self.y0 = floor(min_y / pitch) * pitch
        self.nx = int((max_x - self.x0) // pitch) + 2
        self.ny = int((max_y - self.y0) // pitch) + 2
        self.layers = tuple(board.stackup.copper_layers)
        self.nl = len(self.layers)
        self.plane = self.nx * self.ny
        self.size = self.plane * self.nl
        self.xs = self.x0 + pitch * np.arange(self.nx, dtype=np.float64)
        self.ys = self.y0 + pitch * np.arange(self.ny, dtype=np.float64)

    def index(self, layer: int, ix: int, iy: int) -> int:
        return layer * self.plane + iy * self.nx + ix

    def unpack(self, node: int) -> tuple[int, int, int]:
        layer, rest = divmod(node, self.plane)
        iy, ix = divmod(rest, self.nx)
        return layer, ix, iy

    def point(self, node: int) -> Point:
        _, ix, iy = self.unpack(node)
        return Point(self.x0 + ix * self.pitch, self.y0 + iy * self.pitch)

    def window(self, min_x: float, min_y: float, max_x: float, max_y: float):
        """Index slices covering a nanometre box (clipped to the grid)."""
        ix0 = max(0, int(floor((min_x - self.x0) / self.pitch)))
        iy0 = max(0, int(floor((min_y - self.y0) / self.pitch)))
        ix1 = min(self.nx - 1, int(ceil((max_x - self.x0) / self.pitch)))
        iy1 = min(self.ny - 1, int(ceil((max_y - self.y0) / self.pitch)))
        return ix0, iy0, ix1, iy1


@dataclass(frozen=True, slots=True)
class _Obstacle:
    owner: int                 # net id, or _FOREIGN for netless obstacles
    layers: tuple[int, ...]
    spine: tuple[Point, ...]
    radius: int
    clearance: int             # object-specific clearance floor
    is_pad: bool = False
    blocks_tracks: bool = True
    blocks_vias: bool = True


class _Rasters:
    """Lazily built legality rasters keyed by width/clearance class."""

    def __init__(self, board: PhysicalBoard, grid: _Grid, obstacles: list[_Obstacle],
                 holes: list[tuple[int, float, float, float]], margin: int) -> None:
        self.board = board
        self.grid = grid
        self.obstacles = obstacles
        self.holes = holes          # (owner, x, y, radius)
        self.margin = margin
        self._track: dict[tuple[int, int], tuple[list[int], list[int]]] = {}
        self._via: dict[tuple[int, int, int], list[int]] = {}
        g = grid
        self.px, self.py = np.meshgrid(g.xs, g.ys)                      # node centres
        self.mx, self.my = np.meshgrid(g.xs + g.pitch / 2, g.ys + g.pitch / 2)  # cell centres
        outline = board.outline.vertices
        cutouts = [c.vertices for c in board.outline.cutouts]
        self.inside_nodes = self._material(outline, cutouts, self.px, self.py)
        self.inside_cells = self._material(outline, cutouts, self.mx, self.my)
        self.edge_nodes = self._edge_distance(outline, cutouts, self.px, self.py)
        self.edge_cells = self._edge_distance(outline, cutouts, self.mx, self.my)

    @staticmethod
    def _material(outline, cutouts, px, py):
        inside = _polygon_inside(outline, px, py)
        for ring in cutouts:
            inside &= ~_polygon_inside(ring, px, py)
        return inside

    @staticmethod
    def _edge_distance(outline, cutouts, px, py):
        best = np.full(px.shape, np.inf)
        for ring in (outline, *cutouts):
            n = len(ring)
            for i in range(n):
                a, b = ring[i], ring[(i + 1) % n]
                best = np.minimum(best, _segment_distance(px, py, float(a.x_nm), float(a.y_nm),
                                                          float(b.x_nm), float(b.y_nm)))
        return best

    def track(self, width: int, clearance: int) -> tuple[list[int], list[int]]:
        """Owner lists for nodes and cell centres: _FREE, a net id or _FOREIGN."""
        key = (width, clearance)
        if key not in self._track:
            self._track[key] = (self._build(width / 2, clearance, self.px, self.py,
                                            self.inside_nodes, self.edge_nodes, for_via=False),
                                self._build(width / 2, clearance, self.mx, self.my,
                                            self.inside_cells, self.edge_cells, for_via=False))
        return self._track[key]

    def via(self, size: int, drill: int, clearance: int) -> list[int]:
        """Owner list (one plane) for a through via centred on each node."""
        key = (size, drill, clearance)
        if key not in self._via:
            g = self.grid
            stacked = self._build(size / 2, clearance, self.px, self.py, self.inside_nodes,
                                  self.edge_nodes, for_via=True, as_array=True)
            owner = stacked[0].copy()
            for layer in range(1, g.nl):
                other = stacked[layer]
                both = (owner != other)
                owner = np.where(both & (owner == _FREE), other, owner)
                owner = np.where(both & (other != _FREE) & (owner != other), _FOREIGN, owner)
            hole_clear = self.board.rules.minimum_hole_clearance_nm
            for _, x, y, radius in self.holes:
                reach = drill / 2 + radius + hole_clear + self.margin
                ix0, iy0, ix1, iy1 = g.window(x - reach, y - reach, x + reach, y + reach)
                if ix0 > ix1 or iy0 > iy1:
                    continue
                win = np.hypot(self.px[iy0:iy1 + 1, ix0:ix1 + 1] - x,
                               self.py[iy0:iy1 + 1, ix0:ix1 + 1] - y) < reach
                owner[iy0:iy1 + 1, ix0:ix1 + 1][win] = _FOREIGN
            self._via[key] = owner.ravel().tolist()
        return self._via[key]

    def _build(self, half_width: float, clearance: int, px, py, inside, edge, *,
               for_via: bool, as_array: bool = False):
        g = self.grid
        edge_clear = self.board.rules.minimum_clearance_nm
        legal_material = inside & (edge >= half_width + edge_clear + self.margin)
        owner = np.full((g.nl, g.ny, g.nx), _FREE, dtype=np.int32)
        for layer in range(g.nl):
            owner[layer][~legal_material] = _FOREIGN
        for item in self.obstacles:
            if for_via and not item.blocks_vias or not for_via and not item.blocks_tracks:
                continue
            gap = max(clearance, item.clearance)
            # Vias may never touch any pad, including their own net's.
            owner_id = _FOREIGN if (for_via and item.is_pad) else item.owner
            if for_via and item.is_pad:
                gap = max(gap, 1)
            reach = item.radius + gap + half_width + self.margin
            xs = [p.x_nm for p in item.spine]
            ys = [p.y_nm for p in item.spine]
            ix0, iy0, ix1, iy1 = g.window(min(xs) - reach, min(ys) - reach,
                                          max(xs) + reach, max(ys) + reach)
            if ix0 > ix1 or iy0 > iy1:
                continue
            wx = px[iy0:iy1 + 1, ix0:ix1 + 1]
            wy = py[iy0:iy1 + 1, ix0:ix1 + 1]
            hit = _spine_distance(item.spine, wx, wy) < reach
            if not hit.any():
                continue
            for layer in item.layers:
                window = owner[layer, iy0:iy1 + 1, ix0:ix1 + 1]
                if owner_id == _FOREIGN:
                    window[hit] = _FOREIGN
                    continue
                free = hit & (window == _FREE)
                clash = hit & (window != _FREE) & (window != owner_id)
                window[free] = owner_id
                window[clash] = _FOREIGN
        if as_array:
            return owner
        return owner.ravel().tolist()


# ---------------------------------------------------------------------------
# Net model


@dataclass(slots=True)
class _NetSpec:
    name: str
    nid: int
    width: int
    clearance: int
    via_size: int
    via_drill: int
    layers: tuple[int, ...]
    components: list[list[tuple[int, tuple[TrackSegment, ...], float]]]  # attach candidates per component
    span: int
    neck_width: int = 0
    neck_clearance: int = 0
    region: frozenset[int] = frozenset()     # nodes inside the net's breakout regions
    neck: frozenset[int] = frozenset()       # region nodes legal only at neck width
    neck_mid: frozenset[int] = frozenset()   # region cell centres legal only at neck width


@dataclass(slots=True)
class _NetRoute:
    paths: list[list[int]]                      # node sequences
    leads: list[tuple[TrackSegment, ...]]       # lead-in copper used
    nodes: set[int]                             # path nodes (all layers)
    via_xy: set[int]                            # plane indexes holding a via
    claims: set[int]
    via_claims: set[int]
    connected: bool
    length: float
    reason: str = ""


class _Occupancy:
    def __init__(self, size: int, plane: int) -> None:
        self.occ = [0] * size
        self.via_occ = [0] * plane
        self.history = [0.0] * size
        self.via_history = [0.0] * plane

    def add(self, route: _NetRoute, sign: int) -> None:
        occ, via_occ = self.occ, self.via_occ
        for node in route.claims:
            occ[node] += sign
        for xy in route.via_claims:
            via_occ[xy] += sign


def _disc_offsets(radius_in_pitches: float) -> tuple[tuple[int, int], ...]:
    r = int(ceil(radius_in_pitches))
    return tuple((dx, dy) for dx in range(-r, r + 1) for dy in range(-r, r + 1)
                 if hypot(dx, dy) < radius_in_pitches - 1e-9 or (dx == 0 and dy == 0))


def _segment_offsets(dx: int, dy: int, radius_in_pitches: float) -> tuple[tuple[int, int], ...]:
    """Grid offsets (from the segment start) within ``radius`` of segment (0,0)-(dx,dy)."""
    r = int(ceil(radius_in_pitches)) + 1
    result = []
    length2 = dx * dx + dy * dy
    for ox in range(-r, r + 2):
        for oy in range(-r, r + 2):
            t = max(0.0, min(1.0, (ox * dx + oy * dy) / length2)) if length2 else 0.0
            d = hypot(ox - t * dx, oy - t * dy)
            if d < radius_in_pitches - 1e-9 or (ox, oy) in ((0, 0), (dx, dy)):
                result.append((ox, oy))
    return tuple(result)


# ---------------------------------------------------------------------------
# Router


class _Router:
    def __init__(self, board: PhysicalBoard, options: UnifiedRouterOptions,
                 nets: list, on_progress) -> None:
        self.board = board
        self.options = options
        self.on_progress = on_progress
        self.grid = _Grid(board, options.pitch_nm)
        self.rules: dict[str, NetRoutingRule] = {r.net: r for r in board.net_routing_rules}
        self.clearance = RoutingClearanceIndex(board)
        self.net_ids: dict[str, int] = {}
        for net in board.nets:
            self.net_ids.setdefault(net.name, len(self.net_ids))
        obstacles, holes = self._obstacles()
        margin = options.raster_margin_nm if options.raster_margin_nm is not None else options.pitch_nm // 8
        self.rasters = _Rasters(board, self.grid, obstacles, holes, margin)
        self.occupancy = _Occupancy(self.grid.size, self.grid.plane)
        ranks, headings = signal_layer_preferences(board)
        g = self.grid
        self.layer_cost = [options.outer_layer_cost if i in (0, g.nl - 1) else 0.0 for i in range(g.nl)]
        self.heading = [headings.get(layer) for layer in g.layers]
        default_width = board.rules.default_track_width_nm
        self.default_width = default_width
        self.specs = [spec for spec in (self._spec(net) for net in nets) if spec is not None]
        self.routes: dict[str, _NetRoute] = {}

    # -- setup ------------------------------------------------------------

    def _net_id(self, name: str) -> int:
        if name not in self.net_ids:
            self.net_ids[name] = len(self.net_ids)
        return self.net_ids[name]

    def _obstacles(self):
        index = self.clearance
        g = self.grid
        layer_index = {layer: i for i, layer in enumerate(g.layers)}
        obstacles: list[_Obstacle] = []
        for item in index._objects:
            netless = item.net.startswith("<")
            obstacles.append(_Obstacle(
                _FOREIGN if netless else self._net_id(item.net),
                tuple(layer_index[l] for l in item.layers if l in layer_index),
                item.shape.spine, item.shape.radius_nm, item.clearance_nm, is_pad=item.is_pad))
        for keepout in index._keepouts:
            obstacles.append(_Obstacle(
                _FOREIGN, tuple(layer_index[l] for l in keepout.layers if l in layer_index),
                keepout.shape.spine, keepout.shape.radius_nm, keepout.clearance_nm,
                blocks_tracks=keepout.block_tracks, blocks_vias=keepout.block_vias))
        for layers, shape in index._macro_regions:
            obstacles.append(_Obstacle(
                _FOREIGN, tuple(layer_index[l] for l in layers if l in layer_index),
                shape.spine, shape.radius_nm, 0))
        holes = [(_FOREIGN if hole.net.startswith("<") else self._net_id(hole.net),
                  float(hole.position.x_nm), float(hole.position.y_nm), float(hole.radius_nm))
                 for hole in index._holes]
        return obstacles, holes

    def _spec(self, net) -> _NetSpec | None:
        board, g = self.board, self.grid
        rule = self.rules.get(net.name)
        width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
        clearance = max(board.rules.minimum_clearance_nm, rule.clearance_nm or 0 if rule else 0)
        allowed = set(routing_layers(board, net.name, rule))
        layers = tuple(i for i, layer in enumerate(g.layers) if layer in allowed)
        if not layers:
            return None
        spec = _NetSpec(net.name, self._net_id(net.name), width, clearance,
                        board.rules.default_via_size_nm, board.rules.default_via_drill_nm,
                        layers, [], 0)
        # Breakout rules let a net narrow next to its own lands (see breakout.py).
        if rule is not None and rule.breakout_length_nm and rule.breakout_width_nm \
                and rule.breakout_width_nm < width:
            spec.neck_width = max(rule.breakout_width_nm, board.rules.minimum_track_width_nm)
            spec.neck_clearance = max(board.rules.minimum_clearance_nm,
                                      rule.breakout_clearance_nm or rule.clearance_nm or 0)
            spec.region, spec.neck, spec.neck_mid = self._neck_nodes(net, spec, rule.breakout_length_nm)
        spec.components = self._components(net, spec)
        xs = [self.grid.unpack(node)[1] for comp in spec.components for node, _, _ in comp]
        ys = [self.grid.unpack(node)[2] for comp in spec.components for node, _, _ in comp]
        spec.span = (max(xs) - min(xs) + max(ys) - min(ys)) if xs else 0
        return spec

    def _neck_nodes(self, net, spec: _NetSpec, length: int):
        """Breakout-region nodes, and those legal there only at the neck width."""
        from .drc import placed_pad_shape
        from .placement import transformed_local_point
        g, board = self.grid, self.board
        main, main_mid = self.rasters.track(spec.width, spec.clearance)
        neck, neck_mid = self.rasters.track(spec.neck_width, spec.neck_clearance)
        nid = spec.nid
        region, nodes, cells = set(), set(), set()
        poses = {p.reference: p for p in board.placements}
        for pad in net.pads:
            pose = poses[pad.component]
            for land in board.footprints[pose.footprint].pads:
                if land.number != pad.pad:
                    continue
                shape = placed_pad_shape(transformed_local_point(pose, land.position), land, pose)
                reach = shape.radius_nm + length
                xs = [p.x_nm for p in shape.spine]; ys = [p.y_nm for p in shape.spine]
                ix0, iy0, ix1, iy1 = g.window(min(xs) - reach, min(ys) - reach, max(xs) + reach, max(ys) + reach)
                if ix0 > ix1 or iy0 > iy1:
                    continue
                wx = self.rasters.px[iy0:iy1 + 1, ix0:ix1 + 1]
                wy = self.rasters.py[iy0:iy1 + 1, ix0:ix1 + 1]
                # Strictly inside, with a small margin for the centreline test.
                inside = _spine_distance(shape.spine, wx, wy) < reach - g.pitch // 8
                for jy, jx in zip(*inside.nonzero()):
                    iy, ix = iy0 + int(jy), ix0 + int(jx)
                    for layer in spec.layers:
                        node = g.index(layer, ix, iy)
                        region.add(node)
                        if main[node] not in (_FREE, nid) and neck[node] in (_FREE, nid):
                            nodes.add(node)
                        if main_mid[node] not in (_FREE, nid) and neck_mid[node] in (_FREE, nid):
                            cells.add(node)
        return frozenset(region), frozenset(nodes), frozenset(cells)

    def _components(self, net, spec: _NetSpec):
        """Group already-joined pads; collect grid attach candidates per group."""
        from .drc import explicit_copper_connectivity
        from .hard_macros import macro_routing_pads
        board = self.board
        graph = explicit_copper_connectivity(board, only_nets=frozenset({net.name}))
        pads = tuple(macro_routing_pads(board, net))
        groups: dict[str, list[PadReference]] = {}
        for pad in pads:
            nodes = graph.pad_nodes.get(pad, ())
            root = graph.roots[nodes[0]] if nodes else f"pad:{pad.component}.{pad.pad}"
            groups.setdefault(root, []).append(pad)
        from .hard_macros import macro_port_layers, macro_routing_ports
        ports = macro_routing_ports(board, net.name)
        components = []
        for root, members in sorted(groups.items(), key=lambda item: str(item[1][0])):
            anchors: list[tuple[Point, tuple[int, ...]]] = []
            for pad in members:
                port = ports.get(pad)
                if port is not None:
                    # A hard macro exposes this pad group only at its port.
                    layers = tuple(self.grid.layers.index(l) for l in macro_port_layers(board, port))
                    anchors.append((port.position, layers))
                    continue
                anchors.extend(self._pad_anchors(pad, spec))
            if not root.startswith("pad:"):
                # Existing copper of this group (fanout, plane taps, partial routes).
                for index, via in enumerate(board.vias):
                    if via.net == net.name and graph.roots.get(f"via:{index}") == root:
                        anchors.append((via.position, tuple(range(self.grid.nl))))
                for index, track in enumerate(board.tracks):
                    if track.net == net.name and graph.roots.get(f"track:{index}") == root:
                        layer = self.grid.layers.index(track.layer)
                        anchors.append((track.start, (layer,)))
                        anchors.append((track.end, (layer,)))
            components.append(self._attach(anchors, spec))
        return components

    def _pad_anchors(self, pad: PadReference, spec: _NetSpec):
        from .placement import transformed_local_point
        board = self.board
        pose = next(p for p in board.placements if p.reference == pad.component)
        footprint = board.footprints[pose.footprint]
        result = []
        for land in footprint.pads:
            if land.number != pad.pad or land.kind in {PadKind.NON_PLATED_THROUGH_HOLE, PadKind.APERTURE}:
                continue
            position = transformed_local_point(pose, land.position)
            if land.kind is PadKind.SMD:
                side = CopperLayer.FRONT if pose.side.value == "front" else CopperLayer.BACK
                layers = (self.grid.layers.index(side),)
            else:
                layers = tuple(range(self.grid.nl))
            result.append((position, layers))
        return result

    def _attach(self, anchors, spec: _NetSpec):
        """Grid nodes reachable from anchors with exact, checked lead-ins."""
        from .pin_escape import checked_access_path
        g, options = self.grid, self.options
        owner, _ = self.rasters.track(spec.width, spec.clearance)
        radius = options.attach_radius_nm
        seen: dict[int, tuple[int, tuple[TrackSegment, ...], float]] = {}
        for position, layers in anchors:
            ix0, iy0, ix1, iy1 = g.window(position.x_nm - radius, position.y_nm - radius,
                                          position.x_nm + radius, position.y_nm + radius)
            near = sorted(((hypot(g.x0 + ix * g.pitch - position.x_nm, g.y0 + iy * g.pitch - position.y_nm), ix, iy)
                           for ix in range(ix0, ix1 + 1) for iy in range(iy0, iy1 + 1)))
            for layer in layers:
                if layer not in spec.layers:
                    continue
                found = 0
                for distance, ix, iy in near:
                    if distance > radius or found >= options.attach_candidates:
                        break
                    node = g.index(layer, ix, iy)
                    if owner[node] not in (_FREE, spec.nid) and node not in spec.neck:
                        continue
                    target = Point(g.x0 + ix * g.pitch, g.y0 + iy * g.pitch)
                    path = None
                    if node not in spec.neck:
                        path = checked_access_path(self.board, self.clearance, spec.name, position, target,
                                                   spec.width, g.layers[layer])
                    if path is None and spec.neck_width:
                        path = checked_access_path(self.board, self.clearance, spec.name, position, target,
                                                   spec.neck_width, g.layers[layer])
                    if path is None:
                        continue
                    cost = distance / g.pitch
                    if node not in seen or seen[node][2] > cost:
                        seen[node] = (node, tuple(path), cost)
                    found += 1
        return list(seen.values())

    # -- search -----------------------------------------------------------

    def _route_net(self, spec: _NetSpec, present: float) -> _NetRoute:
        g = self.grid
        components = [c for c in spec.components]
        route = _NetRoute([], [], set(), set(), set(), set(), False, 0.0)
        if len(components) < 2:
            route.connected = True
            return route
        if any(not c for c in components):
            route.reason = "no legal terminal access for %d of %d pad groups" % (
                sum(1 for c in components if not c), len(components))
            return route
        tree: dict[int, float] = {node: cost for node, _, cost in components[0]}
        leads = {node: lead for node, lead, _ in components[0]}
        used_leads: dict[int, tuple[TrackSegment, ...]] = {}
        remaining = list(range(1, len(components)))
        # Order targets by distance from the first component.
        base = g.unpack(next(iter(tree)))
        def dist(ci):
            node = components[ci][0][0]
            _, x, y = g.unpack(node)
            return abs(x - base[1]) + abs(y - base[2])
        remaining.sort(key=dist)
        first_root = True
        while remaining:
            targets: dict[int, tuple[int, tuple[TrackSegment, ...], float]] = {}
            for ci in remaining:
                for node, lead, cost in components[ci]:
                    targets[node] = (ci, lead, cost)
            found = self._search(spec, tree, targets, present)
            if found is None:
                route.reason = "no path to %d remaining pad groups" % len(remaining)
                route.connected = False
                self._finish_route(route, spec)
                return route
            path, end_node = found
            ci, lead, _ = targets[end_node]
            if first_root:
                start = path[0]
                if start in leads:
                    used_leads[start] = leads[start]
                first_root = False
            elif path[0] in leads:
                used_leads[path[0]] = leads[path[0]]
            used_leads[end_node] = lead
            route.paths.append(path)
            for node in path:
                tree[node] = 0.0
            for node, lead2, cost in components[ci]:
                if node not in tree:
                    tree[node] = cost
                    leads[node] = lead2
            remaining.remove(ci)
        route.leads = list(used_leads.values())
        route.connected = True
        self._finish_route(route, spec)
        return route

    def _finish_route(self, route: _NetRoute, spec: _NetSpec) -> None:
        g = self.grid
        nodes: set[int] = set()
        via_xy: set[int] = set()
        claims: set[int] = set()
        via_claims: set[int] = set()
        track_radius = (spec.width / 2 + spec.clearance + self.default_width / 2) / g.pitch
        via_radius = (spec.via_size / 2 + spec.clearance + self.default_width / 2) / g.pitch
        via_via_radius = (spec.via_size + spec.clearance) / g.pitch
        length = 0.0
        for path in route.paths:
            for a, b in zip(path, path[1:]):
                la, xa, ya = g.unpack(a)
                lb, xb, yb = g.unpack(b)
                if la != lb:
                    xy = ya * g.nx + xa
                    via_xy.add(xy)
                    continue
                dx, dy = xb - xa, yb - ya
                length += hypot(dx, dy)
                for ox, oy in _segment_offsets_cached(dx, dy, track_radius):
                    x, y = xa + ox, ya + oy
                    if 0 <= x < g.nx and 0 <= y < g.ny:
                        claims.add(la * g.plane + y * g.nx + x)
            nodes.update(path)
            if len(path) == 1:
                la, xa, ya = g.unpack(path[0])
                for ox, oy in _disc_offsets_cached(track_radius):
                    x, y = xa + ox, ya + oy
                    if 0 <= x < g.nx and 0 <= y < g.ny:
                        claims.add(la * g.plane + y * g.nx + x)
        for xy in via_xy:
            yv, xv = divmod(xy, g.nx)
            for ox, oy in _disc_offsets_cached(via_radius):
                x, y = xv + ox, yv + oy
                if 0 <= x < g.nx and 0 <= y < g.ny:
                    for layer in range(g.nl):
                        claims.add(layer * g.plane + y * g.nx + x)
            for ox, oy in _disc_offsets_cached(via_via_radius):
                x, y = xv + ox, yv + oy
                if 0 <= x < g.nx and 0 <= y < g.ny:
                    via_claims.add(y * g.nx + x)
        route.nodes, route.via_xy = nodes, via_xy
        route.claims, route.via_claims = claims, via_claims
        route.length = length

    def _search(self, spec: _NetSpec, sources: Mapping[int, float], targets, present: float):
        g, options = self.grid, self.options
        owner, mid_owner = self.rasters.track(spec.width, spec.clearance)
        via_owner = self.rasters.via(spec.via_size, spec.via_drill, spec.clearance)
        occ, via_occ = self.occupancy.occ, self.occupancy.via_occ
        hist, via_hist = self.occupancy.history, self.occupancy.via_history
        nid = spec.nid
        nx, plane = g.nx, g.plane
        layers = spec.layers
        layer_cost, heading = self.layer_cost, self.heading
        wrong = options.wrong_way_cost
        via_cost = options.via_cost
        target_xy = [(g.unpack(t)[1], g.unpack(t)[2]) for t in targets]
        tx0 = min(x for x, _ in target_xy); tx1 = max(x for x, _ in target_xy)
        ty0 = min(y for _, y in target_xy); ty1 = max(y for _, y in target_xy)
        src_xy = [g.unpack(s)[1:] for s in sources]
        margin = int(options.window_margin_nm // g.pitch)
        bx0 = max(0, min(tx0, min(x for x, _ in src_xy)) - margin)
        bx1 = min(nx - 1, max(tx1, max(x for x, _ in src_xy)) + margin)
        by0 = max(0, min(ty0, min(y for _, y in src_xy)) - margin)
        by1 = min(g.ny - 1, max(ty1, max(y for _, y in src_xy)) + margin)
        for attempt in range(3):
            result = self._astar(spec, sources, targets, present, (bx0, by0, bx1, by1),
                                 (tx0, ty0, tx1, ty1), owner, mid_owner, via_owner)
            if result is not None:
                return result
            if (bx0, by0, bx1, by1) == (0, 0, nx - 1, g.ny - 1):
                return None
            grow = margin * (attempt + 2) * 2
            bx0, by0 = max(0, bx0 - grow), max(0, by0 - grow)
            bx1, by1 = min(nx - 1, bx1 + grow), min(g.ny - 1, by1 + grow)
            if attempt == 1:
                bx0, by0, bx1, by1 = 0, 0, nx - 1, g.ny - 1
        return None

    def _astar(self, spec, sources, targets, present, box, tbox, owner, mid_owner, via_owner):
        g, options = self.grid, self.options
        occ, via_occ = self.occupancy.occ, self.occupancy.via_occ
        hist, via_hist = self.occupancy.history, self.occupancy.via_history
        nid = spec.nid
        neck, neck_mid, region = spec.neck, spec.neck_mid, spec.region
        nx, ny, plane, nl = g.nx, g.ny, g.plane, g.nl
        bx0, by0, bx1, by1 = box
        tx0, ty0, tx1, ty1 = tbox
        allowed = [False] * nl
        for layer in spec.layers:
            allowed[layer] = True
        layer_cost, heading = self.layer_cost, self.heading
        wrong = options.wrong_way_cost
        via_cost = options.via_cost
        best: dict[int, float] = {}
        parent: dict[int, int] = {}
        heap: list = []
        for node, cost in sources.items():
            layer, x, y = g.unpack(node)
            if not allowed[layer]:
                continue
            best[node] = cost
            parent[node] = -1
            hx = max(tx0 - x, 0, x - tx1)
            hy = max(ty0 - y, 0, y - ty1)
            h = max(hx, hy) + (_SQRT2 - 1) * min(hx, hy)
            heappush(heap, (cost + h, cost, node))
        expansions = 0
        limit = options.maximum_expansions
        while heap:
            f, cost, node = heappop(heap)
            if cost > best.get(node, 1e30):
                continue
            if node in targets:
                path = [node]
                while parent[path[-1]] != -1:
                    path.append(parent[path[-1]])
                path.reverse()
                return path, node
            expansions += 1
            if expansions > limit:
                return None
            layer, rest = divmod(node, plane)
            y, x = divmod(rest, nx)
            lc = layer_cost[layer]
            here_in_region = node in region
            head = heading[layer]
            for dx, dy, step in _MOVES:
                x2, y2 = x + dx, y + dy
                if x2 < bx0 or x2 > bx1 or y2 < by0 or y2 > by1:
                    continue
                n2 = node + dy * nx + dx
                o = owner[n2]
                if o != -1 and o != nid and not (here_in_region and n2 in neck):
                    continue
                if dx and dy:
                    cell = layer * plane + (y if dy > 0 else y - 1) * nx + (x if dx > 0 else x - 1)
                    o = mid_owner[cell]
                    if o != -1 and o != nid and not (here_in_region and n2 in region and cell in neck_mid):
                        continue
                base = step
                if head is not None:
                    if head == "h" and dy and not dx or head == "n" and dx and not dy:
                        base += wrong * step
                base += lc * step
                c = cost + base * (1.0 + hist[n2]) * (1.0 + present * occ[n2])
                if c < best.get(n2, 1e30):
                    best[n2] = c
                    parent[n2] = node
                    hx = max(tx0 - x2, 0, x2 - tx1)
                    hy = max(ty0 - y2, 0, y2 - ty1)
                    heappush(heap, (c + max(hx, hy) + 0.41421356 * min(hx, hy), c, n2))
            # Layer changes through a through via at this xy.
            xy = rest
            o = via_owner[xy]
            if o != -1 and o != nid:
                continue
            vpen = via_cost * (1.0 + via_hist[xy]) * (1.0 + present * via_occ[xy])
            for layer2 in spec.layers:
                if layer2 == layer:
                    continue
                n2 = layer2 * plane + xy
                o = owner[n2]
                if o != -1 and o != nid:
                    continue
                c = cost + vpen * (1.0 + hist[n2]) * (1.0 + present * occ[n2])
                if c < best.get(n2, 1e30):
                    best[n2] = c
                    parent[n2] = node
                    hx = max(tx0 - x, 0, x - tx1)
                    hy = max(ty0 - y, 0, y - ty1)
                    heappush(heap, (c + max(hx, hy) + 0.41421356 * min(hx, hy), c, n2))
        return None

    # -- negotiation ------------------------------------------------------

    def _conflicts(self, routes: Mapping[str, _NetRoute]) -> set[str]:
        occ, via_occ = self.occupancy.occ, self.occupancy.via_occ
        bad = set()
        for name, route in routes.items():
            if not route.connected:
                continue
            if any(occ[node] - (1 if node in route.claims else 0) > 0 for node in route.nodes):
                bad.add(name)
                continue
            if any(via_occ[xy] - (1 if xy in route.via_claims else 0) > 0 for xy in route.via_xy):
                bad.add(name)
        return bad

    def _penalise(self, routes: Mapping[str, _NetRoute], names: Iterable[str]) -> None:
        occ, hist = self.occupancy.occ, self.occupancy.history
        via_occ, via_hist = self.occupancy.via_occ, self.occupancy.via_history
        inc = self.options.history_increment
        for name in names:
            route = routes[name]
            for node in route.nodes:
                if occ[node] - (1 if node in route.claims else 0) > 0:
                    hist[node] += inc
            for xy in route.via_xy:
                if via_occ[xy] - (1 if xy in route.via_claims else 0) > 0:
                    via_hist[xy] += inc

    def negotiate(self, deadline: float | None, only: set[str] | None = None):
        """PathFinder loop. Routes persist on the router between calls."""
        options = self.options
        specs = sorted(self.specs, key=lambda s: (s.span, s.name))
        routes = self.routes
        present = options.present_factor
        best_snapshot = None
        pending = [s.name for s in specs if only is None or s.name in only or s.name not in routes]
        by_name = {s.name: s for s in specs}
        iteration = 0
        for iteration in range(1, options.maximum_iterations + 1):
            emit(self.on_progress, "unified_iteration", "started", iteration=iteration, nets=len(pending))
            for name in pending:
                spec = by_name[name]
                old = routes.get(name)
                if old is not None:
                    self.occupancy.add(old, -1)
                route = self._route_net(spec, present)
                routes[name] = route
                self.occupancy.add(route, +1)
            conflicts = self._conflicts(routes)
            failed = {name for name, route in routes.items() if not route.connected}
            score = (len(failed) + len(conflicts), sum(r.length for r in routes.values()))
            if best_snapshot is None or score < best_snapshot[0]:
                best_snapshot = (score, dict(routes), set(conflicts))
            emit(self.on_progress, "unified_iteration", "finished", iteration=iteration,
                 conflicted=len(conflicts), failed=len(failed))
            if not conflicts:
                break
            if deadline is not None and perf_counter() > deadline:
                break
            self._penalise(routes, conflicts)
            present *= options.present_growth
            # Reroute conflicted nets; retry failed nets now and then, since
            # congestion elsewhere may have moved.
            pending = [s.name for s in specs if s.name in conflicts or (s.name in failed and iteration % 3 == 0)]
            if not pending:
                break
        _, snapshot, conflicts = best_snapshot
        if snapshot != routes:
            for route in routes.values():
                self.occupancy.add(route, -1)
            routes.clear()
            routes.update(snapshot)
            for route in routes.values():
                self.occupancy.add(route, +1)
        return dict(routes), conflicts, iteration

    # -- materialisation --------------------------------------------------

    def copper(self, spec: _NetSpec, route: _NetRoute):
        g = self.grid
        tracks: list[TrackSegment] = []
        vias: list[Via] = []
        for lead in route.leads:
            tracks.extend(lead)
        neck, region = spec.neck, spec.region
        def width_of(a: int, b: int) -> int:
            if a in region and b in region and (a in neck or b in neck or _diagonal_mid(g, a, b) in spec.neck_mid):
                return spec.neck_width
            return spec.width
        for path in route.paths:
            if len(path) < 2:
                continue
            run_start = path[0]
            prev = path[0]
            direction = None
            run_width = None
            for node in path[1:]:
                la, xa, ya = g.unpack(prev)
                lb, xb, yb = g.unpack(node)
                if la != lb:
                    if run_start != prev:
                        tracks.append(TrackSegment(spec.name, g.point(run_start), g.point(prev),
                                                   run_width, g.layers[la]))
                    vias.append(Via(spec.name, g.point(prev), spec.via_size, spec.via_drill,
                                    g.layers[0], g.layers[-1]))
                    run_start, prev, direction, run_width = node, node, None, None
                    continue
                step = (xb - xa, yb - ya)
                width = width_of(prev, node)
                if direction is not None and (step != direction or width != run_width):
                    tracks.append(TrackSegment(spec.name, g.point(run_start), g.point(prev),
                                               run_width, g.layers[la]))
                    run_start = prev
                direction, run_width = step, width
                prev = node
            if run_start != prev:
                tracks.append(TrackSegment(spec.name, g.point(run_start), g.point(prev),
                                           run_width, g.layers[g.unpack(prev)[0]]))
        unique_vias = list(dict.fromkeys(vias))
        return tracks, unique_vias

    def commit(self, routes: Mapping[str, _NetRoute]):
        """Exact check; returns accepted copper per net and rejected net names."""
        from .surface_path import _track_inside_board
        index = RoutingClearanceIndex(self.board)
        accepted: dict[str, tuple[list[TrackSegment], list[Via]]] = {}
        rejected: dict[str, list[Point]] = {}
        by_name = {s.name: s for s in self.specs}
        order = sorted((name for name, route in routes.items() if route.connected and route.paths),
                       key=lambda name: (by_name[name].span, name))
        for name in order:
            spec = by_name[name]
            tracks, vias = self.copper(spec, routes[name])
            bad: list[Point] = []
            for track in tracks:
                if not (index.can_track(name, track.start, track.end, track.width_nm, track.layer)
                        and _track_inside_board(self.board, track.start, track.end, track.width_nm)):
                    bad.append(Point((track.start.x_nm + track.end.x_nm) // 2,
                                     (track.start.y_nm + track.end.y_nm) // 2))
            for via in vias:
                if not index.can_via(name, via.position, via.size_nm, via.from_layer, via.to_layer,
                                     drill_nm=via.drill_nm):
                    bad.append(via.position)
            if not bad and index.candidate_via_conflict(vias) is not None:
                bad.append(vias[0].position)
            if bad:
                rejected[name] = bad
                continue
            for track in tracks:
                index.add_track(track)
            for via in vias:
                index.add_via(via)
            accepted[name] = (tracks, vias)
        return accepted, rejected

    def penalise_points(self, name: str, points: list[Point], amount: float) -> None:
        g = self.grid
        hist = self.occupancy.history
        via_hist = self.occupancy.via_history
        for point in points:
            ix = int(round((point.x_nm - g.x0) / g.pitch))
            iy = int(round((point.y_nm - g.y0) / g.pitch))
            for ox, oy in _disc_offsets_cached(2.0):
                x, y = ix + ox, iy + oy
                if 0 <= x < g.nx and 0 <= y < g.ny:
                    via_hist[y * g.nx + x] += amount
                    for layer in range(g.nl):
                        hist[layer * g.plane + y * g.nx + x] += amount


def _diagonal_mid(g: _Grid, a: int, b: int) -> int:
    la, xa, ya = g.unpack(a)
    _, xb, yb = g.unpack(b)
    if xa == xb or ya == yb:
        return -1
    return la * g.plane + min(ya, yb) * g.nx + min(xa, xb)


_OFFSET_CACHE: dict = {}


def _segment_offsets_cached(dx: int, dy: int, radius: float):
    key = ("s", dx, dy, round(radius, 6))
    value = _OFFSET_CACHE.get(key)
    if value is None:
        value = _OFFSET_CACHE[key] = _segment_offsets(dx, dy, radius)
    return value


def _disc_offsets_cached(radius: float):
    key = ("d", round(radius, 6))
    value = _OFFSET_CACHE.get(key)
    if value is None:
        value = _OFFSET_CACHE[key] = _disc_offsets(radius)
    return value


# ---------------------------------------------------------------------------
# Public entry point


def route_unified(
    board: PhysicalBoard,
    global_route: GlobalRoutingResult,
    options: UnifiedRouterOptions | None = None,
    *,
    fanout_accesses=None,
    fanout_created_vias: frozenset[tuple[str, Point]] | None = None,
    fanout_created_tracks: tuple[TrackSegment, ...] | None = None,
    only_nets: frozenset[str] | None = None,
    on_progress: ProgressCallback | None = None,
) -> DetailedRoutingResult:
    """Route all ordinary nets with one negotiated-congestion loop.

    Signature-compatible with :func:`pcbir.detailed.route_detailed`.
    Existing copper is fixed; zone nets are deferred to fill.
    """

    options = options or UnifiedRouterOptions()
    started = perf_counter()
    deadline = started + options.time_budget_s if options.time_budget_s else None
    if board.hard_macros and set(board.materialized_macros) != {m.cluster for m in board.hard_macros}:
        raise ValueError("materialize hard-macro copper before detailed routing")
    from .zone_geometry import deferred_zone_nets
    rules = {r.net: r for r in board.net_routing_rules}
    zone_nets = deferred_zone_nets(board)
    from .drc import explicit_copper_connectivity
    connectivity = explicit_copper_connectivity(board)
    nets = [net for net in board.nets
            if len(net.pads) >= 2 and net.name not in zone_nets
            and (only_nets is None or net.name in only_nets)
            and (rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL)
            and not connectivity.net_connected(net)]
    already = [net for net in board.nets
               if len(net.pads) >= 2 and net.name not in zone_nets
               and (only_nets is None or net.name in only_nets)
               and (rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL)
               and connectivity.net_connected(net)]
    emit(on_progress, "unified_setup", "started", nets=len(nets))
    router = _Router(board, options, nets, on_progress)
    emit(on_progress, "unified_setup", "finished", nets=len(router.specs),
         grid=[router.grid.nx, router.grid.ny, router.grid.nl], seconds=round(perf_counter() - started, 2))
    accepted: dict[str, tuple[list[TrackSegment], list[Via]]] = {}
    rejected: dict[str, list[Point]] = {}
    iterations = 0
    retry: set[str] | None = None
    for round_index in range(options.commit_rounds):
        routes, conflicts, used = router.negotiate(deadline, retry)
        iterations += used
        candidates = {name: route for name, route in routes.items() if name not in conflicts}
        accepted, rejected = router.commit(candidates)
        emit(on_progress, "unified_commit", "finished", round=round_index + 1,
             accepted=len(accepted), rejected=len(rejected), conflicted=len(conflicts))
        if not rejected or (deadline is not None and perf_counter() > deadline):
            break
        for name, points in rejected.items():
            router.penalise_points(name, points, 4.0)
        retry = set(rejected)
    spec_names = {s.name for s in router.specs}
    new_tracks = tuple(t for name in sorted(accepted) for t in accepted[name][0])
    new_vias = tuple(v for name in sorted(accepted) for v in accepted[name][1])
    all_tracks = (*board.tracks, *new_tracks)
    all_vias = (*board.vias, *new_vias)
    metadata = dict(board.metadata)
    routed_names = set(accepted) | {net.name for net in already}
    unrouted = [net.name for net in nets if net.name not in accepted]
    success = not unrouted
    metadata.update({"detailed_routing": "complete" if success else "partial",
                     "detailed_router": "unified",
                     "global_routing_fingerprint": global_route.routing_fingerprint,
                     "fabrication_ready": "false"})
    routed = replace(board, tracks=tuple(all_tracks), vias=tuple(all_vias),
                     metadata=MappingProxyType(metadata))
    results = []
    for net in [*nets, *already]:
        tracks, vias = accepted.get(net.name, ((), ()))
        diagnostics = ()
        if net.name not in routed_names:
            last = router.routes.get(net.name)
            diagnostics = ("unified: rejected by exact commit check",) if net.name in rejected else (
                ("unified: no routable layer",) if net.name not in spec_names else
                (f"unified: {last.reason}" if last is not None and last.reason else
                 "unified: unresolved congestion",))
        results.append(DetailedNetResult(
            net.name, net.name in routed_names, len(tracks), len(vias),
            int(sum(hypot(t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm) for t in tracks)),
            0, diagnostics))
    for net in board.nets:
        if net.name in zone_nets and len(net.pads) >= 2 and (only_nets is None or net.name in only_nets):
            results.append(DetailedNetResult(net.name, False, 0, 0, 0, 0,
                                             ("zone net awaits verified fill and pad stitching",)))
    metrics = DetailedRoutingMetrics(
        routed_net_count=sum(r.connected for r in results),
        unrouted_net_count=sum(not r.connected for r in results),
        conflict_resource_count=0, total_conflict_overflow=0,
        track_count=len(new_tracks), via_count=len(new_vias),
        total_length_nm=sum(r.length_nm for r in results), passes=iterations)
    emit(on_progress, "unified_route", "finished", routed=len(accepted), unrouted=len(unrouted),
         seconds=round(perf_counter() - started, 2))
    fingerprint = _fingerprint(global_route.routing_fingerprint, routed, metrics)
    status = DetailedRoutingStatus.SUCCESS if success and not any(
        not r.connected and "zone net" not in " ".join(r.diagnostics) for r in results) else DetailedRoutingStatus.PARTIAL
    return DetailedRoutingResult(status, routed, tuple(results), metrics, len(board.tracks),
                                 len(board.vias), global_route.routing_fingerprint, fingerprint)
