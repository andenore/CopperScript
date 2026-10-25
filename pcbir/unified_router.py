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
from operator import itemgetter
from math import ceil, hypot
from time import perf_counter
from types import MappingProxyType
from typing import TYPE_CHECKING, Iterable, Mapping

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
from .routing_layers import routing_layers, signal_layer_preferences
from .routing_lattice import MOVES as _MOVES
from .routing_lattice import NestedLattice, UniformLattice
from .routing_vias import physical_via_span

if TYPE_CHECKING:
    from .fine_regions import FineRegionOptions, FineRegionPlan

_FOREIGN = -2  # Obstacle owned by another net, by several nets or by none.
_FREE = -1


# UnifiedRouterOptions.fine_regions values ("auto" is an alias of "dense").
FINE_REGION_MODES = ("off", "arrays", "dense")
# Base pitch of the lattice when pitch_nm is None (and choose_pitch's default).
DEFAULT_PITCH_NM = nm_from_mm("0.25")


@dataclass(frozen=True, slots=True)
class UnifiedRouterOptions:
    pitch_nm: int | None = None    # None: 0.25 mm (the base pitch; with fine_regions="off" finer where a ball grid needs it)
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
    # Soft deadline: setup, exact final checks and an in-flight bounded native
    # search may finish. Expiry prevents starting another net/search/iteration.
    time_budget_s: float | None = None
    # Optional diagnostic raster/search window, not a replacement outline or
    # copper keepout. All source geometry and final exact checks remain whole.
    grid_window_nm: tuple[int, int, int, int] | None = None
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
    # One negotiation from source-fixed copper; no legacy critical/bypass bootstrap.
    direct: bool = False
    # Escape crowded ordinary pins (fanout.route_fanout) before critical routing.
    escapes_first: bool = True
    # commit/via options
    # One exact clearance index per route_unified call: commits add accepted
    # copper to it, repair rounds and the pour re-check rip nets out of it
    # (RoutingClearanceIndex.remove) instead of building one per router.
    # False builds a fresh index for every router (former behaviour; tests).
    share_commit_index: bool = True

    # -- Failed-net retries during negotiation ------------------------------
    # Conflicted nets reroute every iteration; nets that failed outright are
    # retried after every this many iterations (0: never), since congestion
    # elsewhere may have moved. The limit caps how many of these retry points
    # one negotiation uses (0: no cap). The former schedule (after iterations
    # 3 and 6 only, no exit retry) is every=3, limit=2, before_exit=False.
    failed_retry_every: int = 3
    failed_retry_limit: int = 0
    # When conflicts clear while nets are still failed and none has been
    # retried yet, run one extra iteration that retries them before stopping.
    failed_retry_before_exit: bool = True

    # -- Terminal access tuning ---------------------------------------------
    # A land farther than this from its package centre gets an outward axis
    # (fine-pitch lead-ins along the pin, see _attach_extended).
    outward_axis_threshold_nm: int = nm_from_mm("0.3")
    # Straight stems tried along the outward axis, shortest first, and how far
    # from the stem's end a grid node may be for the checked lead-in.
    attach_extension_steps_nm: tuple[int, ...] = tuple(
        nm_from_mm(step) for step in ("0.3", "0.5", "0.75", "1.0", "1.5"))
    attach_extension_reach_nm: int = nm_from_mm("0.6")
    # Exact local maze for trapped pins (pin_access.local_access_path).
    local_escape_detour_nm: int = nm_from_mm("0.5")
    local_escape_states: int = 20000
    # Cells a pin's flood may visit before it counts as reaching open space.
    open_flood_limit: int = 600
    # Free region at least this large (nm^2; default (2 mm)^2) is open space.
    open_space_area_nm2: int = nm_from_mm("2") ** 2

    # -- Plane pieces (_zone_regions) ---------------------------------------
    # Fill pieces smaller than this (nm^2; default (0.8 mm)^2) are ignored,
    # except a layer's largest piece.
    plane_piece_area_nm2: int = nm_from_mm("0.8") ** 2
    # A layer's largest piece is plane copper when it is at least this
    # fraction of the overall largest piece; smaller ones are islands to join.
    plane_piece_fraction: float = 0.25

    # -- Repair --------------------------------------------------------------
    # Nets with new copper this close to a remaining net's pads are ripped up
    # with it (repair rounds and the pour re-check).
    repair_neighbour_reach_nm: int = nm_from_mm("1.0")

    # -- Via budgets ---------------------------------------------------------
    # Via cost multiplier for nets with a max_vias rule, and the extra factor
    # of the re-search when a route exceeds the budget.
    via_budget_factor: float = 4.0
    via_overrun_factor: float = 10.0

    # -- Search cost terms (plan section 3, "Costs") -------------------------
    # Turn cost per 45 degrees of heading change, in pitch steps (as via_cost
    # is after its mm conversion); a 90-degree turn pays twice this. The
    # search state stays one node: the incoming heading is that of the
    # node's best parent (parent[node]), not a separate per-heading state.
    # This is not exact (a node keeps only its cheapest arrival, so a
    # slightly dearer arrival with a better heading is dropped), but it keeps
    # memory, the expansion budget and the native kernel's buffers as they
    # are. Arrival by via and source nodes have no heading (no turn charged).
    # 0 leaves the search byte-identical to having no turn term. Default 0.25
    # (owner decision 2026-10-24): on cm4, nrf52 and full-vertical it cuts
    # bends 27-40 % with equal completion and DRC and slightly shorter
    # routes; round-led-ring pays a few extra vias.
    bend_cost: float = 0.25
    # Soft cost per pitch step whose target node lies outside the net's
    # connected global-route corridor (guide segments on their layer within
    # guide_half_width_nm + guide_margin_nm; pin accesses within the margin
    # square). Nets without a connected guide pay nothing. Never a restriction
    # and never part of the legality rasters. 0 disables (byte-identical).
    guide_cost: float = 0.0
    guide_margin_nm: int = nm_from_mm("1")

    # -- Congestion export (congestion-driven placement feedback) -----------
    # After the final commit, bin the main negotiation's contention (claim
    # overflow plus history) and the free track capacity into square tiles
    # of this size on a lattice anchored at the grid origin, and attach the
    # map as DetailedRoutingResult.congestion (see _congestion_map). Telemetry
    # only: it never changes copper. The tile must be at least one pitch.
    congestion_tile_nm: int = nm_from_mm("1")
    export_congestion: bool = True

    # -- Multi-resolution lattice (fine_regions.py, routing_lattice.py) -----
    # "off": one uniform lattice at pitch_nm (or choose_pitch). "arrays"
    # (default) and "dense": a pitch_nm (or 0.25 mm) base lattice plus fine
    # patches planned by plan_fine_regions (the choose_pitch rule applied
    # per package): "arrays" refines full ball arrays only; "dense" also
    # refines non-array dense packages (QFN, fine-pitch QFP), a measurement
    # mode. A plan without patches gives the plain uniform base lattice,
    # which on a board without a full ball array is choose_pitch's lattice.
    # "auto" (the S2/S3 name) is accepted and stored as "dense".
    # fine_region_options sets the planner's other knobs; the mode always
    # decides dense_packages.
    fine_regions: str = "arrays"
    fine_region_options: "FineRegionOptions | None" = None   # None: FineRegionOptions()

    def __post_init__(self) -> None:
        if (self.pitch_nm is not None and self.pitch_nm <= 0) or self.maximum_iterations <= 0 or self.maximum_expansions <= 0:
            raise ValueError("unified router pitch, iterations and search states must be positive")
        if self.stall_iterations < 0:
            raise ValueError("unified router stall iterations must not be negative")
        if self.failed_retry_every < 0 or self.failed_retry_limit < 0:
            raise ValueError("unified router failed-net retry interval and limit must not be negative")
        steps = self.attach_extension_steps_nm
        if (not isinstance(steps, (tuple, list)) or not steps
                or any(type(step) is not int or step <= 0 for step in steps)):
            raise ValueError("attach_extension_steps_nm requires positive integer nanometre steps")
        object.__setattr__(self, "attach_extension_steps_nm", tuple(steps))
        if (self.outward_axis_threshold_nm < 0 or self.attach_extension_reach_nm < 0
                or self.local_escape_detour_nm < 0 or self.repair_neighbour_reach_nm < 0
                or self.open_space_area_nm2 < 0 or self.plane_piece_area_nm2 < 0):
            raise ValueError("unified router distances and areas must not be negative")
        if self.local_escape_states <= 0 or self.open_flood_limit <= 0:
            raise ValueError("unified router local escape states and open flood limit must be positive")
        if not 0 < self.plane_piece_fraction <= 1:
            raise ValueError("plane_piece_fraction must be in (0, 1]")
        if self.via_budget_factor <= 0 or self.via_overrun_factor < 1:
            raise ValueError("via_budget_factor must be positive and via_overrun_factor at least 1")
        if not (self.bend_cost >= 0 and self.guide_cost >= 0) or self.guide_margin_nm < 0:
            raise ValueError("bend_cost, guide_cost and guide_margin_nm must not be negative")
        if self.grid_window_nm is not None:
            bounds = self.grid_window_nm
            if (not isinstance(bounds, (tuple, list)) or len(bounds) != 4
                    or any(type(value) is not int for value in bounds)
                    or bounds[0] >= bounds[2] or bounds[1] >= bounds[3]):
                raise ValueError("grid_window_nm requires four ordered integer nanometre coordinates")
            object.__setattr__(self, "grid_window_nm", tuple(bounds))
        if type(self.congestion_tile_nm) is not int or self.congestion_tile_nm <= 0 or (
                self.pitch_nm is not None and self.congestion_tile_nm < self.pitch_nm):
            raise ValueError("congestion_tile_nm must be a positive integer of at least one pitch")
        if self.fine_regions == "auto":
            object.__setattr__(self, "fine_regions", "dense")
        if self.fine_regions not in FINE_REGION_MODES:
            raise ValueError('fine_regions must be "off", "arrays" or "dense"')
        if self.fine_region_options is not None:
            from .fine_regions import FineRegionOptions
            if not isinstance(self.fine_region_options, FineRegionOptions):
                raise ValueError("fine_region_options must be a FineRegionOptions or None")


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


class _BlockRaster:
    """Node and cell-centre coordinates and board material of one lattice block."""

    __slots__ = ("block", "margin", "px", "py", "mx", "my",
                 "inside_nodes", "inside_cells", "edge_nodes", "edge_cells")


