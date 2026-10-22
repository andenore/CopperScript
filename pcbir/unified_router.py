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

from dataclasses import dataclass, field, replace
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
    PhysicalNet,
    Point,
    RouteKind,
    TrackSegment,
    Via,
    nm_from_mm,
)
from .progress import ProgressCallback, emit
from .routing import GlobalRoutingResult
from .routing_clearance import RoutingClearanceIndex
from .routing_costs import LayerBias
from .routing_layers import routing_layers, signal_layer_preferences

_FOREIGN = -2  # Obstacle owned by another net, by several nets or by none.
_FREE = -1
_SQRT2 = sqrt(2.0)
# 8 planar moves: (dx, dy, length in pitches).
_MOVES = ((1, 0, 1.0), (-1, 0, 1.0), (0, 1, 1.0), (0, -1, 1.0),
          (1, 1, _SQRT2), (1, -1, _SQRT2), (-1, 1, _SQRT2), (-1, -1, _SQRT2))


@dataclass(frozen=True, slots=True)
class UnifiedRouterOptions:
    pitch_nm: int | None = None    # None: 0.25 mm, finer where a ball grid needs it
    maximum_iterations: int = 150
    # Stop negotiating once this many iterations in a row have not improved
    # the best result (0: run to maximum_iterations). Boards that converge
    # stop when conflict-free; boards that cannot stop early. Conflicts can
    # plateau for dozens of iterations before the last ones clear, so a short
    # limit cuts off negotiations that would converge.
    stall_iterations: int = 60
    present_factor: float = 0.6
    present_growth: float = 1.5
    history_increment: float = 0.8
    via_cost_mm: float = 1.5       # a via costs as much as this much track
    wrong_way_cost: float = 0.5    # added per pitch against a layer's preferred heading
    outer_layer_cost: float = 0.0  # added per pitch on surface layers
    attach_radius_nm: int = nm_from_mm("1.5")
    # Near its own pads a net may narrow to the board minimum track width.
    neck_radius_nm: int = nm_from_mm("1.2")
    attach_candidates: int = 6
    window_margin_nm: int = nm_from_mm("2.5")
    present_limit: float = 0.0               # cap on the present-congestion factor (0 = none)
    window_growth_limit: int = 6             # persistent conflicts widen the window up to this factor
    maximum_expansions: int = 400_000
    commit_rounds: int = 4
    # After commit, re-check pours with the committed signal copper: a plane
    # pad counted as touching a pour may have been walled off; reroute it.
    pour_recheck: bool = True
    # Route members of length-match groups (buses) among themselves first;
    # the other nets then route around their committed copper. Off by
    # default: on the CopperProbe main board it ties on completion (0.873)
    # but its bus detours break max_length (DRC 1 -> 3); on an earlier
    # placement it gained 2 points. Needs length-aware bus routing first.
    buses_first: bool = False
    # With buses_first: bus members contest only each other, so they retake
    # their shortest path when a detour breaks max_length (as length_priority),
    # and only members within max_length keep their bus copper; the rest
    # route with the other nets.
    bus_length_budget: bool = False
    # With buses_first: lengthen short members of a length-match group over
    # its max_skew with checked bumps or serpentines, as the critical router
    # does, only for nets that declare tuning_amplitude_limit.
    bus_skew_tuning: bool = False
    # A net with a max_length that detours too far retakes its shortest path.
    # Off by default: on CopperProbe it cost 2 points of completion (0.873 ->
    # 0.852); the pushed-aside nets lose more than the length rule gains.
    length_priority: bool = False
    full_reroute_every: int = 0     # also reroute every net every N iterations (0: never)
    # Straighten this call's accepted copper afterwards with the ownership-safe
    # pass the legacy router uses (route_smoothing.smooth_owned_tracks): grid
    # search has no turn cost, so equal-length staircases of 45-degree jogs
    # survive. Existing board copper is never an input. On PCBWorld D3-A it
    # cuts bends by 38% with completion, DRC and vias unchanged.
    route_smoothing: bool = True
    # Extra cost (as history) on a ball grid's own surface layer: escapes are
    # short, so through-traffic between the balls should prefer inner layers.
    ball_field_cost: float = 2.0
    reuse_plane_vias: bool = False           # plane pads may join an existing via of their net
    ball_field_via_sites: bool = False       # new vias inside a ball array only at cell centres
    bga_fanout: bool = True
    raster_margin_nm: int | None = None
    time_budget_s: float | None = None
    # Supply islands carved from a spare plane layer before routing
    # (power_islands.py): "auto", a copper layer name, or None.
    power_islands: str | None = None
    # Nets without a profile width may narrow to the board minimum this close
    # to their pads (0 disables).
    implicit_neck_nm: int = nm_from_mm("2.5")
    # Trapped pins: exact local maze to open space within this radius (0 = off).
    local_escape_radius_nm: int = nm_from_mm("2.0")
    local_escape_step_nm: int = nm_from_mm("0.025")
    # Write the router's inputs to this state file before routing (replay_unified).
    save_inputs_path: str | None = None
    local_escape_limit: int = 40             # trapped pins per router that get an exact local maze
    # Route decoupling capacitors with the dedicated decoupler router before
    # the unified router (their copper is then fixed).
    pre_route_decouplers: bool = True
    # Escape crowded ordinary pins (fanout.route_fanout) before critical routing.
    escapes_first: bool = True
    # An escape plan (``escape_plan.plan_two_end_escapes``): nets negotiated
    # first, in this order, and soft layer assignments. A bias of rate r adds
    # r/10 to the history of its area's nodes on its layer while that net
    # searches (10 doubles the cost of track there, as in the detailed router).
    escape_net_order: tuple[str, ...] = ()
    escape_layer_biases: tuple[LayerBias, ...] = ()

    def __post_init__(self) -> None:
        if (self.pitch_nm is not None and self.pitch_nm <= 0) or self.maximum_iterations <= 0:
            raise ValueError("unified router pitch and iterations must be positive")
        if any(bias.rate < 0 for bias in self.escape_layer_biases):
            raise ValueError("escape layer biases cannot be negative")
        if self.stall_iterations < 0:
            raise ValueError("unified router stall iterations must not be negative")


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