class _Rasters:
    """Lazily built legality rasters keyed by width/clearance class.

    Built block by block (``lattice.blocks``; one block on a uniform
    lattice), each dense ``(nl, ny, nx)`` with its own pitch and raster
    margin, then concatenated into the lattice's flat node (and cell, and
    column) id space. Disabled (shadowed) base nodes and cells of a nested
    lattice are foreign in every raster.
    """

    def __init__(self, board: PhysicalBoard, grid, obstacles: list[_Obstacle],
                 holes: list[tuple[int, float, float, float]], margin) -> None:
        self.board = board
        self.lattice = grid
        self.obstacles = obstacles
        self.holes = holes          # (owner, x, y, radius)
        margins = list(margin) if isinstance(margin, (tuple, list)) else [margin] * len(grid.blocks)
        self.margin = margins[0]
        self._track: dict[tuple[int, int], tuple[list[int], list[int]]] = {}
        self._via: dict[tuple[int, int, int, tuple[int, int]], tuple] = {}
        # False forbids a new via there: (ny, nx) on a uniform lattice, one
        # entry per column on a nested one.
        self.via_mask = None
        outline = board.outline.vertices
        cutouts = [c.vertices for c in board.outline.cutouts]
        self.views: list[_BlockRaster] = []
        for block, block_margin in zip(grid.blocks, margins):
            view = _BlockRaster()
            view.block, view.margin = block, block_margin
            view.px, view.py = np.meshgrid(block.xs, block.ys)                              # node centres
            view.mx, view.my = np.meshgrid(block.xs + block.pitch / 2, block.ys + block.pitch / 2)  # cell centres
            view.inside_nodes = self._material(outline, cutouts, view.px, view.py)
            view.inside_cells = self._material(outline, cutouts, view.mx, view.my)
            view.edge_nodes = self._edge_distance(outline, cutouts, view.px, view.py)
            view.edge_cells = self._edge_distance(outline, cutouts, view.mx, view.my)
            self.views.append(view)
        first = self.views[0]
        self.px, self.py, self.mx, self.my = first.px, first.py, first.mx, first.my
        self.inside_nodes, self.inside_cells = first.inside_nodes, first.inside_cells
        self.edge_nodes, self.edge_cells = first.edge_nodes, first.edge_cells
        self.nested = isinstance(grid, NestedLattice)

    def _flatten(self, parts) -> np.ndarray:
        """Per-block ``(nl, ny, nx)`` owner arrays as one flat node-id array."""
        if len(parts) == 1:
            return parts[0].ravel()
        nl = self.lattice.nl
        return np.concatenate([part.reshape(nl, -1) for part in parts], axis=1).ravel()

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
            nodes = np.ascontiguousarray(self._flatten(self._build(width / 2, clearance, cells=False,
                                                                   for_via=False)))
            cells = np.ascontiguousarray(self._flatten(self._build(width / 2, clearance, cells=True,
                                                                   for_via=False)))
            if self.nested:
                g = self.lattice
                nodes.reshape(g.nl, g.plane)[:, g.shadowed] = _FOREIGN
                cells.reshape(g.nl, g.plane)[:, g.shadowed_cells] = _FOREIGN
            self._track[key] = (nodes.tolist(), cells.tolist(), nodes, cells)
        return self._track[key]

    def via(self, size: int, drill: int, clearance: int, span=None) -> list[int]:
        """Owner list for a physical via span; drill spacing stays layer independent."""
        span = span if span is not None else (0, self.lattice.nl - 1)
        key = (size, drill, clearance, span)
        if key not in self._via:
            g = self.lattice
            # Copper spacing, or the drill's hole clearance to copper if larger.
            half = max(size / 2, drill / 2 + self.board.rules.minimum_hole_clearance_nm - clearance)
            low, high = span
            hole_clear = self.board.rules.minimum_hole_clearance_nm
            owners = []
            for view, stacked in zip(self.views, self._build(half, clearance, cells=False, for_via=True,
                                                             via_annulus_half_width=size / 2)):
                owner = stacked[low].copy()
                for layer in range(low + 1, high + 1):
                    other = stacked[layer]
                    both = (owner != other)
                    owner = np.where(both & (owner == _FREE), other, owner)
                    owner = np.where(both & (other != _FREE) & (owner != other), _FOREIGN, owner)
                for _, x, y, radius in self.holes:
                    reach = drill / 2 + radius + hole_clear + view.margin
                    ix0, iy0, ix1, iy1 = view.block.window(x - reach, y - reach, x + reach, y + reach)
                    if ix0 > ix1 or iy0 > iy1:
                        continue
                    win = np.hypot(view.px[iy0:iy1 + 1, ix0:ix1 + 1] - x,
                                   view.py[iy0:iy1 + 1, ix0:ix1 + 1] - y) < reach
                    owner[iy0:iy1 + 1, ix0:ix1 + 1][win] = _FOREIGN
                owners.append(owner)
            if len(owners) == 1:
                owner = owners[0]
                if self.via_mask is not None:
                    owner = np.where(self.via_mask, owner, _FOREIGN)
                flat = np.ascontiguousarray(owner.ravel().astype(np.int32))
            else:
                owner = np.concatenate([part.ravel() for part in owners])
                if self.via_mask is not None:
                    owner = np.where(self.via_mask, owner, _FOREIGN)
                owner[g.shadowed] = _FOREIGN
                flat = np.ascontiguousarray(owner.astype(np.int32))
            self._via[key] = (flat.tolist(), flat)
        return self._via[key][0]

    def via_array(self, size: int, drill: int, clearance: int, span=None):
        span = span if span is not None else (0, self.lattice.nl - 1)
        self.via(size, drill, clearance, span)
        return self._via[(size, drill, clearance, span)][1]

    def _build(self, half_width: float, clearance: int, *, cells: bool, for_via: bool,
               via_annulus_half_width=None) -> list[np.ndarray]:
        """Owner arrays ``(nl, ny, nx)`` per block, at node or cell centres."""
        return [self._build_block(view, half_width, clearance, cells=cells, for_via=for_via,
                                  via_annulus_half_width=via_annulus_half_width) for view in self.views]

    def _build_block(self, view: _BlockRaster, half_width: float, clearance: int, *, cells: bool,
                     for_via: bool, via_annulus_half_width=None) -> np.ndarray:
        g, block, margin = self.lattice, view.block, view.margin
        px, py = (view.mx, view.my) if cells else (view.px, view.py)
        inside, edge = (view.inside_cells, view.edge_cells) if cells else (view.inside_nodes, view.edge_nodes)
        edge_clear = self.board.rules.minimum_clearance_nm
        legal_material = inside & (edge >= half_width + edge_clear + margin)
        owner = np.full((g.nl, block.ny, block.nx), _FREE, dtype=np.int32)
        for layer in range(g.nl):
            owner[layer][~legal_material] = _FOREIGN
        for item in self.obstacles:
            if for_via and not item.blocks_vias or not for_via and not item.blocks_tracks:
                continue
            gap = max(clearance, item.clearance)
            owner_id = item.owner
            if for_via and item.is_pad:
                gap = max(gap, 1)
            reach = item.radius + gap + half_width + margin
            xs = [p.x_nm for p in item.spine]
            ys = [p.y_nm for p in item.spine]
            ix0, iy0, ix1, iy1 = block.window(min(xs) - reach, min(ys) - reach,
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
            pad_contact = None
            if for_via and item.is_pad:
                # Exact can_via excludes the full annulus from every pad,
                # but ordinary copper/drill clearance applies only to foreign
                # nets. Keep the electrical halo net-aware and its inner
                # annulus exclusion foreign to everyone, on every layer.
                annulus = half_width if via_annulus_half_width is None else via_annulus_half_width
                pad_contact = distance < item.radius + annulus + 1 + margin
            for layer in item.layers:
                window = owner[layer, iy0:iy1 + 1, ix0:ix1 + 1]
                if owner_id == _FOREIGN:
                    window[hit] = _FOREIGN
                    continue
                free = hit & (window == _FREE)
                clash = hit & (window != _FREE) & (window != owner_id)
                window[free] = owner_id
                window[clash] = _FOREIGN
                if pad_contact is not None:
                    window[pad_contact] = _FOREIGN
        return owner


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
    joined: dict = field(default_factory=dict)  # per component: grid nodes of its own vias
    explicit: dict = field(default_factory=dict)  # component index -> layers its plane path may not end on
    # Per pad-group component (not plane pieces): the anchor positions its
    # attach candidates were sought from (failed-point export).
    anchors: list = field(default_factory=list)


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
                 nets: list, on_progress, *, explicit_pads: frozenset[PadReference] = frozenset(),
                 required_accesses=None, guides: GlobalRoutingResult | None = None,
                 clearance: RoutingClearanceIndex | None = None) -> None:
        self.board = board
        # Plane pads that must reach the plane with copper of their own, not
        # by lying in a pour piece (pour re-check, see route_unified).
        self.explicit_pads = frozenset(explicit_pads)
        self.options = options
        self.deadline: float | None = None
        self.on_progress = on_progress
        # Lazy state, filled while specs are built and during search:
        # open-space masks per (width, clearance) raster, the move-cost
        # table, and how many exact local mazes were spent (local_escape_limit).
        self._open_cache: dict[tuple[int, int], np.ndarray] = {}
        self._move_cache: np.ndarray | None = None
        self._local_spent = 0
        # Search cost terms (bend_cost, guide_cost): the turn table and, per
        # net, the global-route corridor nodes (None: no connected guide).
        self._turn_cache: np.ndarray | None = None
        self._guide_cache: dict[str, tuple | None] = {}
        self.guides: dict[str, tuple] = {}
        if guides is not None and options.guide_cost:
            for route in guides.routes:
                if route.connected and (route.segments or route.accesses):
                    self.guides[route.net] = (*self.guides.get(route.net, ()), route)
        # The fine-region plan (fine_regions "arrays" or "dense"), else None.
        # With a plan, its per-package rule replaces choose_pitch and the
        # base pitch is pitch_nm or 0.25 mm.
        self.fine_plan = fine_region_plan(board, options)
        if self.fine_plan is None:
            pitch = options.pitch_nm or choose_pitch(board)
        else:
            pitch = self.fine_plan.base_pitch_nm
        # ``pitch`` is the base pitch: search costs and route lengths are per
        # base pitch on every lattice.
        self.pitch = pitch
        self.via_cost = options.via_cost_mm * 1_000_000 / pitch
        # Every node id, point/node conversion, offset and window goes
        # through this lattice (routing_lattice.Lattice). ``grid`` is the
        # same object under its former name (tests, tools).
        if self.fine_plan is not None and self.fine_plan.patches:
            if options.grid_window_nm is not None:
                raise ValueError("grid_window_nm is not supported with fine_regions patches "
                                 f'({", ".join(r for p in self.fine_plan.patches for r in p.references)}); '
                                 'set fine_regions="off" for a windowed search')
            self.lattice = self.grid = NestedLattice(board, pitch, self.fine_plan)
        else:
            # No patches: the plain uniform lattice. Without a full ball array
            # choose_pitch(board) is 0.25 mm too, so the copper is exactly
            # fine_regions="off"'s.
            self.lattice = self.grid = UniformLattice(board, pitch, options.grid_window_nm)
        self.nested = isinstance(self.lattice, NestedLattice)
        if options.grid_window_nm is not None:
            from .placement import transformed_local_point
            x0, y0, x1, y1 = options.grid_window_nm
            poses = {pose.reference: pose for pose in board.placements}
            for net in nets:
                for ref in net.pads:
                    pose = poses[ref.component]
                    for land in board.footprints[pose.footprint].pads:
                        if land.number != ref.pad or land.kind in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}:
                            continue
                        point = transformed_local_point(pose, land.position)
                        if not (x0 <= point.x_nm <= x1 and y0 <= point.y_nm <= y1):
                            raise ValueError(f"terminal {ref.component}.{ref.pad} is outside grid_window_nm")
        self.rules: dict[str, NetRoutingRule] = {r.net: r for r in board.net_routing_rules}
        self.zone_nets = {zone.net for zone in board.zones}
        # ``clearance`` may be a shared index (route_unified) that holds
        # exactly this board's copper; commit adds accepted copper to it.
        if clearance is not None and not _index_holds(clearance, board):
            raise ValueError("shared clearance index does not hold this router's board copper")
        self.clearance = clearance if clearance is not None else RoutingClearanceIndex(board)
        # Physical via spans by (low, high) layer index; board vias by (net, xy).
        self._via_span_cache: dict[tuple[int, int], tuple | None] = {}
        self._board_via_sites: dict[tuple[str, Point], list[Via]] | None = None
        self.net_ids: dict[str, int] = {}
        for net in board.nets:
            self.net_ids.setdefault(net.name, len(self.net_ids))
        obstacles, holes = self._obstacles()
        margin = options.raster_margin_nm if options.raster_margin_nm is not None else pitch // 8
        if self.nested and options.raster_margin_nm is None:
            margin = tuple(block.pitch // 8 for block in self.lattice.blocks)
        self.rasters = _Rasters(board, self.lattice, obstacles, holes, margin)
        if options.ball_field_via_sites:
            self.rasters.via_mask = self._ball_field_via_mask()
        from .native import astar as native_astar
        # The C kernel serves both lattices (a nested one through its block
        # tables); _astar falls back to the Python search when it is absent.
        self.native = native_astar()
        self._nested_tables = None
        self.occupancy = _Occupancy(self.lattice.size, self.lattice.plane, arrays=self.native is not None)
        from array import array
        self._best = array("d", [1e30]) * self.lattice.size
        self._parent = array("i", [-1]) * self.lattice.size
        if self.native is not None:
            size = self.lattice.size
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
            # Per-node guide penalty for the kernel: guide_cost outside the
            # searched net's corridor, 0 inside (set and restored per search;
            # allocated on the first guided search).
            self._n_guide: np.ndarray | None = None
        ranks, headings = signal_layer_preferences(board)
        g = self.lattice
        self.layer_cost = [options.outer_layer_cost if i in (0, g.nl - 1) else 0.0 for i in range(g.nl)]
        self.heading = [headings.get(layer) for layer in g.layers]
        default_width = board.rules.default_track_width_nm
        self.default_width = default_width
        # (layer, columns, cost) of seeded ball-field history: a routing
        # preference, not contention (_congestion_map).
        self._seeded_history: list[tuple[int, np.ndarray, float]] = []
        self._seed_ball_field_cost()
        # A local transaction may own the area copper but not the launch vias
        # or collar witnesses. Such pads attach only at fresh verified ports.
        from .pin_escape import RoutingAccess, verified_routing_access
        self.required_accesses = {}
        net_by_pad = {pad: net.name for net in board.nets for pad in net.pads}
        selected = {net.name for net in nets}
        for pad, access in (required_accesses or {}).items():
            name = net_by_pad.get(pad)
            if name is None:
                raise ValueError(f"required routing access has no net: {pad}")
            if name not in selected:
                continue
            if (not isinstance(access, RoutingAccess)
                    or verified_routing_access(board, pad, name, access, self.clearance) is None):
                raise ValueError(f"required routing access is stale or invalid: {pad}")
            self.required_accesses[pad] = access
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
        g = self.lattice
        hist = self.occupancy.history
        for pose in self.board.placements:
            try:
                array = circular_array(self.board, pose.reference)
            except ValueError:
                array = None
            if array is None:
                continue
            corners = [transformed_local_point(pose, p.position) for p in self.board.footprints[pose.footprint].pads]
            bounds = (min(p.x_nm for p in corners), min(p.y_nm for p in corners),
                      max(p.x_nm for p in corners), max(p.y_nm for p in corners))
            layer = g.layers.index(CopperLayer.FRONT if pose.side.value == "front" else CopperLayer.BACK)
            # Per block, the covering window's live columns (recorded for _congestion_map).
            for block in g.blocks:
                columns = block.window_columns(block.window(*bounds))
                if not columns.size:
                    continue
                self._seeded_history.append((layer, columns, cost))
                for node in (columns + layer * g.plane).tolist():
                    hist[node] += cost

    def _ball_field_via_mask(self):
        """Inside a full ball array, new vias only at cell centres (dogbone sites).

        Reference BGA layouts keep a regular via lattice so the inner layers
        get straight channels between via rows; a via dropped anywhere else in
        the field closes channels for everyone (the FX10 kit, for example,
        fans out every inner ball to its cell centre).
        """
        from .package_patterns import circular_array
        from .placement import transformed_local_point
        g = self.lattice
        masks = None
        for pose in self.board.placements:
            try:
                array = circular_array(self.board, pose.reference)
            except ValueError:
                array = None
            if array is None:
                continue
            if masks is None:
                masks = [np.ones((block.ny, block.nx), dtype=bool) for block in g.blocks]
            pads = self.board.footprints[pose.footprint].pads
            points = [transformed_local_point(pose, p.position) for p in pads]
            x0 = min(p.x_nm for p in points); x1 = max(p.x_nm for p in points)
            y0 = min(p.y_nm for p in points); y1 = max(p.y_nm for p in points)
            # Cell centres: half a pitch off every ball row and column.
            def centred(values, origin, pitch):
                cells = (values - origin) / array.pitch_nm - 0.5
                return np.abs(cells - np.round(cells)) * array.pitch_nm <= pitch / 4
            for block, mask in zip(g.blocks, masks):
                ix0, iy0, ix1, iy1 = block.window(x0, y0, x1, y1)
                if ix0 > ix1 or iy0 > iy1:
                    continue
                mask[iy0:iy1 + 1, ix0:ix1 + 1] = False
                on_x = centred(np.asarray(block.xs[ix0:ix1 + 1]), x0, block.pitch)
                on_y = centred(np.asarray(block.ys[iy0:iy1 + 1]), y0, block.pitch)
                mask[iy0:iy1 + 1, ix0:ix1 + 1] |= np.outer(on_y, on_x)
        if masks is None:
            return None
        # (ny, nx) on a uniform lattice; one entry per column on a nested one.
        return masks[0] if len(masks) == 1 else np.concatenate([mask.ravel() for mask in masks])

    def _net_id(self, name: str) -> int:
        if name not in self.net_ids:
            self.net_ids[name] = len(self.net_ids)
        return self.net_ids[name]

    def _obstacles(self):
        index = self.clearance
        g = self.lattice
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
        board, g = self.board, self.lattice
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
        if rule is not None and rule.max_vias is not None:
            spec.via_factor = self.options.via_budget_factor
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
        # Manhattan extent of the attach candidates in pitches (an ordering key).
        points = [g.point(node) for comp in spec.components for node, _, _ in comp]
        spec.span = ((max(p.x_nm for p in points) - min(p.x_nm for p in points)
                      + max(p.y_nm for p in points) - min(p.y_nm for p in points)) // self.pitch
                     if points else 0)
        return spec

    def _plane_via_taps(self, name: str, spec: _NetSpec, planes) -> frozenset[int]:
        """Nodes on every routing layer at existing vias of a plane net that
        already land in its plane: a pad reaches the plane by joining such a
        via on its own layer, without a new via (reference layouts share
        return vias between a ball and its bypass capacitor)."""
        g = self.lattice
        layers_all = list(g.layers)
        taps = set()
        for via in self.board.vias:
            if via.net != name:
                continue
            site = g.snap(0, via.position)
            if site is None:
                continue
            column = g.column(site)
            span = layers_all[layers_all.index(via.from_layer):layers_all.index(via.to_layer) + 1]
            # The plane must surround the via (fill reaches it from all
            # sides), not just touch it at one raster node.
            reach = max(1, int(round((via.size_nm / 2 + spec.clearance) / g.step_nm(site))) + 1)
            around = ((reach, 0), (-reach, 0), (0, reach), (0, -reach))
            def surrounded(index):
                ring = g.offset_nodes(g.layer_node(column, index), around)
                return len(ring) == len(around) and all(node in planes for node in ring.tolist())
            if not any(surrounded(layers_all.index(layer)) for layer in span):
                continue
            for layer in span:
                index = layers_all.index(layer)
                if index in spec.layers:
                    taps.add(g.layer_node(column, index))
        return frozenset(taps)

    def _zone_regions(self, name: str, spec: _NetSpec):
        """Plane target nodes of a zone net, its separate pours, and fragments.

        Fill is labelled per layer (8-connected over cells the net may use).
        Each layer's largest piece is plane copper when it is at least a
        quarter of the overall largest piece. Other pieces on a layer without
        plane copper are pours that must be joined; pieces on a plane layer
        are fragments (split islands, cut-off plane) joined when possible.
        """
        if self.nested:
            return self._zone_regions_nested(name, spec)
        from .plane_estimate import _label
        g = self.lattice
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
        minimum = max(4, int(self.options.plane_piece_area_nm2 // (g.pitch * g.pitch)))
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
                found.append(g.layer_node(np.flatnonzero(flat == label), index))
            pieces[index] = found
        if not pieces:
            return frozenset(), [], []
        largest = max(p[0].size for p in pieces.values())
        planes, islands, optional = set(), [], []
        for index, found in pieces.items():
            first, rest = found[0], found[1:]
            if first.size >= largest * self.options.plane_piece_fraction:
                planes.update(first.tolist())
            else:
                islands.append(first.tolist())
            optional.extend(r.tolist() for r in rest)
        return frozenset(planes), islands, optional

    def _label_nested(self, mask: np.ndarray) -> tuple[np.ndarray, int]:
        """4-connected labels of a per-column mask of a nested lattice.

        Each block is labelled on its own dense raster; labels of portal
        twins that are both in the mask are then joined (union-find), so a
        region crossing a patch boundary is one piece. Returns per-column
        labels (0: outside the mask) and the label count.
        """
        from .plane_estimate import _label
        g = self.lattice
        labels = np.zeros(g.plane, dtype=np.int64)
        total = 0
        for block in g.blocks:
            start, end = block.column_offset, block.column_offset + block.plane
            part, count = _label(mask[start:end].reshape(block.ny, block.nx))
            flat = part.ravel().astype(np.int64)
            flat[flat > 0] += total
            labels[start:end] = flat
            total += count
        if not total:
            return labels, 0
        base, fine = g.portal_pairs
        a, b = labels[base], labels[fine]
        join = (a > 0) & (b > 0) & (a != b)
        parent = list(range(total + 1))

        def find(x: int) -> int:
            while parent[x] != x:
                parent[x] = parent[parent[x]]
                x = parent[x]
            return x
        for x, y in sorted(set(zip(a[join].tolist(), b[join].tolist()))):
            rx, ry = find(x), find(y)
            if rx != ry:
                parent[max(rx, ry)] = min(rx, ry)
        roots = np.asarray([find(x) for x in range(total + 1)], dtype=np.int64)
        _, compact = np.unique(roots, return_inverse=True)
        compact = compact.reshape(-1)
        return compact[labels], int(compact.max())

    def _column_weights(self) -> np.ndarray:
        weights = getattr(self, "_weights_cache", None)
        if weights is None:
            weights = self._weights_cache = self.lattice.column_weight()
        return weights

    def _zone_regions_nested(self, name: str, spec: _NetSpec):
        """``_zone_regions`` on a nested lattice: pieces labelled per block and
        joined across portals; piece sizes are areas in base cells."""
        g = self.lattice
        plane = g.plane
        owner = self.rasters.track_arrays(spec.width, spec.clearance)[0].reshape(g.nl, plane)
        free = (owner == _FREE) | (owner == spec.nid)
        xs, ys = g.column_x.astype(np.float64), g.column_y.astype(np.float64)
        per_layer: dict[int, np.ndarray] = {}
        for zone in self.board.zones:
            if zone.net != name:
                continue
            inside = _polygon_inside(zone.outline.outer.vertices, xs, ys)
            for hole in zone.outline.holes:
                inside &= ~_polygon_inside(hole.vertices, xs, ys)
            for layer in zone.layers:
                index = g.layers.index(layer)
                mask = inside & free[index]
                per_layer[index] = per_layer[index] | mask if index in per_layer else mask
        minimum = max(4, int(self.options.plane_piece_area_nm2 // (g.pitch * g.pitch)))
        weights = self._column_weights()
        pieces: dict[int, list[tuple[float, np.ndarray]]] = {}
        for index, mask in per_layer.items():
            labels, count = self._label_nested(mask)
            if not count:
                continue
            sizes = np.bincount(labels, weights=weights, minlength=count + 1)
            order = np.argsort(-sizes[1:], kind="stable") + 1
            found = []
            for k, label in enumerate(order):
                if sizes[label] < minimum and k:
                    break
                found.append((float(sizes[label]), np.flatnonzero(labels == label) + index * plane))
            pieces[index] = found
        if not pieces:
            return frozenset(), [], []
        largest = max(found[0][0] for found in pieces.values())
        planes, islands, optional = set(), [], []
        for index, found in pieces.items():
            (area, first), rest = found[0], found[1:]
            if area >= largest * self.options.plane_piece_fraction:
                planes.update(first.tolist())
            else:
                islands.append(first.tolist())
            optional.extend(nodes.tolist() for _, nodes in rest)
        return frozenset(planes), islands, optional

    def _neck_nodes(self, net, spec: _NetSpec, length: int):
        """Breakout-region nodes, and those legal there only at the neck width."""
        from .drc import placed_pad_shape
        from .placement import transformed_local_point
        g, board = self.lattice, self.board
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
                for view in self.rasters.views:
                    block = view.block
                    window = block.window(min(xs) - reach, min(ys) - reach, max(xs) + reach, max(ys) + reach)
                    ix0, iy0, ix1, iy1 = window
                    if ix0 > ix1 or iy0 > iy1:
                        continue
                    wx = view.px[iy0:iy1 + 1, ix0:ix1 + 1]
                    wy = view.py[iy0:iy1 + 1, ix0:ix1 + 1]
                    # Strictly inside, with a small margin for the centreline test.
                    inside = _spine_distance(shape.spine, wx, wy) < reach - block.pitch // 8
                    for column in block.window_columns(window, inside).tolist():
                        for layer in spec.layers:
                            node = g.layer_node(column, layer)
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
                    layers = tuple(self.lattice.layers.index(l) for l in macro_port_layers(board, port))
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
            # Connectivity roots are opaque identities: union/find may retain
            # a pad label even after source tracks and vias join it. Reuse that
            # group's real copper regardless of the root's spelling.
            joined = set()
            for index, via in enumerate(board.vias):
                if via.net != net.name or graph.roots.get(f"via:{index}") != root:
                    continue
                first, last = sorted((self.lattice.layers.index(via.from_layer),
                                      self.lattice.layers.index(via.to_layer)))
                layers = tuple(range(first, last + 1))
                anchors.append((via.position, layers))
                site = self.lattice.snap(0, via.position)
                if site is not None:
                    joined.update(self.lattice.layer_node(self.lattice.column(site), layer) for layer in layers)
            for index, track in enumerate(board.tracks):
                if track.net == net.name and graph.roots.get(f"track:{index}") == root:
                    layer = self.lattice.layers.index(track.layer)
                    anchors.append((track.start, (layer,)))
                    anchors.append((track.end, (layer,)))
            boundary = [ports[pad] for pad in members if pad in ports]
            if boundary:
                # Source-copper reuse must not expose a macro's private vias or
                # bypass its declared boundary stub. Only the proved ports are
                # public anchors, even when union/find roots carry pad labels.
                anchors = [(port.position, tuple(self.lattice.layers.index(layer)
                            for layer in macro_port_layers(board, port))) for port in boundary]
                # A port off the lattice has no node (it cannot match a joined node).
                public_nodes = {self.lattice.snap(layer, position)
                                for position, layers in anchors for layer in layers}
                joined.intersection_update(public_nodes)
            required = [self.required_accesses[pad] for pad in members if pad in self.required_accesses]
            if required:
                # Land and existing-copper alternatives would bypass the retained
                # witness and leave its far end dangling after the dependency rip-up.
                anchors = [(access.position, (self.lattice.layers.index(access.layer),))
                           for access in required]
            # Existing via taps count only on the actual source barrel span.
            if joined:
                spec.joined[len(components)] = joined
            components.append(self._attach(anchors, spec))
            spec.anchors.append(tuple(anchor[0] for anchor in anchors))
            if self.explicit_pads and set(members) & self.explicit_pads:
                spec.explicit[len(components) - 1] = self._explicit_exclusion(members, spec)
        for anchors in separate:
            components.append(self._attach(anchors, spec))
            spec.anchors.append(tuple(anchor[0] for anchor in anchors))
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
        return frozenset(self.lattice.layers.index(CopperLayer.FRONT if poses[pad.component].side.value == "front"
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
                layers = (self.lattice.layers.index(side),)
            else:
                layers = tuple(range(self.lattice.nl))
            # Outward axis from the package centre, for fine-pitch lead-ins.
            dx, dy = position.x_nm - pose.position.x_nm, position.y_nm - pose.position.y_nm
            outward = None
            if max(abs(dx), abs(dy)) > self.options.outward_axis_threshold_nm:
                outward = (1 if dx > 0 else -1, 0) if abs(dx) >= abs(dy) else (0, 1 if dy > 0 else -1)
            result.append((position, layers, outward))
        return result

    def _attach(self, anchors, spec: _NetSpec):
        """Grid nodes reachable from anchors with exact, checked lead-ins."""
        from .pin_escape import checked_access_path
        g, options = self.lattice, self.options
        owner, _ = self.rasters.track(spec.width, spec.clearance)
        radius = options.attach_radius_nm
        seen: dict[int, tuple[int, tuple[TrackSegment, ...], float]] = {}
        for anchor in anchors:
            position, layers = anchor[0], anchor[1]
            outward = anchor[2] if len(anchor) > 2 else None
            near = _nearest_first(g, position, radius)
            for layer in layers:
                if layer not in spec.layers:
                    continue
                found = 0
                for distance, column, x, y in near:
                    if distance > radius or found >= options.attach_candidates:
                        break
                    node = g.layer_node(column, layer)
                    if owner[node] not in (_FREE, spec.nid) and node not in spec.neck:
                        continue
                    target = Point(x, y)
                    path = None
                    if node not in spec.neck:
                        path = checked_access_path(self.board, self.clearance, spec.name, position, target,
                                                   spec.width, g.layers[layer])
                    if path is None and spec.neck_width:
                        path = checked_access_path(self.board, self.clearance, spec.name, position, target,
                                                   spec.neck_width, g.layers[layer])
                    if path is None:
                        continue
                    cost = distance / self.pitch
                    if node not in seen or seen[node][2] > cost:
                        seen[node] = (node, tuple(path), cost)
                    found += 1
                if found < 2 and outward is not None:
                    self._attach_extended(position, outward, layer, spec, owner, seen)
                if options.local_escape_radius_nm and not self._reaches_open(
                        [node for node in seen if g.layer_of(node) == layer], spec):
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
        cache = self._open_cache
        if key not in cache and self.nested:
            cache[key] = self._open_space_nested(width, clearance)
        if key not in cache:
            g = self.lattice
            owner, _ = self.rasters.track(width, clearance)
            arr = np.asarray(owner, dtype=np.int32).reshape(g.nl, g.ny, g.nx)
            minimum = max(16, int(self.options.open_space_area_nm2 // (g.pitch * g.pitch)))
            result = np.zeros(g.size, dtype=bool)
            for layer in range(g.nl):
                labels, count = _label(arr[layer] == _FREE)
                if not count:
                    continue
                sizes = np.bincount(labels.ravel(), minlength=count + 1)
                big = sizes >= minimum
                big[0] = False
                result[g.layer_node(0, layer):g.layer_node(0, layer + 1)] = big[labels.ravel()]
            cache[key] = result
        return cache[key]

    def _open_space_nested(self, width: int, clearance: int) -> np.ndarray:
        """``_open_space`` on a nested lattice: free regions labelled per block,
        joined across portals, measured as areas in base cells."""
        g = self.lattice
        plane = g.plane
        arr = self.rasters.track_arrays(width, clearance)[0].reshape(g.nl, plane)
        minimum = max(16, int(self.options.open_space_area_nm2 // (g.pitch * g.pitch)))
        weights = self._column_weights()
        result = np.zeros(g.size, dtype=bool)
        for layer in range(g.nl):
            labels, count = self._label_nested(arr[layer] == _FREE)
            if not count:
                continue
            sizes = np.bincount(labels, weights=weights, minlength=count + 1)
            big = sizes >= minimum
            big[0] = False
            result[layer * plane:(layer + 1) * plane] = big[labels]
        return result

    def _reaches_open(self, nodes, spec: _NetSpec, limit: int | None = None) -> bool:
        """Whether a short flood from ``nodes`` (through free or own-net cells
        on the same layer, at the net's narrowest width) reaches open space."""
        if not nodes:
            return False
        if limit is None:
            limit = self.options.open_flood_limit
        g = self.lattice
        open_space = self._open_space(spec)
        owner, _ = self.rasters.track(spec.neck_width or spec.width, spec.neck_clearance or spec.clearance)
        nid = spec.nid
        stack, visited = list(nodes), set(nodes)
        while stack and len(visited) < limit:
            node = stack.pop()
            if open_space[node]:
                return True
            for other, _ in g.neighbours(node, diagonals=False):
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
        g, options = self.lattice, self.options
        if self._local_spent >= options.local_escape_limit:
            return
        self._local_spent += 1
        open_space = self._open_space(spec)
        width = spec.neck_width or spec.width
        reach = options.local_escape_radius_nm
        owner, _ = self.rasters.track(spec.width, spec.clearance)
        def rank(item):
            column, x, y = item
            dx, dy = x - position.x_nm, y - position.y_nm
            distance = hypot(dx, dy)
            # Targets ahead of the pin (along its outward axis) first: a pin's
            # exit is rarely back across its own package.
            behind = outward is not None and dx * outward[0] + dy * outward[1] < 0
            return (distance * (2.0 if behind else 1.0), distance, column, x, y)
        # Stable sort of the lattice's x-major cover: ties keep that order.
        near = sorted(map(rank, g.cover(position.x_nm - reach, position.y_nm - reach,
                                        position.x_nm + reach, position.y_nm + reach)),
                      key=lambda item: (item[0], item[1]))
        found = tries = misses = 0
        for _, distance, column, x, y in near:
            # A failed maze is the expensive case; four in a row mean the pin
            # is enclosed beyond this radius (a dense array without a dogbone).
            if distance > reach or found >= 3 or tries >= 12 or misses >= 4:
                break
            node = g.layer_node(column, layer)
            if not open_space[node] or owner[node] != _FREE:
                continue
            tries += 1
            target = Point(x, y)
            path = local_access_path(self.board, self.clearance, spec.name, position, target,
                                     g.layers[layer], width, step_nm=options.local_escape_step_nm,
                                     detour_nm=options.local_escape_detour_nm,
                                     maximum_states=options.local_escape_states)
            if not path:
                misses += 1
                continue
            misses = 0
            cost = 2 * distance / self.pitch
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
        g = self.lattice
        widths = [spec.width] + ([spec.neck_width] if spec.neck_width else [])
        reach = self.options.attach_extension_reach_nm
        for extension in self.options.attach_extension_steps_nm:
            corner = Point(position.x_nm + outward[0] * extension, position.y_nm + outward[1] * extension)
            for width in widths:
                if not self.clearance.can_track(spec.name, position, corner, width, g.layers[layer]):
                    continue
                stem = TrackSegment(spec.name, position, corner, width, g.layers[layer])
                found = 0
                for distance, column, x, y in _nearest_first(g, corner, reach):
                    if found >= 3:
                        break
                    node = g.layer_node(column, layer)
                    if owner[node] not in (_FREE, spec.nid):
                        continue
                    target = Point(x, y)
                    path = checked_access_path(self.board, self.clearance, spec.name, corner, target,
                                               width, g.layers[layer])
                    if path is None:
                        continue
                    cost = (extension + distance) / self.pitch
                    if node not in seen or seen[node][2] > cost:
                        seen[node] = (node, (stem, *path), cost)
                    found += 1
                if found:
                    return

    # -- search -----------------------------------------------------------

    def _route_plane_net(self, spec: _NetSpec, present: float) -> _NetRoute:
        """Join every pad group (and small pour) of a zone net to its plane."""
        g = self.lattice
        route = _NetRoute([], [], set(), set(), set(), set(), False, 0.0)
        targets = set(spec.plane)
        missing = 0
        whole = g.full_box()
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
            # A reachable grid node is not existing copper. If reaching the
            # plane needs a lead-in, route even a one-node path so commit emits it.
            if excluded is None and (any(node in targets and not lead for node, lead, _ in component)
                                     or any(node in targets for node in spec.joined.get(position, ()))):
                if position >= first_island:
                    targets.update(node for node, _, _ in component)
                continue  # Already inside the plane's fill.
            sources = {node: cost + self._lead_penalty(lead, present) for node, lead, cost in component}
            leads = {node: lead for node, lead, _ in component}
            if excluded is not None:
                # Explicit copper: the path must end on another layer's plane.
                allowed = {node for node in targets if g.layer_of(node) not in excluded}
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
            box = g.grow_box(g.box_of(sources), margin)
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
        return route.length * self.pitch + leads   # route.length is in (base) pitches

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
        if not route.connected and self._expired():
            route.reason = "time budget exhausted"
        elif not route.connected and len(spec.layers) > 1 and self.board.stackup.via_technologies:
            if not self._via_transitions(spec)[0]:
                route.reason += "; no_via_technology for allowed layer transitions"
        return route

    def _route_net_budgeted(self, spec: _NetSpec, present: float) -> _NetRoute:
        route = self._route_net_once(spec, present)
        if spec.via_budget is not None and len(route.via_xy) > spec.via_budget:
            # Over the net's via budget: search again with vias priced far
            # higher; a budget overrun stays a failure, not a DRC error.
            factor = spec.via_factor
            spec.via_factor = factor * self.options.via_overrun_factor
            try:
                retry = self._route_net_once(spec, present)
            finally:
                spec.via_factor = factor
            # An unconnected retry trivially has fewer vias; keep the original
            # route then, so the failure names the via shortfall.
            if retry.connected and len(retry.via_xy) <= len(route.via_xy):
                route = retry
            if len(route.via_xy) > spec.via_budget:
                route = _NetRoute([], [], set(), set(), set(), set(), False, 0.0,
                                  f"needs {len(route.via_xy)} new vias, max_vias leaves {spec.via_budget}")
        return route

    def _route_net_once(self, spec: _NetSpec, present: float) -> _NetRoute:
        if spec.plane:
            return self._route_plane_net(spec, present)
        g = self.lattice
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
        base = g.point(next(iter(tree)))
        def dist(ci):
            here = g.point(components[ci][0][0])
            return abs(here.x_nm - base.x_nm) + abs(here.y_nm - base.y_nm)
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
        if self.nested:
            self._finish_route_nested(route, spec)
            return
        g = self.lattice
        nodes: set[int] = set()
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
        # Layer-change sites: one via per xy, spanning every layer the
        # route changes between there (see _site_spans).
        changes: dict[int, tuple[int, int]] = {}
        layer_of, offset_nodes, delta = g.layer_of, g.offset_nodes, g.delta
        for path in route.paths:
            for a, b in zip(path, path[1:]):
                la, lb = layer_of(a), layer_of(b)
                if la != lb:
                    xy = g.column(a)
                    low, high = changes.get(xy, (la, la))
                    changes[xy] = (min(low, la, lb), max(high, la, lb))
                    continue
                dx, dy = delta(a, b)
                length += hypot(dx, dy)
                claims.update(offset_nodes(a, _segment_offsets_cached(dx, dy, track_radius)).tolist())
            nodes.update(path)
            if len(path) == 1:
                claims.update(offset_nodes(path[0], _disc_offsets_cached(track_radius)).tolist())
        # Lead-ins are off-grid copper: claim around them and treat the
        # nodes along them as path nodes, so they negotiate like paths.
        index_of = {layer: i for i, layer in enumerate(g.layers)}
        for leads in route.leads:
            for track in leads:
                layer = index_of[track.layer]
                radius_nm = track.width_nm / 2 + spec.clearance + max(self.default_width, track.width_nm) / 2
                near = g.pitch / 2
                for node, distance in g.segment_nodes(layer, track.start, track.end, radius_nm):
                    claims.add(node)
                    if distance <= near:
                        nodes.add(node)
        via_xy = set(changes)
        spans = self._site_spans(spec.name, changes)
        for xy in via_xy:
            # A via's copper claims only the layers it spans (all of them for
            # a through via, or where no technology serves the site and the
            # copper falls back to the outer span). Via-to-via spacing below
            # stays per xy: drills clear each other whatever their spans.
            span = spans[xy]
            if span is None:
                via_layers = range(g.nl)
            else:
                low, high = sorted((g.layers.index(span[0]), g.layers.index(span[1])))
                via_layers = range(low, high + 1)
            layer_node = g.layer_node
            if via_body >= 0.5:
                for column in g.offset_columns(xy, _disc_offsets_cached(via_body)).tolist():
                    for layer in via_layers:
                        body[layer_node(column, layer)] = ("via", xy)
            for column in g.offset_columns(xy, _disc_offsets_cached(via_radius)).tolist():
                for layer in via_layers:
                    claims.add(layer_node(column, layer))
            via_claims.update(g.offset_columns(xy, _disc_offsets_cached(via_via_radius)).tolist())
        route.nodes, route.via_xy = nodes, via_xy
        route.claims, route.via_claims = claims, via_claims
        route.body = {node: owner for node, owner in body.items() if node not in nodes}
        route.length = length

    # Nested-lattice claims: a claim disc (or a step's capsule) that lies
    # wholly in its node's own block uses that block's offset tables, the
    # radius scaled by the block's factor; one reaching across a patch
    # boundary is collected geometrically from every block it overlaps.
    # Portal twins are claimed together (they are one site).

    def _nested_step_claims(self, a: int, b: int, pa: Point, pb: Point, radius: float):
        g = self.lattice
        block = g.blocks[g.block_index(a % g.plane)]
        r = radius * self.pitch
        if g.owns(block, min(pa.x_nm, pb.x_nm) - r, min(pa.y_nm, pb.y_nm) - r,
                  max(pa.x_nm, pb.x_nm) + r, max(pa.y_nm, pb.y_nm) + r):
            dx, dy = g.delta(a, b)
            return g.offset_nodes(a, _segment_offsets_cached(dx, dy, radius * block.factor)).tolist()
        return [node for node, _ in g.segment_nodes(a // g.plane, pa, pb, r)] + [a, b]

    def _nested_disc_columns(self, column: int, radius: float) -> np.ndarray:
        g = self.lattice
        block = g.blocks[g.block_index(column)]
        point = g.column_point(column)
        r = radius * self.pitch
        if g.owns(block, point.x_nm - r, point.y_nm - r, point.x_nm + r, point.y_nm + r):
            return g.offset_columns(column, _disc_offsets_cached(radius * block.factor))
        return np.union1d(g.columns_within(point, r), [column])

    def _finish_route_nested(self, route: _NetRoute, spec: _NetSpec) -> None:
        """``_finish_route`` on a nested lattice (lengths in base pitches)."""
        g = self.lattice
        plane, pitch = g.plane, self.pitch
        nodes: set[int] = set()
        claims: set[int] = set()
        via_claims: set[int] = set()
        body: dict = {}
        via_body = (spec.via_size - self.default_width) / 2 / pitch
        neighbour = max(self.default_width, spec.width)
        track_radius = (spec.width / 2 + spec.clearance + neighbour / 2) / pitch
        via_reach = max(spec.via_size / 2 + spec.clearance,
                        spec.via_drill / 2 + self.board.rules.minimum_hole_clearance_nm)
        via_radius = (via_reach + self.default_width / 2) / pitch
        via_via_radius = (via_reach + spec.via_size / 2) / pitch
        length = 0.0
        changes: dict[int, tuple[int, int]] = {}
        point = g.point
        for path in route.paths:
            moved = False
            for a, b in zip(path, path[1:]):
                la, lb = a // plane, b // plane
                if la != lb:
                    xy = a % plane
                    low, high = changes.get(xy, (la, la))
                    changes[xy] = (min(low, la, lb), max(high, la, lb))
                    moved = True
                    continue
                pa, pb = point(a), point(b)
                if pa == pb:
                    continue        # a portal: the twins are one site
                moved = True
                length += hypot(pb.x_nm - pa.x_nm, pb.y_nm - pa.y_nm) / pitch
                claims.update(self._nested_step_claims(a, b, pa, pb, track_radius))
            nodes.update(path)
            if not moved:
                column = path[0] % plane
                base = path[0] - column
                claims.update((self._nested_disc_columns(column, track_radius) + base).tolist())
        index_of = {layer: i for i, layer in enumerate(g.layers)}
        for leads in route.leads:
            for track in leads:
                layer = index_of[track.layer]
                radius_nm = track.width_nm / 2 + spec.clearance + max(self.default_width, track.width_nm) / 2
                for node, distance in g.segment_nodes(layer, track.start, track.end, radius_nm):
                    claims.add(node)
                    if distance <= g.step_nm(node) / 2:
                        nodes.add(node)
        via_xy = set(changes)
        spans = self._site_spans(spec.name, changes)
        for xy in via_xy:
            span = spans[xy]
            if span is None:
                via_layers = range(g.nl)
            else:
                low, high = sorted((g.layers.index(span[0]), g.layers.index(span[1])))
                via_layers = range(low, high + 1)
            if via_body >= 0.5:
                for column in g.columns_with_twins(self._nested_disc_columns(xy, via_body)).tolist():
                    for layer in via_layers:
                        body[layer * plane + column] = ("via", xy)
            for column in self._nested_disc_columns(xy, via_radius).tolist():
                for layer in via_layers:
                    claims.add(layer * plane + column)
            via_claims.update(g.columns_with_twins(self._nested_disc_columns(xy, via_via_radius)).tolist())

        def closed(values) -> set[int]:
            return set(g.with_twins(np.fromiter(values, dtype=np.int64, count=len(values))).tolist())
        nodes, claims = closed(nodes), closed(claims)
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
            g = self.lattice
            layer_of = {layer: i for i, layer in enumerate(g.layers)}
            nodes = []
            for track in lead:
                if self.nested:
                    # Nodes along the lead in their own pitch, each weighted
                    # by that pitch in base pitches (a price per base pitch).
                    nodes.extend((node, weight) for node, _, weight in g.segment_nodes_local(
                        layer_of[track.layer], track.start, track.end, 0.5, 1))
                    continue
                nodes.extend(node for node, distance in g.segment_nodes(
                    layer_of[track.layer], track.start, track.end, g.pitch / 2 + 1))
            self._lead_nodes[lead] = nodes
        if not nodes:
            return 0.0
        hist, occ = self.occupancy.history, self.occupancy.occ
        if self.nested:
            return float(sum(weight * (hist[n] + present * occ[n]) for n, weight in nodes))
        if not self.occupancy.arrays:
            return sum(hist[n] + present * occ[n] for n in nodes)
        index = np.asarray(nodes, dtype=np.int64)
        return float(hist[index].sum() + present * occ[index].sum())

    def _segment_nodes(self, start: Point, end: Point, layer: int, radius_nm: float):
        """(node, distance) for grid nodes strictly within radius of a segment."""
        return self.lattice.segment_nodes(layer, start, end, radius_nm)

    def _margin(self, spec: _NetSpec) -> int:
        age = self.conflict_age.get(spec.name, 0)
        scale = min(self.options.window_growth_limit, 1 + age // 3)
        return int(self.options.window_margin_nm * scale // self.lattice.pitch)

    def _via_owners(self, spec: _NetSpec, *, as_array=False, span=None):
        """Via proposals for a single-ended net must have existing returns.

        The raster narrows proposals only. Exact integer distance/span checks
        repeat at commit, and independent physical DRC checks all source vias.
        """
        rule = self.rules.get(spec.name)
        if rule is None or not rule.require_return_vias or rule.differential_partner is not None:
            return (self.rasters.via_array(spec.via_size, spec.via_drill, spec.clearance, span) if as_array
                    else self.rasters.via(spec.via_size, spec.via_drill, spec.clearance, span))
        key = ("return_via_owner", span)
        cached = spec.arrays.get(key)
        if cached is not None:
            return cached
        from .return_paths import return_span_covers
        g = self.lattice
        low, high = span if span is not None else (0, g.nl - 1)
        masks = [np.zeros((view.block.ny, view.block.nx), dtype=bool) for view in self.rasters.views]
        limit = rule.maximum_return_via_distance_nm
        for via in self.board.vias:
            if (via.net != rule.return_via_net or via.net == spec.name
                    or not return_span_covers(self.board, via, g.layers[low], g.layers[high])):
                continue
            x, y = via.position.x_nm, via.position.y_nm
            for view, mask in zip(self.rasters.views, masks):
                x0, y0, x1, y1 = view.block.window(x-limit, y-limit, x+limit, y+limit)
                if x0 > x1 or y0 > y1:
                    continue
                # As with the obstacle raster, numerical membership is a proposal.
                # Exact integer squared distance is authoritative at commit/DRC.
                distance = np.hypot(view.px[y0:y1+1, x0:x1+1]-x, view.py[y0:y1+1, x0:x1+1]-y)
                mask[y0:y1+1, x0:x1+1] |= distance <= limit
        owners = self.rasters.via_array(spec.via_size, spec.via_drill, spec.clearance, span).copy()
        if len(masks) == 1:
            owners.reshape((g.ny, g.nx))[~masks[0]] = _FOREIGN
        else:
            owners[~np.concatenate([mask.ravel() for mask in masks])] = _FOREIGN
        spec.arrays[key] = owners
        return owners

    def _via_transitions(self, spec: _NetSpec):
        """Optional span-specific owner arrays and native pointer table.

        Boards without a technology catalog keep the single-mask fast path. A null
        entry in the table means no technology serves that layer transition.
        Arrays are retained in the spec for the lifetime of the native call.
        """
        if not self.board.stackup.via_technologies:
            return None
        if "via_transitions" not in spec.arrays:
            g = self.lattice
            owners = {}
            pointers = np.zeros(g.nl * g.nl, dtype=np.uintp)
            for low in spec.layers:
                for high in spec.layers:
                    if low >= high:
                        continue
                    physical = self._transition_span(low, high)
                    if physical is None:
                        continue
                    span = tuple(sorted((g.layers.index(physical[0]), g.layers.index(physical[1]))))
                    mask = self._via_owners(spec, as_array=True, span=span)
                    owners[low, high] = owners[high, low] = mask
                    pointers[low * g.nl + high] = pointers[high * g.nl + low] = mask.ctypes.data
            spec.arrays["via_transitions"] = owners, pointers
        return spec.arrays["via_transitions"]

    def _search(self, spec: _NetSpec, sources: Mapping[int, float], targets, present: float,
                target_extra=None):
        if self._expired():
            return None
        g, options = self.lattice, self.options
        owner, mid_owner = self.rasters.track(spec.width, spec.clearance)
        via_owner = self._via_owners(spec)
        # Search windows are inclusive lattice index boxes (target box for the heuristic).
        tbox = g.box_of(targets)
        sx0, sy0, sx1, sy1 = g.box_of(sources)
        margin = self._margin(spec)
        box = g.grow_box((min(tbox[0], sx0), min(tbox[1], sy0), max(tbox[2], sx1), max(tbox[3], sy1)), margin)
        corridor = self._guide_corridor(spec) if self.guides else None
        if corridor is not None:
            # A guided net's window covers its corridor, so the soft guide
            # cost can be followed where the guide detours.
            cx0, cy0, cx1, cy1 = corridor[2]
            box = (min(box[0], cx0), min(box[1], cy0), max(box[2], cx1), max(box[3], cy1))
        whole = g.full_box()
        for attempt in range(3):
            result = self._astar(spec, sources, targets, present, box, tbox,
                                 owner, mid_owner, via_owner, target_extra)
            if result is not None:
                return result
            if box == whole:
                return None
            box = g.grow_box(box, margin * (attempt + 2) * 2)
            if attempt == 1:
                box = whole
        return None

    def _astar(self, spec, sources, targets, present, box, tbox, owner, mid_owner, via_owner,
               target_extra=None):
        """A* over the shared grid; arrays are reused across searches."""
        if self._expired():
            return None
        if self.native is not None:
            result = self._astar_native(spec, sources, targets, present, box, tbox, target_extra)
            if result is NotImplemented and self._grow_native_buffers():
                result = self._astar_native(spec, sources, targets, present, box, tbox, target_extra)
            if result is not NotImplemented:
                return result
        if self.nested:
            return self._astar_nested(spec, sources, targets, present, box, tbox, owner, mid_owner,
                                      via_owner, target_extra)
        g, options = self.lattice, self.options
        occ, via_occ = self.occupancy.occ, self.occupancy.via_occ
        hist, via_hist = self.occupancy.history, self.occupancy.via_history
        nid = spec.nid
        neck, neck_mid, region = spec.neck, spec.neck_mid, spec.region
        use_neck = bool(neck or neck_mid)
        decode, layer_of, bases = g.decoder(), g.layer_of, g.layer_bases()
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
            layer, rest, x, y = decode(node)
            if layer not in allowed:
                continue
            best[node] = cost
            parent[node] = -1
            touched.append(node)
            hx = tx0 - x if x < tx0 else (x - tx1 if x > tx1 else 0)
            hy = ty0 - y if y < ty0 else (y - ty1 if y > ty1 else 0)
            push(heap, (cost + (hx if hx > hy else hy) + 0.41421356 * (hy if hx > hy else hx), cost, node))
        expansions = 0
        limit = options.maximum_expansions
        found = None
        # Per layer: move list with heading-adjusted base costs, from the
        # lattice's move deltas (node-id step and diagonal cell step).
        moves = g.move_deltas()
        move_tables = []
        for layer in range(g.nl):
            head = heading[layer]
            lc = layer_cost[layer]
            table = []
            for m, (dx, dy, dn, dcell, step) in enumerate(moves):
                base = step * (1.0 + lc)
                if head is not None and (head == "h" and dy and not dx or head == "n" and dx and not dy):
                    base += wrong * step
                table.append((dx, dy, dn, base, bool(dx and dy), dcell, m, step))
            move_tables.append(table)
        # Soft cost terms (see UnifiedRouterOptions.bend_cost/guide_cost);
        # the kernel adds them in the same order.
        turns = arrival = None
        if options.bend_cost:
            table = self._turn_table().tolist()
            turns = [table[8 * m:8 * m + 8] for m in range(8)]
            arrival = {dn: m for m, (_, _, dn, _, _) in enumerate(moves)}
        corridor = self._guide_corridor(spec) if self.guides else None
        inside = corridor[0] if corridor is not None else None
        guide_cost = options.guide_cost
        transitions = self._via_transitions(spec)
        while heap:
            if expansions % 1024 == 0 and self._expired():
                break
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
            layer, rest, x, y = decode(node)
            here_in_region = use_neck and node in region
            # A node legal only at neck width must not be left towards a node
            # outside the region: that step would be drawn at full width.
            here_neck_only = here_in_region and node in neck
            only_targets = layer in target_only
            # Turn cost from the heading of this node's best parent (none
            # after a via or at a source).
            turn = None
            if turns is not None:
                p = parent[node]
                if p != -1 and layer_of(p) == layer:
                    turn = turns[arrival[node - p]]
            for dx, dy, dn, base, diagonal, dcell, m, step in move_tables[layer]:
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
                if turn is not None:
                    c += turn[m]
                if inside is not None and n2 not in inside:
                    c += step * guide_cost
                if c < best[n2]:
                    if best[n2] >= INF:
                        touched.append(n2)
                    best[n2] = c
                    parent[n2] = node
                    hx = tx0 - x2 if x2 < tx0 else (x2 - tx1 if x2 > tx1 else 0)
                    hy = ty0 - y2 if y2 < ty0 else (y2 - ty1 if y2 > ty1 else 0)
                    push(heap, (c + (hx if hx > hy else hy) + 0.41421356 * (hy if hx > hy else hx), c, n2))
            # Layer changes through the selected physical via span.
            o = via_owner[rest]
            if transitions is None and o != -1 and o != nid:
                continue
            vpen = via_cost * (1.0 + via_hist[rest]) * (1.0 + present * via_occ[rest])
            hx = tx0 - x if x < tx0 else (x - tx1 if x > tx1 else 0)
            hy = ty0 - y if y < ty0 else (y - ty1 if y > ty1 else 0)
            h = (hx if hx > hy else hy) + 0.41421356 * (hy if hx > hy else hx)
            for layer2 in layers:
                if layer2 == layer:
                    continue
                if transitions is not None:
                    mask = transitions[0].get((layer, layer2))
                    if mask is None or mask[rest] not in (_FREE, nid):
                        continue
                n2 = bases[layer2] + rest    # the via site's node on layer2
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

    def _nested_search_tables(self):
        """Per block and layer, the heading-adjusted move tables of the nested
        search, and per block the column delta -> move index map."""
        if self._nested_tables is None:
            g, options = self.lattice, self.options
            tables, arrivals = [], []
            for index in range(len(g.blocks)):
                moves = g.block_moves(index)
                per_layer = []
                for layer in range(g.nl):
                    head = self.heading[layer]
                    lc = self.layer_cost[layer]
                    table = []
                    for m, (dx, dy, dn, dcell, step) in enumerate(moves):
                        base = step * (1.0 + lc)
                        if head is not None and (head == "h" and dy and not dx or head == "n" and dx and not dy):
                            base += options.wrong_way_cost * step
                        table.append((dx, dy, dn, base, bool(dx and dy), dcell, m, step))
                    per_layer.append(table)
                tables.append(per_layer)
                arrivals.append({dn: m for m, (_, _, dn, _, _) in enumerate(moves)})
            self._nested_tables = (tables, arrivals)
        return self._nested_tables

    def _astar_nested(self, spec, sources, targets, present, box, tbox, owner, mid_owner, via_owner,
                      target_extra=None):
        """``_astar`` on a nested lattice in Python (the native kernel's parity reference).

        Planar moves stay within a node's block (lengths and costs per base
        pitch); a node with a portal twin also moves to it at cost 0 (no
        heading, turn or guide term). Boxes are base-lattice index boxes; the
        heuristic is the octile distance in base pitches from the node's
        point to the target box.
        """
        g, options = self.lattice, self.options
        occ, via_occ = self.occupancy.occ, self.occupancy.via_occ
        hist, via_hist = self.occupancy.history, self.occupancy.via_history
        nid = spec.nid
        neck, neck_mid, region = spec.neck, spec.neck_mid, spec.region
        use_neck = bool(neck or neck_mid)
        plane = g.plane
        bases = g.layer_bases()
        ux, uy, block_of, twins = g.search_tables()
        offsets = [block.column_offset for block in g.blocks]
        widths = [block.nx for block in g.blocks]
        heights = [block.ny for block in g.blocks]
        bx0, by0, bx1, by1 = box
        tx0, ty0, tx1, ty1 = tbox
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
            best[node] = cost
            parent[node] = -1
            touched.append(node)
            x, y = ux[rest], uy[rest]
            hx = tx0 - x if x < tx0 else (x - tx1 if x > tx1 else 0.0)
            hy = ty0 - y if y < ty0 else (y - ty1 if y > ty1 else 0.0)
            push(heap, (cost + (hx if hx > hy else hy) + 0.41421356 * (hy if hx > hy else hx), cost, node))
        expansions = 0
        limit = options.maximum_expansions
        found = None
        move_tables, arrivals = self._nested_search_tables()
        turns = None
        if options.bend_cost:
            table = self._turn_table().tolist()
            turns = [table[8 * m:8 * m + 8] for m in range(8)]
        corridor = self._guide_corridor(spec) if self.guides else None
        inside = corridor[0] if corridor is not None else None
        guide_cost = options.guide_cost
        transitions = self._via_transitions(spec)
        while heap:
            if expansions % 1024 == 0 and self._expired():
                break
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
            b = block_of[rest]
            nxb, nyb = widths[b], heights[b]
            iy, ix = divmod(rest - offsets[b], nxb)
            x, y = ux[rest], uy[rest]
            here_in_region = use_neck and node in region
            here_neck_only = here_in_region and node in neck
            only_targets = layer in target_only
            turn = None
            if turns is not None:
                p = parent[node]
                if p != -1 and p // plane == layer and block_of[p - bases[layer]] == b:
                    m0 = arrivals[b].get(node - p)
                    if m0 is not None:
                        turn = turns[m0]
            for dx, dy, dn, base, diagonal, dcell, m, step in move_tables[b][layer]:
                x2 = ix + dx
                y2 = iy + dy
                if x2 < 0 or x2 >= nxb or y2 < 0 or y2 >= nyb:
                    continue
                r2 = rest + dn
                u = ux[r2]
                v = uy[r2]
                if u < bx0 or u > bx1 or v < by0 or v > by1:
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
                if turn is not None:
                    c += turn[m]
                if inside is not None and n2 not in inside:
                    c += step * guide_cost
                if c < best[n2]:
                    if best[n2] >= INF:
                        touched.append(n2)
                    best[n2] = c
                    parent[n2] = node
                    hx = tx0 - u if u < tx0 else (u - tx1 if u > tx1 else 0.0)
                    hy = ty0 - v if v < ty0 else (v - ty1 if v > ty1 else 0.0)
                    push(heap, (c + (hx if hx > hy else hy) + 0.41421356 * (hy if hx > hy else hx), c, n2))
            hx = tx0 - x if x < tx0 else (x - tx1 if x > tx1 else 0.0)
            hy = ty0 - y if y < ty0 else (y - ty1 if y > ty1 else 0.0)
            h = (hx if hx > hy else hy) + 0.41421356 * (hy if hx > hy else hx)
            # Portal: the coincident node of the other block, at no cost.
            t = twins[rest]
            if t >= 0:
                n2 = node - rest + t
                o = owner[n2]
                if not (only_targets and n2 not in targets) and not (here_neck_only and n2 not in region) \
                        and (o == -1 or o == nid or (here_in_region and n2 in neck)) and cost < best[n2]:
                    if best[n2] >= INF:
                        touched.append(n2)
                    best[n2] = cost
                    parent[n2] = node
                    push(heap, (cost + h, cost, n2))
            # Layer changes through the selected physical via span.
            o = via_owner[rest]
            if transitions is None and o != -1 and o != nid:
                continue
            vpen = via_cost * (1.0 + via_hist[rest]) * (1.0 + present * via_occ[rest])
            for layer2 in layers:
                if layer2 == layer:
                    continue
                if transitions is not None:
                    mask = transitions[0].get((layer, layer2))
                    if mask is None or mask[rest] not in (_FREE, nid):
                        continue
                n2 = bases[layer2] + rest    # the via site's node on layer2
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
        size = self.lattice.size
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
        if self._expired():
            return None
        import ctypes
        g = self.lattice
        owner, mid_owner = self.rasters.track_arrays(spec.width, spec.clearance)
        via_owner = self._via_owners(spec, as_array=True)
        transitions = self._via_transitions(spec)
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
        turn_pointer = self._turn_table().ctypes.data if self.options.bend_cost else None
        corridor = self._guide_corridor(spec) if self.guides else None
        guide_pointer = None
        if corridor is not None:
            if self._n_guide is None:
                self._n_guide = np.full(g.size, float(self.options.guide_cost), dtype=np.float64)
            self._n_guide[corridor[1]] = 0.0
            guide_pointer = self._n_guide.ctypes.data
        touched_count = ctypes.c_int(0)
        P = lambda a: a.ctypes.data
        # A nested lattice passes its block tables; a uniform one passes none.
        if self.nested:
            blocks, block_of, ux, uy, twins, steps = g.kernel_tables()
            lattice_tables = (len(g.blocks), P(blocks), P(block_of), P(ux), P(uy), P(twins), P(steps))
        else:
            lattice_tables = (1, None, None, None, None, None, None)
        length = self.native(
            g.nx, g.ny, g.nl, P(owner), P(mid_owner), P(via_owner),
            P(occupancy.history), P(occupancy.occ), P(occupancy.via_history), P(occupancy.via_occ),
            P(flags), spec.nid, P(layers), len(layers), P(move_base),
            float(present), float(self.via_cost * spec.via_factor),
            *box, *tbox,
            P(source_nodes), P(source_costs), len(source_nodes),
            extra_pointer, turn_pointer, guide_pointer,
            transitions[1].ctypes.data if transitions is not None else None, self.options.maximum_expansions,
            P(self._n_best), P(self._n_parent), P(self._n_heap), self._n_heap_cap,
            P(self._n_touched), self._n_touched_cap, ctypes.byref(touched_count),
            P(self._n_path), self._n_path_cap, *lattice_tables)
        touched = self._n_touched[:touched_count.value]
        self._n_best[touched] = 1e30
        for nodes, bit in marks:
            if nodes.size:
                flags[nodes] = 0
        if target_extra:
            extra[keys] = 0.0
        if corridor is not None:
            self._n_guide[corridor[1]] = float(self.options.guide_cost)
        if length < 0:
            return NotImplemented
        if length == 0:
            return None
        path = self._n_path[:length].tolist()
        return path, path[-1]

    def _move_base(self, spec) -> np.ndarray:
        """Heading-adjusted planar move costs for the kernel: [layer][move] on a
        uniform lattice, [block][layer][move] on a nested one (the costs of
        ``_nested_search_tables``)."""
        cache = self._move_cache
        if cache is None and self.nested:
            tables, _ = self._nested_search_tables()
            cache = self._move_cache = np.asarray(
                [move[3] for per_layer in tables for table in per_layer for move in table], dtype=np.float64)
        if cache is None:
            g, options = self.lattice, self.options
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

    def _turn_table(self) -> np.ndarray:
        """bend_cost times the 45-degree turns from move a to move b, at [8 * a + b]."""
        cache = self._turn_cache
        if cache is None:
            headings = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)]
            index = [headings.index((dx, dy)) for dx, dy, _ in _MOVES]
            bend = self.options.bend_cost
            cache = self._turn_cache = np.asarray(
                [bend * min(abs(a - b), 8 - abs(a - b)) for a in index for b in index], dtype=np.float64)
        return cache

    def _guide_corridor(self, spec: _NetSpec):
        """The net's global-route corridor as grid nodes (set, int64 array, xy box), or None.

        Matches the legacy corridor (detailed._inside_guide): guide segments
        on their own layer within guide_half_width_nm + guide_margin_nm, and
        pin accesses within the margin square on their layer.
        """
        if spec.name in self._guide_cache:
            return self._guide_cache[spec.name]
        routes = self.guides.get(spec.name)
        result = None
        if routes:
            g = self.lattice
            margin = self.options.guide_margin_nm
            index = {layer: i for i, layer in enumerate(g.layers)}
            parts = []

            # Window masks over each block's xs/ys slices.
            for route in routes:
                for segment in route.segments:
                    layer = index.get(segment.layer)
                    if layer is None:
                        continue
                    r = segment.guide_half_width_nm + margin
                    a, b = segment.start, segment.end
                    for block in g.blocks:
                        window = block.window(min(a.x_nm, b.x_nm) - r, min(a.y_nm, b.y_nm) - r,
                                              max(a.x_nm, b.x_nm) + r, max(a.y_nm, b.y_nm) + r)
                        ix0, iy0, ix1, iy1 = window
                        if ix0 > ix1 or iy0 > iy1:
                            continue
                        px = block.xs[ix0:ix1 + 1][None, :]
                        py = block.ys[iy0:iy1 + 1][:, None]
                        parts.append(block.window_columns(window, _segment_distance(
                            px, py, float(a.x_nm), float(a.y_nm), float(b.x_nm), float(b.y_nm)) <= r)
                            + layer * g.plane)
                for access in route.accesses:
                    layer = index.get(access.layer)
                    if layer is None:
                        continue
                    c = access.access_position
                    for block in g.blocks:
                        window = block.window(c.x_nm - margin, c.y_nm - margin, c.x_nm + margin, c.y_nm + margin)
                        ix0, iy0, ix1, iy1 = window
                        if ix0 > ix1 or iy0 > iy1:
                            continue
                        px = block.xs[ix0:ix1 + 1][None, :]
                        py = block.ys[iy0:iy1 + 1][:, None]
                        parts.append(block.window_columns(window,
                            (np.abs(px - c.x_nm) <= margin) & (np.abs(py - c.y_nm) <= margin)) + layer * g.plane)
            nodes = np.unique(np.concatenate(parts)).astype(np.int64) if parts else np.zeros(0, np.int64)
            if nodes.size:
                result = (frozenset(nodes.tolist()), nodes, g.box_of(nodes))
        self._guide_cache[spec.name] = result
        return result

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

    def _expired(self) -> bool:
        return self.deadline is not None and perf_counter() >= self.deadline

    def negotiate(self, deadline: float | None, only: set[str] | None = None):
        """PathFinder loop. Routes persist on the router between calls."""
        options = self.options
        self.deadline = deadline
        specs = sorted(self.specs, key=lambda s: (s.span, s.name))
        routes = self.routes
        present = self.present
        best_snapshot = None
        best_iteration = 0
        pending = [s.name for s in specs if only is None or s.name in only or s.name not in routes]
        by_name = {s.name: s for s in specs}
        iteration = 0
        failed: set[str] = set()
        retries = 0          # periodic retry points passed so far
        retried = False      # whether any failed net has been routed again
        for next_iteration in range(1, options.maximum_iterations + 1):
            if self._expired():
                break
            iteration = next_iteration
            emit(self.on_progress, "unified_iteration", "started", iteration=iteration, nets=len(pending))
            retried = retried or any(name in failed for name in pending)
            for name in pending:
                if self._expired():
                    break
                spec = by_name[name]
                old = routes.get(name)
                if old is not None:
                    self.occupancy.add(old, -1)
                route = self._route_net(spec, present)
                if self._expired() and not route.connected and old is not None:
                    # Cancellation must not destroy an earlier incumbent.
                    route = old
                routes[name] = route
                self.occupancy.add(route, +1)
            conflicts = self._conflicts(routes)
            # Nets that stay conflicted search a wider window: congestion
            # costs alone never widen the box, so a needed detour beyond the
            # margin would otherwise stay invisible.
            for name in pending:
                self.conflict_age[name] = self.conflict_age.get(name, 0) + 1 if name in conflicts else 0
            failed = {s.name for s in specs if s.name not in routes or not routes[s.name].connected}
            score = (len(failed) + len(conflicts), sum(r.length for r in routes.values()))
            if best_snapshot is None or score < best_snapshot[0]:
                best_snapshot = (score, dict(routes), set(conflicts))
                best_iteration = iteration
            emit(self.on_progress, "unified_iteration", "finished", iteration=iteration,
                 conflicted=len(conflicts), failed=len(failed))
            if not conflicts:
                if (options.failed_retry_before_exit and failed and not retried
                        and iteration < options.maximum_iterations):
                    # Conflicts cleared before any failed net was retried:
                    # give them one more try against the settled copper.
                    pending = [s.name for s in specs if s.name in failed]
                    emit(self.on_progress, "unified_iteration", "failed_retry",
                         iteration=iteration + 1, nets=len(pending))
                    continue
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
            retry = bool(options.failed_retry_every and iteration % options.failed_retry_every == 0
                         and (not options.failed_retry_limit or retries < options.failed_retry_limit))
            retries += retry
            pending = [s.name for s in specs if s.name in conflicts or (s.name in failed and retry)]
            if options.full_reroute_every and iteration % options.full_reroute_every == 0:
                pending = [s.name for s in specs if s.name not in failed or retry]
            if not pending:
                break
        snapshot, conflicts = (best_snapshot[1:] if best_snapshot is not None
                               else (dict(routes), self._conflicts(routes)))
        if snapshot != routes:
            for route in routes.values():
                self.occupancy.add(route, -1)
            routes.clear()
            routes.update(snapshot)
            for route in routes.values():
                self.occupancy.add(route, +1)
        for spec in specs:
            if spec.name not in routes:
                routes[spec.name] = _NetRoute([], [], set(), set(), set(), set(), False, 0.0,
                                             "time budget exhausted")
        return dict(routes), conflicts, iteration

    # -- materialisation --------------------------------------------------

    def _transition_span(self, low: int, high: int):
        key = (low, high)
        if key not in self._via_span_cache:
            g = self.lattice
            self._via_span_cache[key] = physical_via_span(self.board, g.layers[low], g.layers[high])
        return self._via_span_cache[key]

    def _via_sites(self, name: str, paths) -> dict[int, tuple | None]:
        """``_site_spans`` for every layer-change site of ``paths`` (one route)."""
        layer_of, column = self.lattice.layer_of, self.lattice.column
        changes: dict[int, tuple[int, int]] = {}
        for path in paths:
            for a, b in zip(path, path[1:]):
                la, lb = layer_of(a), layer_of(b)
                if la != lb:
                    xy = column(a)
                    low, high = changes.get(xy, (la, la))
                    changes[xy] = (min(low, la, lb), max(high, la, lb))
        return self._site_spans(name, changes)

    def _site_spans(self, name: str, changes: Mapping[int, tuple[int, int]]) -> dict[int, tuple | None]:
        """The via of each layer-change site: xy -> (from, to, technology) or None.

        ``changes`` maps a site to the lowest and highest layer index a route
        changes between there; the route gets one via per site spanning them.
        Without via technologies that is the stackup's through via. With
        them, an existing via of the net at the site that spans those layers
        is reused as it is; otherwise ``physical_via_span`` selects the span
        (through, blind or buried), then the outer span. None: no declared
        technology serves the site (commit rejects the path).
        """
        g = self.lattice
        if not self.board.stackup.via_technologies:
            return dict.fromkeys(changes, (g.layers[0], g.layers[-1], None))
        if self._board_via_sites is None:
            sites: dict[tuple[str, Point], list[Via]] = {}
            for via in self.board.vias:
                sites.setdefault((via.net, via.position), []).append(via)
            self._board_via_sites = sites
        order = {layer: i for i, layer in enumerate(g.layers)}
        result: dict[int, tuple | None] = {}
        for xy, (low, high) in changes.items():
            existing = next((via for via in self._board_via_sites.get((name, g.column_point(xy)), ())
                             if via.from_layer in order and via.to_layer in order
                             and min(order[via.from_layer], order[via.to_layer]) <= low
                             and high <= max(order[via.from_layer], order[via.to_layer])), None)
            if existing is not None:
                result[xy] = (existing.from_layer, existing.to_layer, existing.technology)
                continue
            result[xy] = self._transition_span(low, high)
        return result

    def copper(self, spec: _NetSpec, path: list[int], leads: tuple[TrackSegment, ...],
               spans: Mapping[int, tuple | None] | None = None):
        """Tracks and vias of one path, with its lead-ins.

        ``spans`` (``_via_sites`` of the whole route) gives each layer-change
        site its via; by default the sites of this path alone. A site without
        a span gets the outer span here; commit rejects it.
        """
        g = self.lattice
        if spans is None:
            spans = self._via_sites(spec.name, [path])
        tracks: list[TrackSegment] = list(leads)
        vias: list[Via] = []
        neck, region = spec.neck, spec.region
        def width_of(a: int, b: int) -> int:
            if a in region and b in region and (a in neck or b in neck or g.diagonal_cell(a, b) in spec.neck_mid):
                return spec.neck_width
            return spec.width
        if len(path) >= 2:
            run_start = path[0]
            prev = path[0]
            direction = None
            run_width = None
            for node in path[1:]:
                la, lb = g.layer_of(prev), g.layer_of(node)
                if la != lb:
                    if run_start != prev:
                        tracks.append(TrackSegment(spec.name, g.point(run_start), g.point(prev),
                                                   run_width, g.layers[la]))
                    span = spans.get(g.column(prev)) or (g.layers[0], g.layers[-1], None)
                    vias.append(Via(spec.name, g.point(prev), spec.via_size, spec.via_drill, *span))
                    run_start, prev, direction, run_width = node, node, None, None
                    continue
                step = g.delta(prev, node)
                if step == (0, 0):
                    # A portal between coincident nodes of two lattice blocks.
                    prev = node
                    continue
                width = width_of(prev, node)
                if direction is not None and (step != direction or width != run_width):
                    tracks.append(TrackSegment(spec.name, g.point(run_start), g.point(prev),
                                               run_width, g.layers[la]))
                    run_start = prev
                direction, run_width = step, width
                prev = node
            if run_start != prev:
                tracks.append(TrackSegment(spec.name, g.point(run_start), g.point(prev),
                                           run_width, g.layers[g.layer_of(prev)]))
        # A collinear neck can span several overlapping pad regions without
        # belonging wholly to any one of them. Re-cut emitted mutable copper
        # by the exact source regions before the normal exact commit check.
        tracks = list(self.clearance.breakout.neck_down(tracks))
        return tracks, list(dict.fromkeys(vias))

    def commit(self, routes: Mapping[str, _NetRoute], conflicts=frozenset()):
        """Exact check of every path; returns accepted copper, rejections, complete nets.

        Checks against ``self.clearance`` and adds the accepted copper to it,
        so the router's search model no longer matches its index afterwards:
        commit once. A shared index (route_unified) then holds this commit's
        copper for the next router. Blockers are named by net for all copper
        the index holds as mutable (this call's copper); board and locked
        copper count as fixed.
        """
        from .surface_path import _track_inside_board
        index = self.clearance
        g = self.lattice
        self.rejections: dict[str, list[tuple[Point, str, tuple[str, ...], bool]]] = {}
        accepted: dict[str, tuple[list[TrackSegment], list[Via]]] = {}
        rejected: dict[str, list[Point]] = {}
        # A connected route with nothing to add (every pad group already in its
        # plane) has no copper to check and is complete as it stands.
        complete: set[str] = {name for name, route in routes.items() if route.connected and not route.paths}
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
            spans = self._via_sites(name, route.paths)
            unserved = {xy for xy, span in spans.items() if span is None}
            for path, leads in zip(route.paths, route.leads):
                tracks, vias = self.copper(spec, path, leads, spans)
                bad: list[Point] = []
                if unserved:
                    for a, b in zip(path, path[1:]):
                        if g.layer_of(a) != g.layer_of(b) and g.column(a) in unserved:
                            bad.append(g.point(a))
                            self.rejections.setdefault(name, []).append(
                                (g.point(a), "via", (), False, "no_via_technology"))
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
                    rule = self.rules.get(name)
                    if rule and rule.require_return_vias and rule.differential_partner is None:
                        from .return_paths import covering_return_vias
                        if not covering_return_vias(self.board, rule, via):
                            bad.append(via.position)
                            self.rejections.setdefault(name, []).append(
                                (via.position, "via", (), True, "required_return_via"))
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
        if other.lattice.signature() == self.lattice.signature():
            self.occupancy.copy_history(other.occupancy)
            self.present = other.present
            self.conflict_age = dict(other.conflict_age)

    def penalise_points(self, name: str, points: list[Point], amount: float) -> None:
        g = self.lattice
        hist = self.occupancy.history
        via_hist = self.occupancy.via_history
        for point in points:
            columns = (g.columns_around(point, _disc_offsets_cached(2.0)) if not self.nested else
                       np.union1d(g.columns_within(point, 2.0 * self.pitch), [g.nearest(0, point)]))
            for column in columns.tolist():
                via_hist[column] += amount
                for node in g.column_nodes(column):
                    hist[node] += amount


def _open_count(board: PhysicalBoard, nets, accepted, plane_nets, complete) -> tuple[int, int]:
    """(open signal connections, incomplete plane nets) with ``accepted`` copper.

    A plane net is incomplete unless it is in ``complete``: a complete plane
    net may have no new copper at all (every pad group already in its fill).
    """
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
    planes = sum(1 for net in nets if net.name in plane_nets and net.name not in complete)
    return opens, planes


def _with(board: PhysicalBoard, accepted, *, without=frozenset()) -> PhysicalBoard:
    """``board`` plus the accepted copper of every net not in ``without``."""
    names = sorted(name for name in accepted if name not in without)
    return replace(board,
        tracks=(*board.tracks, *(t for name in names for t in accepted[name][0])),
        vias=(*board.vias, *(v for name in names for v in accepted[name][1])))


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


def _same_but_copper(board: PhysicalBoard, other: PhysicalBoard) -> bool:
    """Whether two boards differ at most in their tracks and vias."""
    from dataclasses import fields
    return board is other or all(
        getattr(board, item.name) is getattr(other, item.name)
        or getattr(board, item.name) == getattr(other, item.name)
        for item in fields(board) if item.name not in ("tracks", "vias"))


def _index_holds(index: RoutingClearanceIndex, board: PhysicalBoard) -> bool:
    """Whether ``index`` answers legality queries as a fresh index of ``board``.

    True when the index's own board differs from ``board`` only in copper and
    its additions (its board's copper, locked, then mutable additions) are
    ``board``'s copper as multisets. The exact legality checks read neither
    lock flags nor insertion order; only blocker naming (mutable additions
    are named by net, board copper counts as fixed) and snapshot-local
    occurrence labels differ.
    """
    from collections import Counter
    if not _same_but_copper(index.board, board):
        return False
    tracks, vias = Counter(), Counter()
    for item, _ in index.additions():
        (vias if isinstance(item, Via) else tracks)[item] += 1
    return tracks == Counter(board.tracks) and vias == Counter(board.vias)


class _SharedIndex:
    """One exact index of a base board plus a chosen part of this call's copper.

    ``held`` is that copper per net, exactly what the index holds as mutable
    additions on top of its base. ``added`` records what a commit into the
    index accepted; ``hold`` switches to another selection by ripping out
    (``RoutingClearanceIndex.remove``) and re-adding only the nets whose
    copper differs. Repair rounds and the pour re-check use it instead of
    building an index per router; copper outside ``held`` (the board's, or
    bus copper frozen into the base) is never touched.
    """

    def __init__(self, index: RoutingClearanceIndex) -> None:
        self.index = index
        self.held: dict[str, tuple[tuple[TrackSegment, ...], tuple[Via, ...]]] = {}
        self.frozen: set[str] = set()

    def added(self, more) -> None:
        for name, (tracks, vias) in more.items():
            old_tracks, old_vias = self.held.get(name, ((), ()))
            self.held[name] = (old_tracks + tuple(tracks), old_vias + tuple(vias))

    def freeze(self) -> None:
        """Make the copper held now part of the base (it is never ripped)."""
        self.frozen |= self.held.keys()
        self.held = {}

    def hold(self, target) -> None:
        for name in sorted(self.held.keys() | target.keys()):
            old_tracks, old_vias = self.held.get(name, ((), ()))
            tracks, vias = target.get(name, ((), ()))
            if old_tracks == tuple(tracks) and old_vias == tuple(vias):
                continue
            if name in self.frozen:
                raise AssertionError(f"net {name} has frozen copper in the shared index")
            self.index.remove(name)
            if self.index.owned_count(name):
                raise AssertionError(f"ripped net {name} keeps mutable copper in the shared index")
            for track in tracks:
                self.index.add_track(track)
            for via in vias:
                self.index.add_via(via)
        self.held = {name: (tuple(tracks), tuple(vias)) for name, (tracks, vias) in target.items()
                     if tracks or vias}


def _recheck_pours(board, nets, plane_nets, accepted, complete, options, deadline, on_progress,
                   required_accesses=None, shared: _SharedIndex | None = None, guides=None):
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

    ``shared`` (holding ``board`` plus ``accepted``) serves every step's
    router and commit: rip-ups and rejected steps are removed from it, and it
    holds the returned copper on return.
    """
    before = _pour_estimate(board, accepted)
    routed_planes = {net.name for net in nets if net.name in plane_nets}
    cut = {name: pads for name, pads in before.open_pads.items() if name in routed_planes}
    report = {"cut_off": sum(map(len, cut.values())),
              "pads": sorted(p for pads in cut.values() for p in pads)[:12], "steps": [], "iterations": 0}
    if not cut:
        return accepted, complete, report
    signal_before = _open_count(board, nets, accepted, plane_nets, complete)[0]
    plane_list = [net for net in nets if net.name in cut]

    def keep(candidate, estimate, label, candidate_complete):
        signal = _open_count(board, nets, candidate, plane_nets, candidate_complete)[0]
        ok = estimate.opens < current[1].opens and signal <= signal_before
        report["steps"].append(f"{label}: pour opens {current[1].opens}->{estimate.opens}, "
                               f"signal opens {signal_before}->{signal}" + ("" if ok else " (not kept)"))
        return ok

    def router(copper, route_nets, **extra):
        """A router over ``board`` plus ``copper``, on the shared index if any."""
        if shared is not None:
            shared.hold(copper)
        return _Router(_with(board, copper), options, route_nets, on_progress,
                       required_accesses=required_accesses, guides=guides,
                       clearance=shared.index if shared is not None else None, **extra)

    def commit(step_router, routes, conflicts):
        result = step_router.commit(routes, conflicts)
        if shared is not None:
            shared.added(result[0])
        return result

    current = (accepted, before)
    # Step 1: plane nets only, everything else fixed.
    step = router(accepted, plane_list)
    routes, conflicts, used = step.negotiate(deadline)
    report["iterations"] += used
    more, _, _ = commit(step, routes, conflicts)
    candidate = _merge(accepted, more)
    estimate = _pour_estimate(board, candidate)
    if keep(candidate, estimate, "plane reroute", complete):
        current = (candidate, estimate)
    still = {name: pads for name, pads in current[1].open_pads.items() if name in routed_planes}
    if still and (deadline is None or perf_counter() <= deadline):
        # Step 2: rip up the crowd around the remaining pads; plane copper first.
        pads = frozenset(PadReference(*pad.rsplit(".", 1)) for group in still.values() for pad in group)
        focus = [PhysicalNet(name, tuple(PadReference(*pad.rsplit(".", 1)) for pad in group))
                 for name, group in sorted(still.items())]
        crowd = {name for name in _neighbour_nets(board, focus, current[0], options.repair_neighbour_reach_nm)
                 if name not in plane_nets}
        ripped = {name: value for name, value in current[0].items() if name not in crowd}
        first = router(ripped, [net for net in nets if net.name in still], explicit_pads=pads)
        routes, conflicts, used = first.negotiate(deadline)
        report["iterations"] += used
        plane_more, plane_rejected, _ = commit(first, routes, conflicts)
        report["explicit_route"] = {
            name: {"copper": [len(part) for part in plane_more.get(name, ((), ()))],
                   "reason": (routes[name].reason if name in routes else "not routed"),
                   "rejected": _rejection_text(first, name) if name in plane_rejected else ""}
            for name in still}
        candidate = _merge(ripped, plane_more)
        second = router(candidate, [net for net in nets if net.name in crowd])
        routes, conflicts, used = second.negotiate(deadline)
        report["iterations"] += used
        signal_more, _, signal_complete = commit(second, routes, conflicts)
        candidate = _merge(candidate, signal_more)
        estimate = _pour_estimate(board, candidate)
        candidate_complete = (complete - crowd) | set(signal_complete)
        if keep(candidate, estimate, f"explicit copper, {len(crowd)} net(s) ripped up", candidate_complete):
            current = (candidate, estimate)
            complete = candidate_complete
    report["adopted"] = current[0] is not accepted
    if shared is not None:
        shared.hold(current[0])
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


def choose_pitch(board: PhysicalBoard, default_nm: int = DEFAULT_PITCH_NM) -> int:
    """0.25 mm, or the coarsest finer pitch that puts nodes on every ball and
    on the channel centre lines of each full-grid ball array (BGA).

    Only fine_regions="off" uses it. Each array is judged as placed: every
    transformed ball and every adjacent-ball midpoint must be a lattice
    point (``fine_regions.array_alignment``), so a rotated array is not
    called aligned because its first ball is. An array that no candidate
    aligns leaves the pitch unchanged.
    """
    from .fine_regions import array_alignment
    from .package_patterns import circular_array
    pitch = default_nm
    for pose in board.placements:
        try:
            array = circular_array(board, pose.reference)
        except ValueError:
            array = None
        if array is None:
            continue
        aligned = array_alignment(array, pose)
        for candidate in (pitch, pitch // 2, pitch // 4, nm_from_mm("0.05")):
            if candidate > 0 and aligned(candidate):
                pitch = candidate
                break
    return pitch


def fine_region_plan(board: PhysicalBoard, options: UnifiedRouterOptions) -> "FineRegionPlan | None":
    """The fine-patch plan the router builds its lattice from; None for "off".

    The base pitch is ``options.pitch_nm`` or 0.25 mm. ``fine_region_options``
    sets the planner's knobs; the mode sets ``dense_packages``.
    """
    if options.fine_regions == "off":
        return None
    from .fine_regions import FineRegionOptions, plan_fine_regions
    planner = replace(options.fine_region_options or FineRegionOptions(),
                      dense_packages=options.fine_regions == "dense")
    return plan_fine_regions(board, options.pitch_nm or DEFAULT_PITCH_NM, planner)


def _nearest_first(lattice, position: Point, reach: float):
    """(distance, column, x, y) of the lattice's cover of ``position`` +- ``reach``,
    nearest first; equal distances keep the cover's x-major order."""
    near = [(hypot(x - position.x_nm, y - position.y_nm), column, x, y)
            for column, x, y in lattice.cover(position.x_nm - reach, position.y_nm - reach,
                                              position.x_nm + reach, position.y_nm + reach)]
    near.sort(key=itemgetter(0))
    return near


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
# Congestion export (congestion-driven placement feedback, stage A1)

def _failed_points(router: _Router, failed: Iterable[str]) -> tuple[tuple[str, Point], ...]:
    """(net, point) where each failed net was blocked or could not be reached.

    Points are the exact commit check's rejection sites, plus the anchor
    positions of pad groups the net's route never reached: groups without
    any attach candidate, and (for a route that did not connect) groups its
    paths do not touch. Plane nets count only groups outside their plane.
    """
    specs = {spec.name: spec for spec in router.specs}
    rejections = getattr(router, "rejections", {})
    result: list[tuple[str, Point]] = []
    for name in sorted(failed):
        points = [record[0] for record in rejections.get(name, ())]
        spec, route = specs.get(name), router.routes.get(name)
        if spec is not None:
            disconnected = route is None or not route.connected
            touched = route.nodes if route is not None else set()
            for component, anchors in zip(spec.components, spec.anchors):
                if component and not disconnected:
                    continue
                if component and (any(node in touched for node, _, _ in component)
                                  or any(node in spec.plane for node, _, _ in component)):
                    continue
                points.extend(anchors)
        result.extend((name, point) for point in dict.fromkeys(points))
    return tuple(result)


def _congestion_map(router: _Router, failed: Iterable[str], tile_nm: int):
    """Per-tile contention of ``router``'s final negotiated state.

    Demand per tile, summed over every node of every layer in it:
    ``max(0, occ - 1)`` (claims beyond the first; via sites count as one more
    layer) plus ``history / history_increment``, so one unit is about one
    conflicting net on one node for one iteration. Seeded ball-field history
    is a routing preference, not contention, and is left out.

    Capacity per tile: the free nodes (``_FREE``) of the default track
    raster (board default width, minimum clearance) on every signal layer,
    divided by the pitches one track lane occupies,
    ``(default width + clearance) / pitch``: default-width tracks that fit
    across the tile, times the tile edge in pitches. Outside board material
    every node is foreign, so capacity is 0 there.
    Every quantity is gathered per column (via site) and binned by the
    column's board position, so the map works on any lattice. On a nested
    lattice each column counts with its area in base cells,
    ``(step / base pitch) ** 2``: a fine patch neither looks k**2 times
    roomier nor k**2 times more contended than the same area at the base.
    """
    g = router.lattice
    occupancy = router.occupancy
    nl, plane = g.nl, g.plane
    occ = np.asarray(occupancy.occ, dtype=np.int64).reshape(nl, plane)
    node_demand = np.maximum(occ - 1, 0).sum(axis=0).astype(np.float64)
    node_demand += np.maximum(np.asarray(occupancy.via_occ, dtype=np.int64) - 1, 0)
    increment = router.options.history_increment
    if increment > 0:
        history = np.asarray(occupancy.history, dtype=np.float64).reshape(nl, plane).sum(axis=0)
        for _, columns, cost in router._seeded_history:
            history[columns] -= cost
        history += np.asarray(occupancy.via_history, dtype=np.float64)
        node_demand += np.maximum(history, 0.0) / increment
    weight = None
    if isinstance(g, NestedLattice):
        tx = (g.column_x - g.x0) // tile_nm
        ty = (g.column_y - g.y0) // tile_nm
        tiles_x, tiles_y = int(tx.max()) + 1, int(ty.max()) + 1
        tile_ids = ty * tiles_x + tx
        weight = g.column_weight()
        node_demand = node_demand * weight
    else:
        tx = (np.arange(g.nx, dtype=np.int64) * g.pitch) // tile_nm
        ty = (np.arange(g.ny, dtype=np.int64) * g.pitch) // tile_nm
        tiles_x, tiles_y = int(tx[-1]) + 1, int(ty[-1]) + 1
        tile_ids = (ty[:, None] * tiles_x + tx[None, :]).ravel()
    count = tiles_x * tiles_y
    demand = np.bincount(tile_ids, weights=node_demand, minlength=count).reshape(tiles_y, tiles_x)
    width, clearance = router.default_width, router.board.rules.minimum_clearance_nm
    owner = router.rasters.track_arrays(width, clearance)[0].reshape(nl, plane)
    signal = set(routing_layers(router.board, "", None))
    layers = [i for i, layer in enumerate(g.layers) if layer in signal]
    free = (owner[layers] == _FREE).sum(axis=0) if layers else np.zeros(plane)
    lane = (width + clearance) / g.pitch
    lanes = free / lane
    if weight is not None:
        lanes = lanes * weight
    capacity = np.bincount(tile_ids, weights=lanes, minlength=count).reshape(tiles_y, tiles_x)
    from .congestion_feedback import CongestionMap
    return CongestionMap(
        tile_nm=tile_nm, x0_nm=int(g.x0), y0_nm=int(g.y0), nx=tiles_x, ny=tiles_y,
        demand=demand, capacity=capacity, source="unified_router",
        failed_points=_failed_points(router, failed))


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
    required_accesses=None,
) -> DetailedRoutingResult:
    """Route all ordinary nets with one negotiated-congestion loop.

    ``escape_seeder(board, net_names)`` may return a board with checked pad
    escapes for nets that failed; repair rounds use it before rerouting.

    ``required_accesses`` are immutable, verified package ports for local
    transactions. Pad and fixed-copper alternatives cannot bypass these ports.
    Stale witnesses fail instead of being silently treated as ordinary pads.

    Signature-compatible with :func:`pcbir.detailed.route_detailed`.
    Existing copper is fixed; zone nets are deferred to fill.
    ``fanout_accesses``, ``fanout_created_vias`` and ``fanout_created_tracks``
    are only recorded in the replay state. A ``fanout_accesses`` entry that is
    not also passed (unchanged) in ``required_accesses`` raises ValueError.
    """

    if fanout_accesses and any(required_accesses is None or pad not in required_accesses
                               or required_accesses[pad] != access
                               for pad, access in fanout_accesses.items()):
        # Pad launches reach this router only as required_accesses (verified,
        # immutable ports); silently ignoring other fanout accesses would
        # drop the caller's package exits.
        raise ValueError("unified router does not consume verified fanout accesses; pass required_accesses")
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
            "required_accesses": required_accesses,
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
            and (options.direct or rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL)
            and not connectivity.net_connected(net)]
    already = [net for net in board.nets
               if len(net.pads) >= 2 and net.name not in zone_nets
               and (only_nets is None or net.name in only_nets)
               and (options.direct or rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL)
               and connectivity.net_connected(net)]
    original_board, all_nets = board, nets
    # One exact index for this call's commits and repairs (share_commit_index).
    shared: _SharedIndex | None = None
    bus_accepted: dict = {}
    bus_complete: set[str] = set()
    bus_names = {name for group in board.match_groups for name in group.nets} & {net.name for net in nets}
    if options.buses_first and bus_names and len(bus_names) < len(nets):
        # A bus needs a shared corridor; negotiated together with every other
        # net it loses it piecemeal. Its members route first, among themselves.
        bus_options = replace(options, length_priority=True) if options.bus_length_budget else options
        bus_router = _Router(board, bus_options, [net for net in nets if net.name in bus_names], on_progress,
                             required_accesses=required_accesses, guides=global_route)
        bus_routes, bus_conflicts, used = bus_router.negotiate(deadline)
        bus_accepted, _, bus_complete = bus_router.commit(bus_routes, bus_conflicts)
        if options.share_commit_index:
            shared = _SharedIndex(bus_router.clearance)
            shared.added(bus_accepted)
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
        if shared is not None:
            # Kept bus copper is fixed for the other nets, as on ``board`` below.
            shared.hold(bus_accepted)
            shared.freeze()
        emit(on_progress, "unified_buses", "finished", nets=len(bus_names), complete=len(bus_complete),
             over_length=len(over_length), iterations=used)
        if bus_accepted:
            board = _with(board, bus_accepted)
            nets = [net for net in nets if net.name not in bus_complete]
    emit(on_progress, "unified_setup", "started", nets=len(nets))
    router = _Router(board, options, nets, on_progress, required_accesses=required_accesses,
                     guides=global_route, clearance=shared.index if shared is not None else None)
    if options.share_commit_index and shared is None:
        shared = _SharedIndex(router.clearance)
    if options.export_congestion and options.congestion_tile_nm < router.pitch:
        raise ValueError("congestion_tile_nm must be at least the router pitch")
    # The base block's shape; on a nested lattice also the patches and node counts.
    lattice = router.lattice
    setup = {"nets": len(router.specs), "grid": [lattice.nx, lattice.ny, lattice.nl]}
    if router.nested:
        setup["patches"] = [{"x0": block.x0 / 1e6, "y0": block.y0 / 1e6, "x1": block.x1 / 1e6,
                             "y1": block.y1 / 1e6, "factor": block.factor, "nodes": block.plane * lattice.nl}
                            for block in lattice.blocks[1:]]
        setup["nodes"] = lattice.size
        setup["uniform_nodes_at_finest"] = router.fine_plan.uniform_nodes_at_finest
    emit(on_progress, "unified_setup", "finished", **setup, seconds=round(perf_counter() - started, 2))
    routes, conflicts, iterations = router.negotiate(deadline)
    commit_started = perf_counter()
    accepted, rejected, complete = router.commit(routes, conflicts)
    if shared is not None:
        shared.added(accepted)
    emit(on_progress, "unified_commit", "finished", round=1,
         accepted=len(accepted), rejected=len(rejected), conflicted=len(conflicts),
         seconds=round(perf_counter() - commit_started, 3))
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
        crowd = _neighbour_nets(board, remaining, work, options.repair_neighbour_reach_nm)
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
        # The shared index rips the crowd out (remove) instead of being rebuilt.
        share = shared is not None
        if share:
            shared.hold(work)
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
            # Seeded escapes join the shared index only if the seeder just
            # added copper; any other change gets a fresh index of its board.
            share = share and _same_but_copper(seeded, partial) and not (
                Counter(partial.tracks) - Counter(seeded.tracks)) and not (
                Counter(partial.vias) - Counter(seeded.vias))
            if share:
                shared.hold(work)
            partial = seeded
        fixed = explicit_copper_connectivity(partial)
        remaining = [net for net in remaining if not fixed.net_connected(net) or net.name in plane_nets]
        repair = _Router(partial, options, remaining, on_progress, required_accesses=required_accesses,
                         clearance=shared.index if share else None, guides=global_route)
        repair.history_seed(router)
        more_routes, more_conflicts, used = repair.negotiate(deadline)
        more, more_rejected, more_complete = repair.commit(more_routes, more_conflicts)
        if share:
            shared.added(more)
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
        round_started = perf_counter()
        baseline = _open_count(board, nets, accepted, plane_nets, complete)
        best_attempt = None
        for seed in ((False, True) if escape_seeder is not None else (False,)):
            if deadline is not None and perf_counter() >= deadline:
                emit(on_progress, "unified_repair", "skipped", round=round_index + 1,
                     seeded=seed, reason="time_budget")
                break
            attempt = repair_attempt(seed)
            if attempt is None:
                continue
            iterations += attempt[4][3]
            score = _open_count(board, nets, attempt[0], plane_nets, attempt[1])
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
        if shared is not None:
            # Back to the adopted copper (attempts left their own in the index).
            shared.hold(accepted)
        emit(on_progress, "unified_commit", "finished", round=round_index + 1, adopted=improved,
             seeded=bool(improved and seeded), seconds=round(perf_counter() - round_started, 3))
        if not improved:
            break
    if options.pour_recheck and plane_nets and (deadline is None or perf_counter() <= deadline):
        accepted, complete, recheck = _recheck_pours(
            board, nets, plane_nets, accepted, set(complete), options, deadline, on_progress,
            required_accesses=required_accesses, shared=shared, guides=global_route)
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
    # Search and tentative commit flags are not whole-net budget checks.
    # Measure final, smoothed copper exactly as DRC does, including the fixed
    # prefix and all repair additions. Reject only this call's mutable copper.
    budget_errors = {}
    for net in (*nets, *already):
        rule = rules.get(net.name)
        if rule is None or (rule.max_length_nm is None and rule.max_vias is None):
            continue
        tracks, vias = accepted.get(net.name, ((), ()))
        errors = []
        length = _net_track_length(board, net.name, tracks)
        if rule.max_length_nm is not None and length > rule.max_length_nm:
            errors.append(f"route length {length} nm exceeds {rule.max_length_nm} nm")
        count = sum(v.net == net.name for v in board.vias) + len(vias)
        if rule.max_vias is not None and count > rule.max_vias:
            errors.append(f"via count {count} exceeds {rule.max_vias}")
        if errors:
            budget_errors[net.name] = tuple(errors)
            accepted.pop(net.name, None)
    new_tracks = tuple(t for name in sorted(accepted) for t in accepted[name][0])
    new_vias = tuple(v for name in sorted(accepted) for v in accepted[name][1])
    all_tracks = (*board.tracks, *new_tracks)
    all_vias = (*board.vias, *new_vias)
    metadata = dict(board.metadata)
    routed_names = (set(complete) | {net.name for net in already}) - budget_errors.keys()
    unrouted = [net.name for net in (*nets, *already) if net.name not in routed_names]
    success = not unrouted
    congestion = None
    if options.export_congestion:
        export_started = perf_counter()
        congestion = _congestion_map(router, unrouted, options.congestion_tile_nm)
        utilisation = congestion.utilisation()
        emit(on_progress, "unified_congestion", "finished", tiles=int(congestion.nx * congestion.ny),
             hot_tiles=int(np.count_nonzero((utilisation > 1) & (np.asarray(congestion.demand) > 0))),
             failed_points=len(congestion.failed_points),
             seconds=round(perf_counter() - export_started, 3))
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
        if net.name in budget_errors:
            diagnostics = budget_errors[net.name]
        elif net.name not in routed_names:
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
    # Every result is connected when nothing is unrouted.
    status = DetailedRoutingStatus.SUCCESS if success else DetailedRoutingStatus.PARTIAL
    return DetailedRoutingResult(status, routed, tuple(results), metrics, len(board.tracks),
                                 len(board.vias), global_route.routing_fingerprint, fingerprint,
                                 congestion=congestion)


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
                         only_nets=saved["only_nets"], on_progress=on_progress,
                         required_accesses=saved.get("required_accesses"))