def _polygon_distance(vertices: tuple[Point, ...], px, py):
    """Distance from points to a simple polygon of any shape (0 inside)."""
    xs = [float(p.x_nm) for p in vertices]
    ys = [float(p.y_nm) for p in vertices]
    best = None
    for i in range(len(vertices)):
        d = _segment_distance(px, py, xs[i], ys[i], xs[(i + 1) % len(xs)], ys[(i + 1) % len(ys)])
        best = d if best is None else np.minimum(best, d)
    return np.where(_polygon_inside(vertices, px, py), 0.0, best)


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
    polygon: bool = False      # spine is a simple polygon, not a convex hull


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
        self._via: dict[tuple[int, int, int], tuple] = {}
        self.via_mask = None        # (ny, nx) bool: False forbids a new via there
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
        return self._track_entry(width, clearance)[:2]

    def track_arrays(self, width: int, clearance: int):
        """The same owner rasters as flat int32 numpy arrays (native kernel)."""
        return self._track_entry(width, clearance)[2:]

    def _track_entry(self, width: int, clearance: int):
        key = (width, clearance)
        if key not in self._track:
            nodes = np.ascontiguousarray(self._build(width / 2, clearance, self.px, self.py,
                self.inside_nodes, self.edge_nodes, for_via=False, as_array=True).ravel())
            cells = np.ascontiguousarray(self._build(width / 2, clearance, self.mx, self.my,
                self.inside_cells, self.edge_cells, for_via=False, as_array=True).ravel())
            self._track[key] = (nodes.tolist(), cells.tolist(), nodes, cells)
        return self._track[key]

    def via(self, size: int, drill: int, clearance: int) -> list[int]:
        """Owner list (one plane) for a through via centred on each node."""
        key = (size, drill, clearance)
        if key not in self._via:
            g = self.grid
            # Copper spacing, or the drill's hole clearance to copper if larger.
            half = max(size / 2, drill / 2 + self.board.rules.minimum_hole_clearance_nm - clearance)
            stacked = self._build(half, clearance, self.px, self.py, self.inside_nodes,
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
            if self.via_mask is not None:
                owner = np.where(self.via_mask, owner, _FOREIGN)
            flat = np.ascontiguousarray(owner.ravel().astype(np.int32))
            self._via[key] = (flat.tolist(), flat)
        return self._via[key][0]

    def via_array(self, size: int, drill: int, clearance: int):
        self.via(size, drill, clearance)
        return self._via[(size, drill, clearance)][1]

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
            distance = (_polygon_distance(item.spine, wx, wy) if item.polygon
                        else _spine_distance(item.spine, wx, wy))
            hit = distance < reach
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
    via_factor: float = 1.0                  # extra via cost for nets with a via budget
    neck_width: int = 0
    neck_clearance: int = 0
    plane: frozenset[int] = frozenset()      # plane-region nodes (zone nets only)
    arrays: dict = field(default_factory=dict)  # numpy copies of the node sets, for the kernel
    region: frozenset[int] = frozenset()     # nodes inside the net's breakout regions
    neck: frozenset[int] = frozenset()       # region nodes legal only at neck width
    neck_mid: frozenset[int] = frozenset()   # region cell centres legal only at neck width
    target_only: tuple[int, ...] = ()        # layers entered only on target nodes (plane layers)
    optional: int = 0                        # trailing components that may stay unjoined
    via_budget: int | None = None            # new vias allowed (max_vias minus existing ones)
    max_length: int | None = None            # nm, the net's maximum routed length rule
    islands: int = 0                         # trailing components that are plane pieces
    explicit: dict = field(default_factory=dict)  # component index -> layers its plane path may not end on


@dataclass(slots=True)
class _NetRoute:
    paths: list[list[int]]                      # node sequences
    leads: list[tuple[TrackSegment, ...]]       # lead-in copper used, per path
    nodes: set[int]                             # path nodes (all layers)
    via_xy: set[int]                            # plane indexes holding a via
    claims: set[int]
    via_claims: set[int]
    connected: bool
    length: float
    reason: str = ""
    # Nodes covered by this route's own copper beyond a default-width track
    # (wide tracks, via pads), mapped to the path node or ("via", xy) that
    # owns them. Claims assume default-width neighbours, so these nodes are
    # checked against other nets' claims as well.
    body: dict = field(default_factory=dict)


class _Occupancy:
    """Shared negotiation state as flat numpy arrays (also read by the kernel)."""

    def __init__(self, size: int, plane: int, *, arrays: bool = True) -> None:
        # numpy arrays feed the native kernel; plain lists are much faster for
        # the pure-Python search, which indexes element by element.
        self.arrays = arrays
        if arrays:
            self.occ = np.zeros(size, dtype=np.int32)
            self.via_occ = np.zeros(plane, dtype=np.int32)
            self.history = np.zeros(size, dtype=np.float64)
            self.via_history = np.zeros(plane, dtype=np.float64)
        else:
            self.occ, self.via_occ = [0] * size, [0] * plane
            self.history, self.via_history = [0.0] * size, [0.0] * plane

    def copy_history(self, other: "_Occupancy") -> None:
        if self.arrays:
            self.history = np.asarray(other.history, dtype=np.float64).copy()
            self.via_history = np.asarray(other.via_history, dtype=np.float64).copy()
        else:
            self.history, self.via_history = list(other.history), list(other.via_history)

    def add(self, route: _NetRoute, sign: int) -> None:
        if not self.arrays:
            occ, via_occ = self.occ, self.via_occ
            for node in route.claims:
                occ[node] += sign
            for xy in route.via_claims:
                via_occ[xy] += sign
            return
        if route.claims:
            self.occ[np.fromiter(route.claims, dtype=np.int64, count=len(route.claims))] += sign
        if route.via_claims:
            self.via_occ[np.fromiter(route.via_claims, dtype=np.int64, count=len(route.via_claims))] += sign


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
                 nets: list, on_progress, *, explicit_pads: frozenset[PadReference] = frozenset()) -> None:
        self.board = board
        # Plane pads that must reach the plane with copper of their own, not
        # by lying in a pour piece (pour re-check, see route_unified).
        self.explicit_pads = frozenset(explicit_pads)
        self.options = options
        self.on_progress = on_progress
        pitch = options.pitch_nm or choose_pitch(board)
        self.pitch = pitch
        self.via_cost = options.via_cost_mm * 1_000_000 / pitch
        self.grid = _Grid(board, pitch)
        self.rules: dict[str, NetRoutingRule] = {r.net: r for r in board.net_routing_rules}
        self.zone_nets = {zone.net for zone in board.zones}
        self.clearance = RoutingClearanceIndex(board)
        self.net_ids: dict[str, int] = {}
        for net in board.nets:
            self.net_ids.setdefault(net.name, len(self.net_ids))
        obstacles, holes = self._obstacles()
        margin = options.raster_margin_nm if options.raster_margin_nm is not None else pitch // 8
        self.rasters = _Rasters(board, self.grid, obstacles, holes, margin)
        if options.ball_field_via_sites:
            self.rasters.via_mask = self._ball_field_via_mask()
        from .native import astar as native_astar
        self.native = native_astar()
        self.occupancy = _Occupancy(self.grid.size, self.grid.plane, arrays=self.native is not None)
        from array import array
        self._best = array("d", [1e30]) * self.grid.size
        self._parent = array("i", [-1]) * self.grid.size
        if self.native is not None:
            size = self.grid.size
            self._n_best = np.full(size, 1e30, dtype=np.float64)
            self._n_parent = np.full(size, -1, dtype=np.int32)
            self._n_flags = np.zeros(size, dtype=np.uint8)
            self._n_extra = np.zeros(size, dtype=np.float64)
            self._n_heap_cap = 3_000_000
            self._n_heap = np.zeros(self._n_heap_cap * 3, dtype=np.float64)  # 24-byte entries
            self._n_touched_cap = min(size, 4_000_000)
            self._n_touched = np.zeros(self._n_touched_cap, dtype=np.int32)
            self._n_path_cap = 1_000_000
            self._n_path = np.zeros(self._n_path_cap, dtype=np.int32)
        ranks, headings = signal_layer_preferences(board)
        g = self.grid
        self.layer_cost = [options.outer_layer_cost if i in (0, g.nl - 1) else 0.0 for i in range(g.nl)]
        self.heading = [headings.get(layer) for layer in g.layers]
        default_width = board.rules.default_track_width_nm
        self.default_width = default_width
        self._seed_ball_field_cost()
        self.specs = [spec for spec in (self._spec(net) for net in nets) if spec is not None]
        self.routes: dict[str, _NetRoute] = {}
        self._lead_nodes: dict[tuple, list[int]] = {}
        self.present = options.present_factor
        self.conflict_age: dict[str, int] = {}

    # -- setup ------------------------------------------------------------

    def _seed_ball_field_cost(self) -> None:
        cost = self.options.ball_field_cost
        if not cost:
            return
        from .package_patterns import circular_array
        from .placement import transformed_local_point
        g = self.grid
        hist = self.occupancy.history
        for pose in self.board.placements:
            try:
                array = circular_array(self.board, pose.reference)
            except ValueError:
                array = None
            if array is None:
                continue
            corners = [transformed_local_point(pose, p.position) for p in self.board.footprints[pose.footprint].pads]
            ix0, iy0, ix1, iy1 = g.window(min(p.x_nm for p in corners), min(p.y_nm for p in corners),
                                          max(p.x_nm for p in corners), max(p.y_nm for p in corners))
            layer = g.layers.index(CopperLayer.FRONT if pose.side.value == "front" else CopperLayer.BACK)
            for iy in range(iy0, iy1 + 1):
                for ix in range(ix0, ix1 + 1):
                    hist[g.index(layer, ix, iy)] += cost

    def _ball_field_via_mask(self):
        """Inside a full ball array, new vias only at cell centres (dogbone sites).

        Reference BGA layouts keep a regular via lattice so the inner layers
        get straight channels between via rows; a via dropped anywhere else in
        the field closes channels for everyone (the FX10 kit, for example,
        fans out every inner ball to its cell centre).
        """
        from .package_patterns import circular_array
        from .placement import transformed_local_point
        g = self.grid
        mask = None
        for pose in self.board.placements:
            try:
                array = circular_array(self.board, pose.reference)
            except ValueError:
                array = None
            if array is None:
                continue
            if mask is None:
                mask = np.ones((g.ny, g.nx), dtype=bool)
            pads = self.board.footprints[pose.footprint].pads
            points = [transformed_local_point(pose, p.position) for p in pads]
            x0 = min(p.x_nm for p in points); x1 = max(p.x_nm for p in points)
            y0 = min(p.y_nm for p in points); y1 = max(p.y_nm for p in points)
            ix0, iy0, ix1, iy1 = g.window(x0, y0, x1, y1)
            mask[iy0:iy1 + 1, ix0:ix1 + 1] = False
            # Cell centres: half a pitch off every ball row and column.
            def centred(values, origin):
                cells = (values - origin) / array.pitch_nm - 0.5
                return np.abs(cells - np.round(cells)) * array.pitch_nm <= g.pitch / 4
            on_x = centred(np.asarray(g.xs[ix0:ix1 + 1]), x0)
            on_y = centred(np.asarray(g.ys[iy0:iy1 + 1]), y0)
            mask[iy0:iy1 + 1, ix0:ix1 + 1] |= np.outer(on_y, on_x)
        return mask

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
            # The exact check spaces two nets by the larger of their rule
            # clearances: an obstacle's own net rule is part of its floor.
            rule = None if netless else self.rules.get(item.net)
            floor = max(item.clearance_nm, rule.clearance_nm or 0 if rule else 0)
            obstacles.append(_Obstacle(
                _FOREIGN if netless else self._net_id(item.net),
                tuple(layer_index[l] for l in item.layers if l in layer_index),
                item.shape.spine, item.shape.radius_nm, floor, is_pad=item.is_pad))
        for keepout in index._keepouts:
            obstacles.append(_Obstacle(
                _FOREIGN, tuple(layer_index[l] for l in keepout.layers if l in layer_index),
                keepout.shape.spine, keepout.shape.radius_nm, keepout.clearance_nm,
                blocks_tracks=keepout.block_tracks, blocks_vias=keepout.block_vias))
        for layers, shape in index._macro_regions:
            obstacles.append(_Obstacle(
                _FOREIGN, tuple(layer_index[l] for l in layers if l in layer_index),
                shape.spine, shape.radius_nm, 0))
        # A routing-reserved zone keeps other nets' tracks out, as the exact
        # check does; its own net and every net's vias may still enter it.
        reservations = index.zone_reservations
        for zone, outer in reservations.zones:
            floor = max(reservations.minimum_clearance_nm, zone.clearance_nm or 0,
                        reservations.clearances.get(zone.net, 0))
            obstacles.append(_Obstacle(
                self._net_id(zone.net), tuple(layer_index[l] for l in zone.layers if l in layer_index),
                outer.spine, outer.radius_nm, floor, blocks_vias=False, polygon=True))
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
        if rule is not None and rule.max_vias == 0:
            # No vias: stay on the copper layers of the net's own pads.
            pad_layers = set()
            for pad in net.pads:
                for _, layer_indexes, *_ in self._pad_anchors(pad, None):
                    pad_layers.update(g.layers[i] for i in layer_indexes)
            allowed &= pad_layers
        target_only = ()
        if net.name in self.zone_nets:
            # A plane net reaches its own zone layers even where they are
            # dedicated planes, but only by landing inside its fill.
            zone_layers = {layer for zone in board.zones if zone.net == net.name for layer in zone.layers}
            target_only = tuple(i for i, layer in enumerate(g.layers)
                                if layer in zone_layers and layer not in allowed)
            allowed |= zone_layers
        layers = tuple(i for i, layer in enumerate(g.layers) if layer in allowed)
        if not layers:
            return None
        spec = _NetSpec(net.name, self._net_id(net.name), width, clearance,
                        board.rules.default_via_size_nm, board.rules.default_via_drill_nm,
                        layers, [], 0)
        spec.target_only = target_only
        if rule is not None and rule.max_vias:
            spec.via_factor = 4.0
            existing = sum(1 for via in board.vias if via.net == net.name)
            spec.via_budget = max(0, rule.max_vias - existing)
        if rule is not None and rule.max_length_nm:
            spec.max_length = rule.max_length_nm
        # Breakout rules let a net narrow next to its own lands (see breakout.py).
        if rule is not None and rule.breakout_length_nm and rule.breakout_width_nm \
                and rule.breakout_width_nm < width:
            spec.neck_width = max(rule.breakout_width_nm, board.rules.minimum_track_width_nm)
            spec.neck_clearance = max(board.rules.minimum_clearance_nm,
                                      rule.breakout_clearance_nm or rule.clearance_nm or 0)
            spec.region, spec.neck, spec.neck_mid = self._neck_nodes(net, spec, rule.breakout_length_nm)
        elif (self.options.implicit_neck_nm and (rule is None or rule.width_nm is None)
              and width > board.rules.minimum_track_width_nm):
            # Without a profile width only the board minimum binds (DRC), so a
            # net may narrow near its own pads to pass between fine-pitch lands.
            spec.neck_width = board.rules.minimum_track_width_nm
            spec.neck_clearance = clearance
            spec.region, spec.neck, spec.neck_mid = self._neck_nodes(net, spec, self.options.implicit_neck_nm)
        spec.components = self._components(net, spec)
        if net.name in self.zone_nets:
            planes, islands, optional = self._zone_regions(net.name, spec)
            if self.options.reuse_plane_vias:
                planes = planes | self._plane_via_taps(net.name, spec, planes)
            spec.plane = planes
            # Small pours (surface islands) must also reach the main plane;
            # fragments of a layer that already has plane copper may stay apart.
            spec.components.extend([[(node, (), 0.0) for node in island] for island in islands])
            spec.components.extend([[(node, (), 0.0) for node in island] for island in optional])
            spec.islands = len(islands) + len(optional)
            spec.optional = len(optional)
        for key in ("neck", "neck_mid", "region", "plane"):
            nodes = getattr(spec, key)
            spec.arrays[key] = np.fromiter(nodes, dtype=np.int64, count=len(nodes))
        xs = [self.grid.unpack(node)[1] for comp in spec.components for node, _, _ in comp]
        ys = [self.grid.unpack(node)[2] for comp in spec.components for node, _, _ in comp]
        spec.span = (max(xs) - min(xs) + max(ys) - min(ys)) if xs else 0
        return spec

    def _plane_via_taps(self, name: str, spec: _NetSpec, planes) -> frozenset[int]:
        """Nodes on every routing layer at existing vias of a plane net that
        already land in its plane: a pad reaches the plane by joining such a
        via on its own layer, without a new via (reference layouts share
        return vias between a ball and its bypass capacitor)."""
        g = self.grid
        layers_all = list(g.layers)
        taps = set()
        for via in self.board.vias:
            if via.net != name:
                continue
            ix = int(round((via.position.x_nm - g.x0) / g.pitch))
            iy = int(round((via.position.y_nm - g.y0) / g.pitch))
            if not (0 <= ix < g.nx and 0 <= iy < g.ny):
                continue
            span = layers_all[layers_all.index(via.from_layer):layers_all.index(via.to_layer) + 1]
            # The plane must surround the via (fill reaches it from all
            # sides), not just touch it at one raster node.
            reach = max(1, int(round((via.size_nm / 2 + spec.clearance) / g.pitch)) + 1)
            def surrounded(index):
                return all(0 <= ix + dx < g.nx and 0 <= iy + dy < g.ny
                           and g.index(index, ix + dx, iy + dy) in planes
                           for dx, dy in ((reach, 0), (-reach, 0), (0, reach), (0, -reach)))
            if not any(surrounded(layers_all.index(layer)) for layer in span):
                continue
            for layer in span:
                index = layers_all.index(layer)
                if index in spec.layers:
                    taps.add(g.index(index, ix, iy))
        return frozenset(taps)

    def _zone_regions(self, name: str, spec: _NetSpec):
        """Plane target nodes of a zone net, its separate pours, and fragments.

        Fill is labelled per layer (8-connected over cells the net may use).
        Each layer's largest piece is plane copper when it is at least a
        quarter of the overall largest piece. Other pieces on a layer without
        plane copper are pours that must be joined; pieces on a plane layer
        are fragments (split islands, cut-off plane) joined when possible.
        """
        from .plane_estimate import _label
        g = self.grid
        owner, _ = self.rasters.track(spec.width, spec.clearance)
        owner_arr = np.asarray(owner, dtype=np.int32).reshape(g.nl, g.ny, g.nx)
        free = (owner_arr == _FREE) | (owner_arr == spec.nid)
        per_layer: dict[int, np.ndarray] = {}
        for zone in self.board.zones:
            if zone.net != name:
                continue
            inside = _polygon_inside(zone.outline.outer.vertices, self.rasters.px, self.rasters.py)
            for hole in zone.outline.holes:
                inside &= ~_polygon_inside(hole.vertices, self.rasters.px, self.rasters.py)
            for layer in zone.layers:
                index = g.layers.index(layer)
                mask = inside & free[index]
                per_layer[index] = per_layer[index] | mask if index in per_layer else mask
        minimum = max(4, int(nm_from_mm("0.8") ** 2 // (g.pitch * g.pitch)))
        pieces: dict[int, list[np.ndarray]] = {}
        for index, mask in per_layer.items():
            labels, count = _label(mask)
            if not count:
                continue
            flat = labels.ravel()
            sizes = np.bincount(flat, minlength=count + 1)
            order = np.argsort(-sizes[1:]) + 1
            found = []
            for k, label in enumerate(order):
                if sizes[label] < minimum and k:
                    break
                found.append(np.flatnonzero(flat == label) + index * g.plane)
            pieces[index] = found
        if not pieces:
            return frozenset(), [], []
        largest = max(p[0].size for p in pieces.values())
        planes, islands, optional = set(), [], []
        for index, found in pieces.items():
            first, rest = found[0], found[1:]
            if first.size * 4 >= largest:
                planes.update(first.tolist())
            else:
                islands.append(first.tolist())
            optional.extend(r.tolist() for r in rest)
        return frozenset(planes), islands, optional

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
        from .hard_macros import macro_owned_pads, macro_port_layers, macro_routing_ports
        ports = macro_routing_ports(board, net.name)
        owned = macro_owned_pads(board) if net.name in self.zone_nets else frozenset()
        components = []
        separate: list[list] = []
        for root, members in sorted(groups.items(), key=lambda item: str(item[1][0])):
            if owned and all(pad in owned for pad in members):
                # A hard macro proves its own plane returns at materialization.
                continue
            anchors: list[tuple[Point, tuple[int, ...]]] = []
            for pad in members:
                port = ports.get(pad)
                if port is not None:
                    # A hard macro exposes this pad group only at its port.
                    layers = tuple(self.grid.layers.index(l) for l in macro_port_layers(board, port))
                    anchors.append((port.position, layers))
                    continue
                pad_anchors = self._pad_anchors(pad, spec)
                # Lands sharing a number are one pin, but KiCad needs copper
                # between them unless the part declares them internally joined
                # (internal_pad_groups): each separate land is its own terminal,
                # as in the legacy router (plan 3.4).
                for piece in self._separate_land_pieces(pad):
                    separate.append([anchor for anchor in pad_anchors if anchor[0] in piece])
                    pad_anchors = [anchor for anchor in pad_anchors if anchor[0] not in piece]
                anchors.extend(pad_anchors)
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
            if self.explicit_pads and set(members) & self.explicit_pads:
                spec.explicit[len(components) - 1] = self._explicit_exclusion(members, spec)
        components.extend(self._attach(anchors, spec) for anchors in separate)
        return components

    def _separate_land_pieces(self, pad: PadReference) -> tuple[frozenset[Point], ...]:
        """Land positions of each piece of ``pad`` not joined by copper to its first land."""
        from .detailed import land_pieces
        pose = next(p for p in self.board.placements if p.reference == pad.component)
        footprint = self.board.footprints[pose.footprint]
        if any(pad.pad in group.numbers for group in footprint.internal_pad_groups):
            return ()
        return tuple(frozenset(position for _, position in piece)
                     for piece in land_pieces(pose, footprint, pad.pad)[1:])

    def _explicit_exclusion(self, pads, spec: _NetSpec) -> frozenset[int]:
        """The surface layers of these pads: a plane path from an explicit pad
        may not end there. It has to change layer (a via to an inner plane or
        the far side), because pour on its own layer can be walled off again
        by later signal copper."""
        poses = {p.reference: p for p in self.board.placements}
        return frozenset(self.grid.layers.index(CopperLayer.FRONT if poses[pad.component].side.value == "front"
                                                else CopperLayer.BACK)
                         for pad in pads if pad.component in poses)

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
            # Outward axis from the package centre, for fine-pitch lead-ins.
            dx, dy = position.x_nm - pose.position.x_nm, position.y_nm - pose.position.y_nm
            outward = None
            if max(abs(dx), abs(dy)) > nm_from_mm("0.3"):
                outward = (1 if dx > 0 else -1, 0) if abs(dx) >= abs(dy) else (0, 1 if dy > 0 else -1)
            result.append((position, layers, outward))
        return result

    def _attach(self, anchors, spec: _NetSpec):
        """Grid nodes reachable from anchors with exact, checked lead-ins."""
        from .pin_escape import checked_access_path
        g, options = self.grid, self.options
        owner, _ = self.rasters.track(spec.width, spec.clearance)
        radius = options.attach_radius_nm
        seen: dict[int, tuple[int, tuple[TrackSegment, ...], float]] = {}
        for anchor in anchors:
            position, layers = anchor[0], anchor[1]
            outward = anchor[2] if len(anchor) > 2 else None
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
                if found < 2 and outward is not None:
                    self._attach_extended(position, outward, layer, spec, owner, seen)
                if options.local_escape_radius_nm and not self._reaches_open(
                        [node for node in seen if node // g.plane == layer], spec):
                    self._attach_local(position, layer, spec, seen, outward)
        return list(seen.values())

    def _open_space(self, spec: _NetSpec):
        """Per node: True when it lies in a large connected region of free space.

        Labelled per layer on the raster of the narrowest width the net may
        use (4-connected); "large" is at least 4 mm² of free cells.
        """
        from .plane_estimate import _label
        width = spec.neck_width or spec.width
        clearance = spec.neck_clearance or spec.clearance
        key = (width, clearance)
        cache = self.__dict__.setdefault("_open_cache", {})
        if key not in cache:
            g = self.grid
            owner, _ = self.rasters.track(width, clearance)
            arr = np.asarray(owner, dtype=np.int32).reshape(g.nl, g.ny, g.nx)
            minimum = max(16, int(nm_from_mm("2") ** 2 // (g.pitch * g.pitch)))
            result = np.zeros(g.size, dtype=bool)
            for layer in range(g.nl):
                labels, count = _label(arr[layer] == _FREE)
                if not count:
                    continue
                sizes = np.bincount(labels.ravel(), minlength=count + 1)
                big = sizes >= minimum
                big[0] = False
                result[layer * g.plane:(layer + 1) * g.plane] = big[labels.ravel()]
            cache[key] = result
        return cache[key]

    def _reaches_open(self, nodes, spec: _NetSpec, limit: int = 600) -> bool:
        """Whether a short flood from ``nodes`` (through free or own-net cells
        on the same layer, at the net's narrowest width) reaches open space."""
        if not nodes:
            return False
        g = self.grid
        open_space = self._open_space(spec)
        owner, _ = self.rasters.track(spec.neck_width or spec.width, spec.neck_clearance or spec.clearance)
        nid = spec.nid
        stack, visited = list(nodes), set(nodes)
        while stack and len(visited) < limit:
            node = stack.pop()
            if open_space[node]:
                return True
            layer, rest = divmod(node, g.plane)
            y, x = divmod(rest, g.nx)
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                if 0 <= x + dx < g.nx and 0 <= y + dy < g.ny:
                    other = node + dy * g.nx + dx
                    if other not in visited and owner[other] in (_FREE, nid):
                        visited.add(other)
                        stack.append(other)
        return len(visited) >= limit

    def _attach_local(self, position, layer, spec, seen, outward=None) -> None:
        """Exact-geometry escape for a pin whose grid neighbourhood is a pocket.

        The legality raster is conservative by a fraction of the pitch, so
        a minimum-width channel between fixed copper can vanish from it. A
        small exact maze (pin_access.local_access_path) then reaches the
        nearest grid nodes that lie in open space.
        """
        from .pin_access import local_access_path
        g, options = self.grid, self.options
        spent = self.__dict__.get("_local_spent", 0)
        if spent >= options.local_escape_limit:
            return
        self._local_spent = spent + 1
        open_space = self._open_space(spec)
        width = spec.neck_width or spec.width
        reach = options.local_escape_radius_nm
        ix0, iy0, ix1, iy1 = g.window(position.x_nm - reach, position.y_nm - reach,
                                      position.x_nm + reach, position.y_nm + reach)
        owner, _ = self.rasters.track(spec.width, spec.clearance)
        def rank(ix, iy):
            dx, dy = g.x0 + ix * g.pitch - position.x_nm, g.y0 + iy * g.pitch - position.y_nm
            distance = hypot(dx, dy)
            # Targets ahead of the pin (along its outward axis) first: a pin's
            # exit is rarely back across its own package.
            behind = outward is not None and dx * outward[0] + dy * outward[1] < 0
            return (distance * (2.0 if behind else 1.0), distance, ix, iy)
        near = sorted(rank(ix, iy) for ix in range(ix0, ix1 + 1) for iy in range(iy0, iy1 + 1))
        found = tries = misses = 0
        for _, distance, ix, iy in near:
            # A failed maze is the expensive case; four in a row mean the pin
            # is enclosed beyond this radius (a dense array without a dogbone).
            if distance > reach or found >= 3 or tries >= 12 or misses >= 4:
                break
            node = g.index(layer, ix, iy)
            if not open_space[node] or owner[node] != _FREE:
                continue
            tries += 1
            target = Point(g.x0 + ix * g.pitch, g.y0 + iy * g.pitch)
            path = local_access_path(self.board, self.clearance, spec.name, position, target,
                                     g.layers[layer], width, step_nm=options.local_escape_step_nm,
                                     detour_nm=nm_from_mm("0.5"), maximum_states=20000)
            if not path:
                misses += 1
                continue
            misses = 0
            cost = 2 * distance / g.pitch
            if node not in seen or seen[node][2] > cost:
                seen[node] = (node, tuple(path), cost)
            found += 1

    def _attach_extended(self, position, outward, layer, spec, owner, seen):
        """Straight out along the pad's outward axis, then a checked lead-in.

        Fine-pitch pins rarely have a legal grid node within reach of a
        two-leg lead-in; their channel is the pin axis itself.
        """
        if outward is None:
            return
        from .pin_escape import checked_access_path
        g = self.grid
        widths = [spec.width] + ([spec.neck_width] if spec.neck_width else [])
        for extension_mm in ("0.3", "0.5", "0.75", "1.0", "1.5"):
            extension = nm_from_mm(extension_mm)
            corner = Point(position.x_nm + outward[0] * extension, position.y_nm + outward[1] * extension)
            for width in widths:
                if not self.clearance.can_track(spec.name, position, corner, width, g.layers[layer]):
                    continue
                stem = TrackSegment(spec.name, position, corner, width, g.layers[layer])
                reach = nm_from_mm("0.6")
                ix0, iy0, ix1, iy1 = g.window(corner.x_nm - reach, corner.y_nm - reach,
                                              corner.x_nm + reach, corner.y_nm + reach)
                near = sorted(((hypot(g.x0 + ix * g.pitch - corner.x_nm, g.y0 + iy * g.pitch - corner.y_nm), ix, iy)
                               for ix in range(ix0, ix1 + 1) for iy in range(iy0, iy1 + 1)))
                found = 0
                for distance, ix, iy in near:
                    if found >= 3:
                        break
                    node = g.index(layer, ix, iy)
                    if owner[node] not in (_FREE, spec.nid):
                        continue
                    target = Point(g.x0 + ix * g.pitch, g.y0 + iy * g.pitch)
                    path = checked_access_path(self.board, self.clearance, spec.name, corner, target,
                                               width, g.layers[layer])
                    if path is None:
                        continue
                    cost = (extension + distance) / g.pitch
                    if node not in seen or seen[node][2] > cost:
                        seen[node] = (node, (stem, *path), cost)
                    found += 1
                if found:
                    return

    # -- search -----------------------------------------------------------

    def _route_plane_net(self, spec: _NetSpec, present: float) -> _NetRoute:
        """Join every pad group (and small pour) of a zone net to its plane."""
        g = self.grid
        route = _NetRoute([], [], set(), set(), set(), set(), False, 0.0)
        targets = set(spec.plane)
        used_leads: dict[int, tuple[TrackSegment, ...]] = {}
        missing = 0
        whole = (0, 0, g.nx - 1, g.ny - 1)
        owner, mid_owner = self.rasters.track(spec.width, spec.clearance)
        via_owner = self.rasters.via(spec.via_size, spec.via_drill, spec.clearance)
        margin = self._margin(spec)
        count = len(spec.components)
        first_island = count - spec.islands
        first_optional = count - spec.optional
        # Plane pieces first: once joined they are targets for the pads.
        order = list(range(first_island, count)) + list(range(first_island))
        for position in order:
            component = spec.components[position]
            optional = position >= first_optional
            if not component:
                missing += not optional
                continue
            excluded = spec.explicit.get(position)
            if excluded is None and any(node in targets for node, _, _ in component):
                if position >= first_island:
                    targets.update(node for node, _, _ in component)
                continue  # Already inside the plane's fill.
            sources = {node: cost + self._lead_penalty(lead, present) for node, lead, cost in component}
            leads = {node: lead for node, lead, _ in component}
            if excluded is not None:
                # Explicit copper: the path must end on another layer's plane.
                allowed = {node for node in targets if node // g.plane not in excluded}
                found = self._astar(spec, sources, allowed, present, whole, whole,
                                    owner, mid_owner, via_owner)
                if found is None:
                    missing += not optional
                    continue
                path, _ = found
                route.paths.append(path)
                route.leads.append(tuple(leads.get(path[0], ())))
                targets.update(path)
                continue
            xs = [g.unpack(n)[1] for n in sources]; ys = [g.unpack(n)[2] for n in sources]
            box = (max(0, min(xs) - margin), max(0, min(ys) - margin),
                   min(g.nx - 1, max(xs) + margin), min(g.ny - 1, max(ys) + margin))
            found = self._astar(spec, sources, targets, present, box, whole, owner, mid_owner, via_owner)
            if found is None:
                found = self._astar(spec, sources, targets, present, whole, whole, owner, mid_owner, via_owner)
            if found is None:
                missing += not optional
                continue
            path, _ = found
            route.paths.append(path)
            route.leads.append(tuple(leads.get(path[0], ())))
            targets.update(path)
            if position >= first_island:
                targets.update(sources)
        route.connected = missing == 0
        if missing:
            route.reason = f"{missing} of {count - spec.optional} pad groups cannot reach the plane"
        self._finish_route(route, spec)
        return route

    def _route_length_nm(self, route: _NetRoute) -> float:
        leads = sum(hypot(t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm)
                    for lead in route.leads for t in lead)
        return route.length * self.grid.pitch + leads

    def _route_net(self, spec: _NetSpec, present: float) -> _NetRoute:
        route = self._route_net_budgeted(spec, present)
        if (self.options.length_priority and spec.max_length and present and route.connected
                and self._route_length_nm(route) > spec.max_length):
            # A detour around congestion broke the length rule: take the
            # shortest path through other nets' copper instead, so the
            # negotiation moves them rather than this net.
            direct = self._route_net_budgeted(spec, 0.0)
            if direct.connected and self._route_length_nm(direct) < self._route_length_nm(route):
                route = direct
        return route

    def _route_net_budgeted(self, spec: _NetSpec, present: float) -> _NetRoute:
        route = self._route_net_once(spec, present)
        if spec.via_budget is not None and len(route.via_xy) > spec.via_budget:
            # Over the net's via budget: search again with vias priced far
            # higher; a budget overrun stays a failure, not a DRC error.
            factor = spec.via_factor
            spec.via_factor = factor * 10
            try:
                retry = self._route_net_once(spec, present)
            finally:
                spec.via_factor = factor
            route = retry if len(retry.via_xy) <= len(route.via_xy) else route
            if len(route.via_xy) > spec.via_budget:
                route = _NetRoute([], [], set(), set(), set(), set(), False, 0.0,
                                  f"needs {len(route.via_xy)} new vias, max_vias leaves {spec.via_budget}")
        return route

    def _route_net_once(self, spec: _NetSpec, present: float) -> _NetRoute:
        if spec.plane:
            return self._route_plane_net(spec, present)
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
        tree: dict[int, float] = {node: cost + self._lead_penalty(lead, present)
                                  for node, lead, cost in components[0]}
        leads = {node: lead for node, lead, _ in components[0]}
        used_leads: dict[tuple[TrackSegment, ...], bool] = {}
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
            extra = {node: cost + self._lead_penalty(lead, present)
                     for node, (_, lead, cost) in targets.items()}
            found = self._search(spec, tree, targets, present, extra)
            if found is None:
                route.reason = "no path to %d remaining pad groups" % len(remaining)
                route.connected = False
                self._finish_route(route, spec)
                return route
            path, end_node = found
            ci, lead, _ = targets[end_node]
            # Leads are per component: two pad groups may share an attach
            # node, so key used leads by their copper, not by the node.
            copper: list[TrackSegment] = []
            start_lead = leads.get(path[0], ())
            if start_lead and start_lead not in used_leads:
                used_leads[start_lead] = True
                copper.extend(start_lead)
            if lead and lead not in used_leads:
                used_leads[lead] = True
                copper.extend(lead)
            route.paths.append(path)
            route.leads.append(tuple(copper))
            for node in path:
                tree[node] = 0.0
            for node, lead2, cost in components[ci]:
                if node not in tree:
                    tree[node] = cost + self._lead_penalty(lead2, present)
                    leads[node] = lead2
            remaining.remove(ci)
        route.connected = True
        self._finish_route(route, spec)
        return route

    def _finish_route(self, route: _NetRoute, spec: _NetSpec) -> None:
        g = self.grid
        nodes: set[int] = set()
        via_xy: set[int] = set()
        claims: set[int] = set()
        via_claims: set[int] = set()
        body: dict = {}
        via_body = (spec.via_size - self.default_width) / 2 / g.pitch
        # Claims assume a neighbour at least as wide as this net. Grid nodes
        # cannot carry a wide net's extra half-width below one pitch, and two
        # wider-than-default nets would otherwise pass negotiation closer than
        # the exact check allows (each claimed only a default-width neighbour).
        neighbour = max(self.default_width, spec.width)
        track_radius = (spec.width / 2 + spec.clearance + neighbour / 2) / g.pitch
        # Each via's copper keeps the clearance and its drill the hole clearance
        # from foreign copper, as the exact check requires; the larger decides.
        via_reach = max(spec.via_size / 2 + spec.clearance,
                        spec.via_drill / 2 + self.board.rules.minimum_hole_clearance_nm)
        via_radius = (via_reach + self.default_width / 2) / g.pitch
        via_via_radius = (via_reach + spec.via_size / 2) / g.pitch
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
        # Lead-ins are off-grid copper: claim around them and treat the
        # nodes along them as path nodes, so they negotiate like paths.
        layer_of = {layer: i for i, layer in enumerate(g.layers)}
        for leads in route.leads:
            for track in leads:
                layer = layer_of[track.layer]
                radius_nm = track.width_nm / 2 + spec.clearance + max(self.default_width, track.width_nm) / 2
                near = g.pitch / 2
                for node, distance in self._segment_nodes(track.start, track.end, layer, radius_nm):
                    claims.add(node)
                    if distance <= near:
                        nodes.add(node)
        for xy in via_xy:
            yv, xv = divmod(xy, g.nx)
            if via_body >= 0.5:
                for ox, oy in _disc_offsets_cached(via_body):
                    x, y = xv + ox, yv + oy
                    if 0 <= x < g.nx and 0 <= y < g.ny:
                        for layer in range(g.nl):
                            body[layer * g.plane + y * g.nx + x] = ("via", xy)
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
        route.body = {node: owner for node, owner in body.items() if node not in nodes}
        route.length = length

    def _lead_penalty(self, lead: tuple[TrackSegment, ...], present: float) -> float:
        """Congestion price of an off-grid lead-in, so attach choices negotiate too."""
        if not lead:
            return 0.0
        nodes = self._lead_nodes.get(lead)
        if nodes is None:
            g = self.grid
            layer_of = {layer: i for i, layer in enumerate(g.layers)}
            nodes = []
            for track in lead:
                nodes.extend(node for node, distance in self._segment_nodes(
                    track.start, track.end, layer_of[track.layer], g.pitch / 2 + 1) )
            self._lead_nodes[lead] = nodes
        if not nodes:
            return 0.0
        hist, occ = self.occupancy.history, self.occupancy.occ
        if not self.occupancy.arrays:
            return sum(hist[n] + present * occ[n] for n in nodes)
        index = np.asarray(nodes, dtype=np.int64)
        return float(hist[index].sum() + present * occ[index].sum())

    def _segment_nodes(self, start: Point, end: Point, layer: int, radius_nm: float):
        """(node, distance) for grid nodes strictly within radius of a segment."""
        g = self.grid
        ix0, iy0, ix1, iy1 = g.window(min(start.x_nm, end.x_nm) - radius_nm, min(start.y_nm, end.y_nm) - radius_nm,
                                      max(start.x_nm, end.x_nm) + radius_nm, max(start.y_nm, end.y_nm) + radius_nm)
        ax, ay, bx, by = start.x_nm, start.y_nm, end.x_nm, end.y_nm
        dx, dy = bx - ax, by - ay
        length2 = dx * dx + dy * dy
        result = []
        for iy in range(iy0, iy1 + 1):
            py = g.y0 + iy * g.pitch
            for ix in range(ix0, ix1 + 1):
                px = g.x0 + ix * g.pitch
                t = 0.0 if not length2 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / length2))
                distance = hypot(px - (ax + t * dx), py - (ay + t * dy))
                if distance < radius_nm:
                    result.append((layer * g.plane + iy * g.nx + ix, distance))
        return result

    def _margin(self, spec: _NetSpec) -> int:
        age = self.conflict_age.get(spec.name, 0)
        scale = min(self.options.window_growth_limit, 1 + age // 3)
        return int(self.options.window_margin_nm * scale // self.grid.pitch)

    def _search(self, spec: _NetSpec, sources: Mapping[int, float], targets, present: float,
                target_extra=None):
        bias = self._escape_bias(spec.name)
        if bias is None:
            return self._search_unbiased(spec, sources, targets, present, target_extra)
        nodes, amount = bias
        history = self.occupancy.history
        if self.occupancy.arrays:
            saved = history[nodes].copy()
            history[nodes] += amount
        else:
            saved = [history[node] for node in nodes]
            for node, extra in zip(nodes, amount):
                history[node] += extra
        try:
            return self._search_unbiased(spec, sources, targets, present, target_extra)
        finally:
            if self.occupancy.arrays:
                history[nodes] = saved
            else:
                for node, value in zip(nodes, saved):
                    history[node] = value

    def _escape_bias(self, net: str):
        """(nodes, extra history) of ``net``'s escape-plan layer biases, cached per net."""
        cache = self.__dict__.setdefault("_escape_bias_cache", {})
        if net in cache:
            return cache[net]
        g = self.grid
        extra: dict[int, float] = {}
        for bias in self.options.escape_layer_biases:
            if bias.net != net or bias.layer not in g.layers or not bias.rate:
                continue
            layer = g.layers.index(bias.layer)
            ix0, iy0, ix1, iy1 = g.window(bias.area.min_x, bias.area.min_y, bias.area.max_x, bias.area.max_y)
            for iy in range(iy0, iy1 + 1):
                for ix in range(ix0, ix1 + 1):
                    node = g.index(layer, ix, iy)
                    extra[node] = extra.get(node, 0.0) + bias.rate / 10
        if not extra:
            cache[net] = None
        elif self.occupancy.arrays:
            nodes = np.fromiter(sorted(extra), dtype=np.int64, count=len(extra))
            cache[net] = (nodes, np.array([extra[int(node)] for node in nodes], dtype=np.float64))
        else:
            nodes = sorted(extra)
            cache[net] = (nodes, [extra[node] for node in nodes])
        return cache[net]

    def _search_unbiased(self, spec: _NetSpec, sources: Mapping[int, float], targets, present: float,
                         target_extra=None):
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
        via_cost = self.via_cost
        target_xy = [(g.unpack(t)[1], g.unpack(t)[2]) for t in targets]
        tx0 = min(x for x, _ in target_xy); tx1 = max(x for x, _ in target_xy)
        ty0 = min(y for _, y in target_xy); ty1 = max(y for _, y in target_xy)
        src_xy = [g.unpack(s)[1:] for s in sources]
        margin = self._margin(spec)
        bx0 = max(0, min(tx0, min(x for x, _ in src_xy)) - margin)
        bx1 = min(nx - 1, max(tx1, max(x for x, _ in src_xy)) + margin)
        by0 = max(0, min(ty0, min(y for _, y in src_xy)) - margin)
        by1 = min(g.ny - 1, max(ty1, max(y for _, y in src_xy)) + margin)
        for attempt in range(3):
            result = self._astar(spec, sources, targets, present, (bx0, by0, bx1, by1),
                                 (tx0, ty0, tx1, ty1), owner, mid_owner, via_owner, target_extra)
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

    def _astar(self, spec, sources, targets, present, box, tbox, owner, mid_owner, via_owner,
               target_extra=None):
        """A* over the shared grid; arrays are reused across searches."""
        if self.native is not None:
            result = self._astar_native(spec, sources, targets, present, box, tbox, target_extra)
            if result is NotImplemented and self._grow_native_buffers():
                result = self._astar_native(spec, sources, targets, present, box, tbox, target_extra)
            if result is not NotImplemented:
                return result
        g, options = self.grid, self.options
        occ, via_occ = self.occupancy.occ, self.occupancy.via_occ
        hist, via_hist = self.occupancy.history, self.occupancy.via_history
        nid = spec.nid
        neck, neck_mid, region = spec.neck, spec.neck_mid, spec.region
        use_neck = bool(neck or neck_mid)
        nx, plane = g.nx, g.plane
        bx0, by0, bx1, by1 = box
        tx0, ty0, tx1, ty1 = tbox
        layer_cost, heading = self.layer_cost, self.heading
        wrong = options.wrong_way_cost
        via_cost = self.via_cost * spec.via_factor
        best, parent = self._best, self._parent
        touched = []
        heap: list = []
        push, pop = heappush, heappop
        layers = spec.layers
        allowed = set(layers)
        target_only = set(spec.target_only)
        INF = 1e30
        for node, cost in sources.items():
            layer, rest = divmod(node, plane)
            if layer not in allowed:
                continue
            y, x = divmod(rest, nx)
            best[node] = cost
            parent[node] = -1
            touched.append(node)
            hx = tx0 - x if x < tx0 else (x - tx1 if x > tx1 else 0)
            hy = ty0 - y if y < ty0 else (y - ty1 if y > ty1 else 0)
            push(heap, (cost + (hx if hx > hy else hy) + 0.41421356 * (hy if hx > hy else hx), cost, node))
        expansions = 0
        limit = options.maximum_expansions
        found = None
        # Per layer: move list with heading-adjusted base costs.
        move_tables = []
        for layer in range(g.nl):
            head = heading[layer]
            lc = layer_cost[layer]
            table = []
            for dx, dy, step in _MOVES:
                base = step * (1.0 + lc)
                if head is not None and (head == "h" and dy and not dx or head == "n" and dx and not dy):
                    base += wrong * step
                cell_dy = 0 if dy > 0 else -1
                cell_dx = 0 if dx > 0 else -1
                table.append((dx, dy, dy * nx + dx, base, bool(dx and dy), cell_dy * nx + cell_dx))
            move_tables.append(table)
        while heap:
            f, cost, node = pop(heap)
            if node < 0:
                found = -node - 1  # a target, completed with its lead-in price
                break
            if cost > best[node]:
                continue
            if node in targets:
                if target_extra is None:
                    found = node
                    break
                extra = target_extra.get(node, 0.0)
                push(heap, (cost + extra, cost + extra, -node - 1))
            expansions += 1
            if expansions > limit:
                break
            layer, rest = divmod(node, plane)
            y, x = divmod(rest, nx)
            here_in_region = use_neck and node in region
            # A node legal only at neck width must not be left towards a node
            # outside the region: that step would be drawn at full width.
            here_neck_only = here_in_region and node in neck
            only_targets = layer in target_only
            for dx, dy, dn, base, diagonal, dcell in move_tables[layer]:
                x2 = x + dx
                y2 = y + dy
                if x2 < bx0 or x2 > bx1 or y2 < by0 or y2 > by1:
                    continue
                n2 = node + dn
                if only_targets and n2 not in targets:
                    continue
                if here_neck_only and n2 not in region:
                    continue
                o = owner[n2]
                if o != -1 and o != nid and not (here_in_region and n2 in neck):
                    continue
                if diagonal:
                    cell = node + dcell
                    o = mid_owner[cell]
                    if o != -1 and o != nid and not (here_in_region and n2 in region and cell in neck_mid):
                        continue
                c = cost + base * (1.0 + hist[n2]) * (1.0 + present * occ[n2])
                if c < best[n2]:
                    if best[n2] >= INF:
                        touched.append(n2)
                    best[n2] = c
                    parent[n2] = node
                    hx = tx0 - x2 if x2 < tx0 else (x2 - tx1 if x2 > tx1 else 0)
                    hy = ty0 - y2 if y2 < ty0 else (y2 - ty1 if y2 > ty1 else 0)
                    push(heap, (c + (hx if hx > hy else hy) + 0.41421356 * (hy if hx > hy else hx), c, n2))
            # Layer changes through a through via at this xy.
            o = via_owner[rest]
            if o != -1 and o != nid:
                continue
            vpen = via_cost * (1.0 + via_hist[rest]) * (1.0 + present * via_occ[rest])
            hx = tx0 - x if x < tx0 else (x - tx1 if x > tx1 else 0)
            hy = ty0 - y if y < ty0 else (y - ty1 if y > ty1 else 0)
            h = (hx if hx > hy else hy) + 0.41421356 * (hy if hx > hy else hx)
            for layer2 in layers:
                if layer2 == layer:
                    continue
                n2 = layer2 * plane + rest
                if layer2 in target_only and n2 not in targets:
                    continue
                o = owner[n2]
                if o != -1 and o != nid:
                    continue
                c = cost + vpen * (1.0 + hist[n2]) * (1.0 + present * occ[n2])
                if c < best[n2]:
                    if best[n2] >= INF:
                        touched.append(n2)
                    best[n2] = c
                    parent[n2] = node
                    push(heap, (c + h, c, n2))
        result = None
        if found is not None:
            path = [found]
            while parent[path[-1]] != -1:
                path.append(parent[path[-1]])
            path.reverse()
            result = (path, found)
        for node in touched:
            best[node] = INF
        return result

    def _grow_native_buffers(self) -> bool:
        """Enlarge the kernel's scratch buffers after an overflow (bounded by the grid)."""
        size = self.grid.size
        if self._n_heap_cap >= 4 * size and self._n_touched_cap >= size and self._n_path_cap >= size:
            return False
        self._n_heap_cap = min(4 * size, self._n_heap_cap * 2)
        self._n_heap = np.zeros(self._n_heap_cap * 3, dtype=np.float64)
        self._n_touched_cap = size
        self._n_touched = np.zeros(size, dtype=np.int32)
        self._n_path_cap = min(size, self._n_path_cap * 2)
        self._n_path = np.zeros(self._n_path_cap, dtype=np.int32)
        return True

    def _astar_native(self, spec, sources, targets, present, box, tbox, target_extra):
        """The same search in the C kernel; NotImplemented asks for the Python path."""
        import ctypes
        g = self.grid
        owner, mid_owner = self.rasters.track_arrays(spec.width, spec.clearance)
        via_owner = self.rasters.via_array(spec.via_size, spec.via_drill, spec.clearance)
        occupancy = self.occupancy
        flags, extra = self._n_flags, self._n_extra
        layers = np.asarray([layer | (0x100 if layer in spec.target_only else 0) for layer in spec.layers],
                            dtype=np.int32)
        move_base = self._move_base(spec)
        source_nodes = np.fromiter(sources.keys(), dtype=np.int32, count=len(sources))
        source_costs = np.fromiter(sources.values(), dtype=np.float64, count=len(sources))
        if isinstance(targets, (set, frozenset)) and spec.plane and len(targets) >= len(spec.plane):
            plane_nodes = spec.arrays["plane"]
            others = np.fromiter((t for t in targets if t not in spec.plane), dtype=np.int64)
            target_nodes = np.concatenate((plane_nodes, others)) if others.size else plane_nodes
        else:
            target_nodes = np.fromiter(targets, dtype=np.int64, count=len(targets))
        marks = [(spec.arrays["neck"], 1), (spec.arrays["neck_mid"], 2), (spec.arrays["region"], 4),
                 (target_nodes, 8)]
        for nodes, bit in marks:
            if nodes.size:
                flags[nodes] |= bit
        extra_pointer = None
        if target_extra:
            keys = np.fromiter(target_extra.keys(), dtype=np.int64, count=len(target_extra))
            extra[keys] = np.fromiter(target_extra.values(), dtype=np.float64, count=len(target_extra))
            extra_pointer = extra.ctypes.data
        touched_count = ctypes.c_int(0)
        P = lambda a: a.ctypes.data
        length = self.native(
            g.nx, g.ny, g.nl, P(owner), P(mid_owner), P(via_owner),
            P(occupancy.history), P(occupancy.occ), P(occupancy.via_history), P(occupancy.via_occ),
            P(flags), spec.nid, P(layers), len(layers), P(move_base),
            float(present), float(self.via_cost * spec.via_factor),
            *box, *tbox,
            P(source_nodes), P(source_costs), len(source_nodes),
            extra_pointer, self.options.maximum_expansions,
            P(self._n_best), P(self._n_parent), P(self._n_heap), self._n_heap_cap,
            P(self._n_touched), self._n_touched_cap, ctypes.byref(touched_count),
            P(self._n_path), self._n_path_cap)
        touched = self._n_touched[:touched_count.value]
        self._n_best[touched] = 1e30
        for nodes, bit in marks:
            if nodes.size:
                flags[nodes] = 0
        if target_extra:
            extra[keys] = 0.0
        if length < 0:
            return NotImplemented
        if length == 0:
            return None
        path = self._n_path[:length].tolist()
        return path, path[-1]

    def _move_base(self, spec) -> np.ndarray:
        cache = getattr(self, "_move_cache", None)
        if cache is None:
            g, options = self.grid, self.options
            table = []
            for layer in range(g.nl):
                head = self.heading[layer]
                lc = self.layer_cost[layer]
                for dx, dy, step in _MOVES:
                    base = step * (1.0 + lc)
                    if head is not None and (head == "h" and dy and not dx or head == "n" and dx and not dy):
                        base += options.wrong_way_cost * step
                    table.append(base)
            cache = self._move_cache = np.asarray(table, dtype=np.float64)
        return cache

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
            if any(occ[node] - (1 if node in route.claims else 0) > 0 for node in route.body):
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
            for node, owner in route.body.items():
                if occ[node] - (1 if node in route.claims else 0) > 0:
                    if isinstance(owner, tuple):
                        via_hist[owner[1]] += inc
                    else:
                        hist[owner] += inc

    def negotiate(self, deadline: float | None, only: set[str] | None = None):
        """PathFinder loop. Routes persist on the router between calls."""
        options = self.options
        planned = {name: rank for rank, name in enumerate(options.escape_net_order)}
        specs = sorted(self.specs, key=lambda s: (planned.get(s.name, len(planned)), s.span, s.name))
        routes = self.routes
        present = self.present
        best_snapshot = None
        best_iteration = 0
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
            # Nets that stay conflicted search a wider window: congestion
            # costs alone never widen the box, so a needed detour beyond the
            # margin would otherwise stay invisible.
            for name in pending:
                self.conflict_age[name] = self.conflict_age.get(name, 0) + 1 if name in conflicts else 0
            failed = {name for name, route in routes.items() if not route.connected}
            score = (len(failed) + len(conflicts), sum(r.length for r in routes.values()))
            if best_snapshot is None or score < best_snapshot[0]:
                best_snapshot = (score, dict(routes), set(conflicts))
                best_iteration = iteration
            emit(self.on_progress, "unified_iteration", "finished", iteration=iteration,
                 conflicted=len(conflicts), failed=len(failed))
            if not conflicts:
                break
            if options.stall_iterations and iteration - best_iteration >= options.stall_iterations:
                break
            if deadline is not None and perf_counter() > deadline:
                break
            self._penalise(routes, conflicts)
            present *= options.present_growth
            if options.present_limit:
                present = min(present, options.present_limit)
            self.present = present
            # Reroute conflicted nets; retry failed nets now and then, since
            # congestion elsewhere may have moved.
            pending = [s.name for s in specs if s.name in conflicts or (s.name in failed and iteration in (3, 6))]
            if options.full_reroute_every and iteration % options.full_reroute_every == 0:
                pending = [s.name for s in specs if s.name not in failed or iteration in (3, 6)]
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

    def copper(self, spec: _NetSpec, path: list[int], leads: tuple[TrackSegment, ...]):
        """Tracks and vias of one path, with its lead-ins."""
        g = self.grid
        tracks: list[TrackSegment] = list(leads)
        vias: list[Via] = []
        neck, region = spec.neck, spec.region
        def width_of(a: int, b: int) -> int:
            if a in region and b in region and (a in neck or b in neck or _diagonal_mid(g, a, b) in spec.neck_mid):
                return spec.neck_width
            return spec.width
        if len(path) >= 2:
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
        return tracks, list(dict.fromkeys(vias))

    def commit(self, routes: Mapping[str, _NetRoute], conflicts=frozenset()):
        """Exact check of every path; returns accepted copper, rejections, complete nets."""
        from .surface_path import _track_inside_board
        index = RoutingClearanceIndex(self.board)
        self.rejections: dict[str, list[tuple[Point, str, tuple[str, ...], bool]]] = {}
        accepted: dict[str, tuple[list[TrackSegment], list[Via]]] = {}
        rejected: dict[str, list[Point]] = {}
        complete: set[str] = set()
        by_name = {s.name: s for s in self.specs}
        order = sorted((name for name, route in routes.items() if route.paths),
                       key=lambda name: (name in conflicts, not routes[name].connected,
                                         by_name[name].span, name))
        for name in order:
            spec = by_name[name]
            route = routes[name]
            all_ok = route.connected
            net_tracks: list[TrackSegment] = []
            net_vias: list[Via] = []
            for path, leads in zip(route.paths, route.leads):
                tracks, vias = self.copper(spec, path, leads)
                bad: list[Point] = []
                for track in tracks:
                    if not (index.can_track(name, track.start, track.end, track.width_nm, track.layer)
                            and _track_inside_board(self.board, track.start, track.end, track.width_nm)):
                        where = Point((track.start.x_nm + track.end.x_nm) // 2,
                                      (track.start.y_nm + track.end.y_nm) // 2)
                        bad.append(where)
                        blockers, fixed = index.blocking_track_nets(track)
                        self.rejections.setdefault(name, []).append(
                            (where, track.layer.value, tuple(sorted(blockers)), fixed,
                             _first_reason(index.track_blocker_records(track))))
                for via in vias:
                    if via in net_vias:
                        continue
                    if not index.can_via(name, via.position, via.size_nm, via.from_layer, via.to_layer,
                                         drill_nm=via.drill_nm):
                        bad.append(via.position)
                        blockers, fixed = index.blocking_via_nets(via)
                        self.rejections.setdefault(name, []).append(
                            (via.position, "via", tuple(sorted(blockers)), fixed,
                             _first_reason(index.via_blocker_records(via))))
                if not bad and index.candidate_via_conflict([v for v in vias if v not in net_vias]) is not None:
                    bad.append(vias[0].position)
                if bad:
                    rejected.setdefault(name, []).extend(bad)
                    all_ok = False
                    continue
                for track in tracks:
                    index.add_track(track)
                    net_tracks.append(track)
                for via in vias:
                    if via not in net_vias:
                        index.add_via(via)
                        net_vias.append(via)
            if net_tracks or net_vias:
                accepted[name] = (net_tracks, net_vias)
            if all_ok:
                complete.add(name)
        return accepted, rejected, complete

    def history_seed(self, other: "_Router") -> None:
        """Carry congestion history over from an earlier router on the same grid."""
        if other.grid.size == self.grid.size and other.grid.pitch == self.grid.pitch:
            self.occupancy.copy_history(other.occupancy)
            self.present = other.present
            self.conflict_age = dict(other.conflict_age)

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


def _open_count(board: PhysicalBoard, nets, accepted, plane_nets) -> tuple[int, int]:
    """(open signal connections, incomplete plane nets) with ``accepted`` copper."""
    from .drc import explicit_copper_connectivity
    names = {net.name for net in nets}
    candidate = replace(board,
        tracks=(*board.tracks, *(t for name in sorted(accepted) for t in accepted[name][0])),
        vias=(*board.vias, *(v for name in sorted(accepted) for v in accepted[name][1])))
    graph = explicit_copper_connectivity(candidate, only_nets=frozenset(names))
    opens = 0
    for net in nets:
        if net.name in plane_nets:
            continue
        roots = set()
        isolated = 0
        for pad in net.pads:
            nodes = graph.pad_nodes.get(pad, ())
            if nodes:
                roots.add(graph.roots[nodes[0]])
            else:
                isolated += 1
        opens += max(0, len(roots) + isolated - 1)
    planes = sum(1 for net in nets if net.name in plane_nets and net.name not in accepted)
    return opens, planes


def _with(board: PhysicalBoard, accepted, *, without=frozenset()) -> PhysicalBoard:
    """``board`` plus the accepted copper of every net not in ``without``."""
    names = sorted(name for name in accepted if name not in without)
    return replace(board,
        tracks=(*board.tracks, *(t for name in names for t in accepted[name][0])),
        vias=(*board.vias, *(v for name in names for v in accepted[name][1])))


def _tune_bus_skew(board: PhysicalBoard, accepted: dict, rules: Mapping) -> tuple[dict, list[str]]:
    """Lengthen short bus members so each length-match group meets its ``max_skew``.

    A group is checked once every member has copper (DRC's rule). A member
    shorter than the longest minus ``max_skew`` is lengthened with checked
    bumps or serpentines (``critical_tuning.UnitTuner``) against all other
    copper: to the longest when that fits, else by the least that brings it
    within the limit, never past its own ``max_length``. Like the critical
    router, only a net that declares ``tuning_amplitude_limit`` is tuned;
    every member left over the limit is reported with its reason.
    """
    from .critical_tuning import UnitTuner
    accepted = dict(accepted)
    notes: list[str] = []
    for group in board.match_groups:
        if group.max_skew_nm is None:
            continue
        lengths = {name: _net_track_length(board, name, accepted.get(name, ((), ()))[0]) for name in group.nets}
        if any(not length for length in lengths.values()):
            continue  # a member without copper: the group is not complete yet
        longest = max(lengths.values())
        for name in sorted(group.nets, key=lambda item: (lengths[item], item)):
            need = longest - group.max_skew_nm - lengths[name]
            if need <= 0:
                continue
            rule = rules.get(name)
            if name not in accepted:
                notes.append(f"{group.id}: {name} is {need} nm short; its copper is not the bus router's")
                continue
            if rule is None or rule.tuning_amplitude_limit_nm is None:
                notes.append(f"{group.id}: {name} is {need} nm short; no tuning_amplitude_limit is declared")
                continue
            tracks, vias = accepted[name]
            others = [*(t for t in board.tracks), *(t for other, (items, _) in accepted.items()
                                                    if other != name for t in items)]
            other_vias = [*board.vias, *(v for other, (_, items) in accepted.items() if other != name for v in items)]
            index = RoutingClearanceIndex(replace(board, tracks=tuple(others), vias=(*other_vias, *vias)))
            clearance = max(board.rules.minimum_clearance_nm, rule.clearance_nm or 0)
            tuner = UnitTuner(board, index, tuple(tracks), (name,), 0, rule.tuning_amplitude_limit_nm, clearance,
                              style=rule.tuning_style, leg_gap_nm=rule.tuning_spacing_nm, vias=tuple(vias))
            room = (rule.max_length_nm - lengths[name]) if rule.max_length_nm is not None else None
            full, least = longest - lengths[name], need + need % 2
            targets = [target - target % 2 for target in dict.fromkeys((full, least))
                       if target - target % 2 >= need and (room is None or target - target % 2 <= room)]
            for target in targets:
                plan = tuner.plan(target)
                if plan.tracks is not None:
                    width = rule.width_nm or board.rules.default_track_width_nm
                    accepted[name] = (list(index.breakout.split_tracks(plan.tracks, width)), vias)
                    break
            else:
                notes.append(f"{group.id}: {name} is {need} nm short; "
                             + (plan.reason if targets else "max_length leaves no room"))
    return accepted, notes


def _net_track_length(board: PhysicalBoard, name: str, new_tracks) -> int:
    """Routed length of a net as DRC measures it: its existing and new tracks."""
    return sum(round(hypot(t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm))
               for t in (*(t for t in board.tracks if t.net == name), *new_tracks))


def _smoothed(board: PhysicalBoard, accepted):
    """``accepted`` with its tracks straightened against the complete final board."""
    from .detailed import _smooth_owned
    owned = tuple(t for name in sorted(accepted) for t in accepted[name][0])
    vias = (*board.vias, *(v for name in sorted(accepted) for v in accepted[name][1]))
    by_net: dict[str, list[TrackSegment]] = {name: [] for name in accepted}
    for track in _smooth_owned(board, board.tracks, owned, vias):
        by_net[track.net].append(track)
    return {name: (tuple(by_net[name]), accepted[name][1]) for name in accepted}


def _merge(accepted, more):
    merged = {name: (list(tracks), list(vias)) for name, (tracks, vias) in accepted.items()}
    for name, (tracks, vias) in more.items():
        old_tracks, old_vias = merged.get(name, ([], []))
        merged[name] = (old_tracks + list(tracks), old_vias + list(vias))
    return merged


def _recheck_pours(board, nets, plane_nets, accepted, complete, options, deadline, on_progress):
    """Reconnect plane pads that committed signal copper walled off from their pour.

    Plane pieces are labelled once, from fixed copper, so signal copper
    committed afterwards can wall a plane pad off from the pour piece it was
    counted in. The raster pour estimate names such pads. Two steps, each kept
    only if the estimate improves and no signal connection is lost:

    1. Route the plane nets again against all committed copper (new vias or
       tracks where the pads still have room).
    2. For pads still cut off, rip up the signal copper crowding them, route
       the plane net first with explicit copper for those pads (lying in a
       pour piece does not count), then route the ripped nets around it.
    """
    before = _pour_estimate(board, accepted)
    routed_planes = {net.name for net in nets if net.name in plane_nets}
    cut = {name: pads for name, pads in before.open_pads.items() if name in routed_planes}
    report = {"cut_off": sum(map(len, cut.values())),
              "pads": sorted(p for pads in cut.values() for p in pads)[:12], "steps": [], "iterations": 0}
    if not cut:
        return accepted, complete, report
    signal_before = _open_count(board, nets, accepted, plane_nets)[0]
    plane_list = [net for net in nets if net.name in cut]

    def keep(candidate, estimate, label):
        signal = _open_count(board, nets, candidate, plane_nets)[0]
        ok = estimate.opens < current[1].opens and signal <= signal_before
        report["steps"].append(f"{label}: pour opens {current[1].opens}->{estimate.opens}, "
                               f"signal opens {signal_before}->{signal}" + ("" if ok else " (not kept)"))
        return ok

    current = (accepted, before)
    # Step 1: plane nets only, everything else fixed.
    step = _Router(_with(board, accepted), options, plane_list, on_progress)
    routes, conflicts, used = step.negotiate(deadline)
    report["iterations"] += used
    more, _, _ = step.commit(routes, conflicts)
    candidate = _merge(accepted, more)
    estimate = _pour_estimate(board, candidate)
    if keep(candidate, estimate, "plane reroute"):
        current = (candidate, estimate)
    still = {name: pads for name, pads in current[1].open_pads.items() if name in routed_planes}
    if still and (deadline is None or perf_counter() <= deadline):
        # Step 2: rip up the crowd around the remaining pads; plane copper first.
        pads = frozenset(PadReference(*pad.rsplit(".", 1)) for group in still.values() for pad in group)
        focus = [PhysicalNet(name, tuple(PadReference(*pad.rsplit(".", 1)) for pad in group))
                 for name, group in sorted(still.items())]
        crowd = {name for name in _neighbour_nets(board, focus, current[0], nm_from_mm("1.0"))
                 if name not in plane_nets}
        ripped = {name: value for name, value in current[0].items() if name not in crowd}
        partial = _with(board, ripped)
        first = _Router(partial, options, [net for net in nets if net.name in still], on_progress,
                        explicit_pads=pads)
        routes, conflicts, used = first.negotiate(deadline)
        report["iterations"] += used
        plane_more, plane_rejected, _ = first.commit(routes, conflicts)
        report["explicit_route"] = {
            name: {"copper": [len(part) for part in plane_more.get(name, ((), ()))],
                   "reason": (routes[name].reason if name in routes else "not routed"),
                   "rejected": _rejection_text(first, name) if name in plane_rejected else ""}
            for name in still}
        candidate = _merge(ripped, plane_more)
        second = _Router(_with(board, candidate), options, [net for net in nets if net.name in crowd], on_progress)
        routes, conflicts, used = second.negotiate(deadline)
        report["iterations"] += used
        signal_more, _, signal_complete = second.commit(routes, conflicts)
        candidate = _merge(candidate, signal_more)
        estimate = _pour_estimate(board, candidate)
        if keep(candidate, estimate, f"explicit copper, {len(crowd)} net(s) ripped up"):
            current = (candidate, estimate)
            complete = (complete - crowd) | set(signal_complete)
    report["adopted"] = current[0] is not accepted
    report["still_cut_off"] = sorted(p for name, group in current[1].open_pads.items()
                                     if name in routed_planes for p in group)[:12]
    return current[0], complete, report


def _pour_estimate(board: PhysicalBoard, accepted):
    """Raster pour connectivity of ``board`` with ``accepted`` copper added."""
    from .plane_estimate import estimate_plane_connectivity
    return estimate_plane_connectivity(replace(board,
        tracks=(*board.tracks, *(t for name in sorted(accepted) for t in accepted[name][0])),
        vias=(*board.vias, *(v for name in sorted(accepted) for v in accepted[name][1]))))


def _neighbour_nets(board: PhysicalBoard, nets, accepted, reach: int) -> set[str]:
    """Nets with accepted new copper within ``reach`` of the given nets' pads."""
    from .placement import transformed_local_point
    poses = {p.reference: p for p in board.placements}
    names = {net.name for net in nets}
    points = []
    for net in nets:
        for pad in net.pads:
            pose = poses[pad.component]
            for land in board.footprints[pose.footprint].pads:
                if land.number == pad.pad:
                    points.append(transformed_local_point(pose, land.position))
    result = set()
    for name, (tracks, vias) in accepted.items():
        if name in names:
            continue
        ends = [p for t in tracks for p in (t.start, t.end)] + [v.position for v in vias]
        if any(abs(a.x_nm - b.x_nm) <= reach and abs(a.y_nm - b.y_nm) <= reach for a in ends for b in points):
            result.add(name)
    return result


def _first_reason(records) -> str:
    """"reason: identity" of the first blocker record, or ""."""
    return f"{records[0].reason}: {records[0].identity}" if records else ""


def _rejection_text(router, name: str) -> str:
    """Where the exact commit check rejected a net, and what blocked it."""
    details = getattr(router, "rejections", {}).get(name, [])
    if not details:
        return "unified: rejected by exact commit check"
    where, layer, blockers, fixed, *rest = details[0]
    by = ", ".join(blockers) if blockers else ("fixed copper" if fixed else "board geometry")
    reason = f" [{rest[0]}]" if rest and rest[0] else ""
    more = f" (+{len(details) - 1} more)" if len(details) > 1 else ""
    return (f"unified: rejected by exact commit check: blocked by {by} at "
            f"({where.x_nm / 1e6:.2f},{where.y_nm / 1e6:.2f}) {layer}{reason}{more}")


def choose_pitch(board: PhysicalBoard, default_nm: int = nm_from_mm("0.25")) -> int:
    """0.25 mm, or the coarsest finer pitch that puts nodes on every ball and
    on the channel centre lines of each full-grid ball array (BGA)."""
    from .package_patterns import circular_array
    from .placement import transformed_local_point
    pitch = default_nm
    for pose in board.placements:
        try:
            array = circular_array(board, pose.reference)
        except ValueError:
            array = None
        if array is None:
            continue
        land = board.footprints[pose.footprint].pads[0]
        corner = transformed_local_point(pose, land.position)
        for candidate in (pitch, pitch // 2, pitch // 4, nm_from_mm("0.05")):
            if candidate <= 0:
                continue
            if (array.pitch_nm // 2) % candidate == 0 and corner.x_nm % candidate == 0 \
                    and corner.y_nm % candidate == 0:
                pitch = candidate
                break
    return pitch


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
    escape_seeder=None,
) -> DetailedRoutingResult:
    """Route all ordinary nets with one negotiated-congestion loop.

    ``escape_seeder(board, net_names)`` may return a board with checked pad
    escapes for nets that failed; repair rounds use it before rerouting.

    Signature-compatible with :func:`pcbir.detailed.route_detailed`.
    Existing copper is fixed; zone nets are deferred to fill.
    """

    options = options or UnifiedRouterOptions()
    if options.save_inputs_path:
        # A replayable input state for iterating on the router alone
        # (see replay_unified); a working artifact, not evidence.
        from .state_codec import save_state
        save_state(options.save_inputs_path, {
            "board": board, "global_route": global_route,
            "options": replace(options, save_inputs_path=None),
            "fanout_accesses": fanout_accesses, "fanout_created_vias": fanout_created_vias,
            "fanout_created_tracks": fanout_created_tracks, "only_nets": only_nets,
        }, note="route_unified inputs")
    started = perf_counter()
    deadline = started + options.time_budget_s if options.time_budget_s else None
    if board.hard_macros and set(board.materialized_macros) != {m.cluster for m in board.hard_macros}:
        raise ValueError("materialize hard-macro copper before detailed routing")
    from .zone_geometry import deferred_zone_nets
    rules = {r.net: r for r in board.net_routing_rules}
    zone_nets = deferred_zone_nets(board)
    from .drc import explicit_copper_connectivity
    connectivity = explicit_copper_connectivity(board)
    plane_nets = {zone.net for zone in board.zones}
    nets = [net for net in board.nets
            if (len(net.pads) >= 2 or net.name in plane_nets) and (net.name not in zone_nets or net.name in plane_nets)
            and (only_nets is None or net.name in only_nets)
            and (rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL)
            and not connectivity.net_connected(net)]
    already = [net for net in board.nets
               if len(net.pads) >= 2 and net.name not in zone_nets
               and (only_nets is None or net.name in only_nets)
               and (rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL)
               and connectivity.net_connected(net)]
    original_board, all_nets = board, nets
    bus_accepted: dict = {}
    bus_complete: set[str] = set()
    skew_notes: list[str] = []
    bus_names = {name for group in board.match_groups for name in group.nets} & {net.name for net in nets}
    if options.buses_first and bus_names and len(bus_names) < len(nets):
        # A bus needs a shared corridor; negotiated together with every other
        # net it loses it piecemeal. Its members route first, among themselves.
        bus_options = replace(options, length_priority=True) if options.bus_length_budget else options
        bus_router = _Router(board, bus_options, [net for net in nets if net.name in bus_names], on_progress)
        bus_routes, bus_conflicts, used = bus_router.negotiate(deadline)
        bus_accepted, _, bus_complete = bus_router.commit(bus_routes, bus_conflicts)
        over_length = set()
        if options.bus_length_budget:
            # The DRC measure: every track of the net, existing copper included.
            for name in bus_complete:
                rule = rules.get(name)
                if rule is not None and rule.max_length_nm is not None and _net_track_length(
                        board, name, bus_accepted.get(name, ((), ()))[0]) > rule.max_length_nm:
                    over_length.add(name)
            bus_complete = set(bus_complete) - over_length
        bus_accepted = {name: value for name, value in bus_accepted.items() if name in bus_complete}
        if options.bus_skew_tuning and bus_accepted:
            bus_accepted, skew_notes = _tune_bus_skew(board, bus_accepted, rules)
        emit(on_progress, "unified_buses", "finished", nets=len(bus_names), complete=len(bus_complete),
             over_length=len(over_length), iterations=used, skew_notes=skew_notes)
        if bus_accepted:
            board = _with(board, bus_accepted)
            nets = [net for net in nets if net.name not in bus_complete]
    emit(on_progress, "unified_setup", "started", nets=len(nets))
    router = _Router(board, options, nets, on_progress)
    emit(on_progress, "unified_setup", "finished", nets=len(router.specs),
         grid=[router.grid.nx, router.grid.ny, router.grid.nl], seconds=round(perf_counter() - started, 2))
    routes, conflicts, iterations = router.negotiate(deadline)
    accepted, rejected, complete = router.commit(routes, conflicts)
    emit(on_progress, "unified_commit", "finished", round=1,
         accepted=len(accepted), rejected=len(rejected), conflicted=len(conflicts))
    # Rip-in repair: committed copper becomes a fixed obstacle and the
    # remaining nets are routed again against it. This settles the residual
    # cases where the raster and the exact check disagree (off-grid lead-ins).
    def repair_attempt(seed: bool):
        """One rip-in repair; returns the new (accepted, complete, rejected, routes, stats)."""
        work = {k: (list(v[0]), list(v[1])) for k, v in accepted.items()}
        done = set(complete)
        remaining = [net for net in nets if net.name not in done]
        # Rip up neighbours whose new copper crowds the remaining nets' pads,
        # so they renegotiate together; everything else stays fixed.
        crowd = _neighbour_nets(board, remaining, work, nm_from_mm("1.0"))
        # The exact check names what blocked a rejected path: rip those too.
        remaining_names = {net.name for net in remaining}
        crowd |= {blocker for name in remaining_names
                  for _, _, blockers, *_ in router.rejections.get(name, ())
                  for blocker in blockers if blocker in work and blocker not in plane_nets}
        for name in crowd:
            work.pop(name, None)
            done.discard(name)
        remaining = [net for net in nets if net.name not in done]
        partial = replace(board,
            tracks=(*board.tracks, *(t for name in sorted(work) for t in work[name][0])),
            vias=(*board.vias, *(v for name in sorted(work) for v in work[name][1])))
        if seed:
            failed_signal = frozenset(net.name for net in remaining
                                      if net.name not in plane_nets and net.name not in crowd)
            if not failed_signal:
                return None
            seeded = escape_seeder(partial, failed_signal)
            from collections import Counter
            for track in (Counter(seeded.tracks) - Counter(partial.tracks)).elements():
                work.setdefault(track.net, ([], []))[0].append(track)
            for via in (Counter(seeded.vias) - Counter(partial.vias)).elements():
                work.setdefault(via.net, ([], []))[1].append(via)
            partial = seeded
        fixed = explicit_copper_connectivity(partial)
        remaining = [net for net in remaining if not fixed.net_connected(net) or net.name in plane_nets]
        repair = _Router(partial, options, remaining, on_progress)
        repair.history_seed(router)
        more_routes, more_conflicts, used = repair.negotiate(deadline)
        more, more_rejected, more_complete = repair.commit(more_routes, more_conflicts)
        for name, (tracks, vias) in more.items():
            old_tracks, old_vias = work.get(name, ([], []))
            work[name] = (old_tracks + tracks, old_vias + vias)
        done |= more_complete
        done |= {net.name for net in remaining if net.name not in more_routes
                 and fixed.net_connected(net) and net.name not in plane_nets}
        still = {name: points for name, points in more_rejected.items() if name not in done}
        return work, done, still, more_routes, (len(more), len(more_rejected), len(more_conflicts), used), \
            repair.rejections

    for round_index in range(1, options.commit_rounds):
        if all(net.name in complete for net in nets) or (deadline is not None and perf_counter() > deadline):
            break
        baseline = _open_count(board, nets, accepted, plane_nets)
        best_attempt = None
        for seed in ((False, True) if escape_seeder is not None else (False,)):
            attempt = repair_attempt(seed)
            if attempt is None:
                continue
            iterations += attempt[4][3]
            score = _open_count(board, nets, attempt[0], plane_nets)
            if score < baseline and (best_attempt is None or score < best_attempt[0]):
                best_attempt = (score, attempt, seed)
            if best_attempt is not None and best_attempt[0][0] == 0:
                break
        improved = best_attempt is not None
        if improved:
            _, (accepted, complete, rejected, more_routes, stats, rejections), seeded = best_attempt
            router.rejections = {**router.rejections, **rejections}
            for name, route in more_routes.items():
                router.routes[name] = route
        emit(on_progress, "unified_commit", "finished", round=round_index + 1, adopted=improved,
             seeded=bool(improved and seeded))
        if not improved:
            break
    if options.pour_recheck and plane_nets and (deadline is None or perf_counter() <= deadline):
        accepted, complete, recheck = _recheck_pours(
            board, nets, plane_nets, accepted, set(complete), options, deadline, on_progress)
        iterations += recheck.pop("iterations", 0)
        emit(on_progress, "unified_pour_recheck", "finished", **recheck)
    if bus_accepted:
        # Report bus copper as this call's own routing, on the original board.
        accepted = _merge(bus_accepted, accepted)
        complete = set(complete) | bus_complete
        board, nets = original_board, all_nets
    spec_names = {s.name for s in router.specs} | bus_complete
    if options.route_smoothing and accepted:
        accepted = _smoothed(board, accepted)
    new_tracks = tuple(t for name in sorted(accepted) for t in accepted[name][0])
    new_vias = tuple(v for name in sorted(accepted) for v in accepted[name][1])
    all_tracks = (*board.tracks, *new_tracks)
    all_vias = (*board.vias, *new_vias)
    metadata = dict(board.metadata)
    routed_names = set(complete) | {net.name for net in already}
    unrouted = [net.name for net in nets if net.name not in complete]
    success = not unrouted
    metadata.update({"detailed_routing": "complete" if success else "partial",
                     "detailed_router": "unified",
                     "global_routing_fingerprint": global_route.routing_fingerprint,
                     "fabrication_ready": "false"})
    if skew_notes:
        # Bus members bus skew tuning left over max_skew, with the reason (DRC reports the skew itself).
        metadata["bus_skew"] = " | ".join(skew_notes)
    routed = replace(board, tracks=tuple(all_tracks), vias=tuple(all_vias),
                     metadata=MappingProxyType(metadata))
    results = []
    for net in [*nets, *already]:
        tracks, vias = accepted.get(net.name, ((), ()))
        diagnostics = ()
        if net.name not in routed_names:
            last = router.routes.get(net.name)
            diagnostics = (_rejection_text(router, net.name),) if net.name in rejected else (
                ("unified: no routable layer",) if net.name not in spec_names else
                (f"unified: {last.reason}" if last is not None and last.reason else
                 "unified: unresolved congestion",))
        results.append(DetailedNetResult(
            net.name, net.name in routed_names, len(tracks), len(vias),
            int(sum(hypot(t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm) for t in tracks)),
            0, diagnostics))
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


def replay_unified(path, on_progress: ProgressCallback | None = None, *,
                   only_nets: frozenset[str] | None = None, **overrides) -> DetailedRoutingResult:
    """Route the inputs saved by ``save_inputs_path`` again, with option overrides.

    ``only_nets`` narrows the saved net selection (to study a net alone).
    Without the pipeline's escape seeder (repair rounds run unseeded).
    """
    from .state_codec import load_state
    saved = load_state(path)
    options = replace(saved["options"], **overrides)
    if only_nets is not None:
        saved["only_nets"] = only_nets if saved["only_nets"] is None else saved["only_nets"] & only_nets
    return route_unified(saved["board"], saved["global_route"], options,
                         fanout_accesses=saved["fanout_accesses"],
                         fanout_created_vias=saved["fanout_created_vias"],
                         fanout_created_tracks=saved["fanout_created_tracks"],
                         only_nets=saved["only_nets"], on_progress=on_progress)
