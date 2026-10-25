"""Routability-driven placement feedback (routing refactor plan, section 4).

One congestion map per tile lattice drives the placement feedback loop: the
unified router exports its own contention (``source="unified_router"``), and
:func:`estimate_demand` gives a cheap analytic estimate (``source="rudy"``).
:func:`propose_moves` moves parts on failed nets and parts on hot tiles down
the utilisation gradient, and :func:`screen` keeps the few proposals that
the estimate plus ``route_global`` rate best, so only those pay for a full
reroute of the pipeline tail.

Units. Demand and capacity are both in *lane cells per tile*: an area divided
by ``lane_nm ** 2``, where ``lane_nm`` is the default track width plus the
minimum clearance. A track of length ``L`` and lane width ``lane`` occupies
``L * lane / lane_nm ** 2`` lane cells, so a default-width track straight
across a tile counts ``tile_nm / lane_nm``. An empty tile on one signal layer
has capacity ``(tile_nm / lane_nm) ** 2``: room for ``tile_nm / lane_nm``
parallel tracks of that length. Utilisation (demand / capacity) of 1 is a
tile whose free area is fully used by lanes; above 1 is overflow.

The estimate never replaces exact legality: it only orders candidate poses,
and every pose still passes ``placement_solution_is_legal``.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field, replace
from decimal import Decimal
from math import ceil, hypot
from typing import Iterable

import numpy as np

from .clusters import move_placement_unit
from .physical import CopperLayer, PadKind, PhysicalBoard, Placement, Point, nm_from_mm
from .placement import (
    PlacementPlannerOptions,
    _allowed_orientations,
    placement_solution_is_legal,
    resolved_copper_keepouts,
    transformed_footprint_polygon,
    transformed_local_point,
)
from .progress import ProgressCallback, emit
from .routing import GlobalRouterOptions, GlobalRoutingResult, route_global

#: Utilisation reported for a tile with demand but no capacity (no material,
#: or fully covered by pads/keepouts on every signal layer). Large enough to
#: rank above any real overflow, finite so sums and fits stay finite.
UTILISATION_SENTINEL = 1.0e6

#: Default estimate lattice pitch; matches the unified router's default
#: ``congestion_tile_nm`` so the two maps combine on the same lattice.
DEFAULT_TILE_NM = nm_from_mm("1")

_SUBSAMPLES = 4  # capacity sampling points per tile side (4 x 4 per tile)


def _readonly(array, shape: tuple[int, int], name: str) -> np.ndarray:
    result = np.array(array, dtype=np.float64, copy=True)
    if result.shape != shape:
        raise ValueError(f"congestion {name} shape {result.shape} does not match lattice {shape}")
    result.setflags(write=False)
    return result


@dataclass(frozen=True, slots=True)
class CongestionMap:
    """Per-tile routing demand against capacity on a fixed square lattice.

    Tile ``(ix, iy)`` covers ``[x0 + ix * tile, x0 + (ix + 1) * tile)`` by the
    same in y. Arrays are indexed ``[iy, ix]``, copied to float64 and frozen.
    """

    tile_nm: int
    x0_nm: int
    y0_nm: int
    nx: int
    ny: int
    demand: np.ndarray        # (ny, nx) float64: routing demand per tile, summed over layers
    capacity: np.ndarray      # (ny, nx) float64: track-equivalents per tile; 0 outside material
    source: str               # "unified_router" | "rudy" | "combined"
    failed_points: tuple[tuple[str, Point], ...] = ()   # (net, where it was blocked/unreachable)

    def __post_init__(self) -> None:
        if self.tile_nm <= 0 or self.nx <= 0 or self.ny <= 0:
            raise ValueError("congestion lattice requires a positive tile and tile counts")
        shape = (self.ny, self.nx)
        object.__setattr__(self, "demand", _readonly(self.demand, shape, "demand"))
        object.__setattr__(self, "capacity", _readonly(self.capacity, shape, "capacity"))
        object.__setattr__(self, "failed_points", tuple(
            (str(net), point) for net, point in self.failed_points))

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, CongestionMap):
            return NotImplemented
        return (self._lattice() == other._lattice() and self.source == other.source
                and self.failed_points == other.failed_points
                and np.array_equal(self.demand, other.demand)
                and np.array_equal(self.capacity, other.capacity))

    def __hash__(self) -> int:
        return hash((self._lattice(), self.source, self.failed_points,
                     self.demand.tobytes(), self.capacity.tobytes()))

    def _lattice(self) -> tuple[int, int, int, int, int]:
        return (self.tile_nm, self.x0_nm, self.y0_nm, self.nx, self.ny)

    def utilisation(self) -> np.ndarray:
        """Demand / capacity; ``UTILISATION_SENTINEL`` where only capacity is 0, 0 where both are."""
        result = np.zeros((self.ny, self.nx), dtype=np.float64)
        open_ = self.capacity > 0
        np.divide(self.demand, self.capacity, out=result, where=open_)
        result[~open_ & (self.demand > 0)] = UTILISATION_SENTINEL
        return result

    def tile_of(self, point: Point) -> tuple[int, int]:
        """The ``(ix, iy)`` tile holding ``point``, clipped to the lattice edge."""
        ix = (point.x_nm - self.x0_nm) // self.tile_nm
        iy = (point.y_nm - self.y0_nm) // self.tile_nm
        return (min(max(int(ix), 0), self.nx - 1), min(max(int(iy), 0), self.ny - 1))

    def tile_bounds(self, ix: int, iy: int) -> tuple[int, int, int, int]:
        """``(x0, y0, x1, y1)`` in nanometres of tile ``(ix, iy)``."""
        if not (0 <= ix < self.nx and 0 <= iy < self.ny):
            raise IndexError(f"tile ({ix}, {iy}) is outside the {self.nx} x {self.ny} lattice")
        x = self.x0_nm + ix * self.tile_nm
        y = self.y0_nm + iy * self.tile_nm
        return (x, y, x + self.tile_nm, y + self.tile_nm)

    def value_at(self, point: Point) -> float:
        """Utilisation of the tile holding ``point`` (clipped like :meth:`tile_of`)."""
        ix, iy = self.tile_of(point)
        return float(self.utilisation()[iy, ix])

    def gradient_at(self, point: Point, radius_tiles: int = 2) -> tuple[float, float]:
        """Unit ``(dx, dy)`` of decreasing utilisation near ``point``; ``(0, 0)`` when flat.

        Least-squares plane fit of utilisation over the ``(2r+1)^2`` window
        (clipped at the lattice edge). Tiles without capacity are left out of
        the fit, so the direction never points off the board only because no
        copper can be there. Sentinel tiles have no capacity, so they never
        enter the fit either.
        """
        if radius_tiles < 1:
            raise ValueError("gradient radius must be at least one tile")
        cx, cy = self.tile_of(point)
        x0, x1 = max(0, cx - radius_tiles), min(self.nx, cx + radius_tiles + 1)
        y0, y1 = max(0, cy - radius_tiles), min(self.ny, cy + radius_tiles + 1)
        capacity = self.capacity[y0:y1, x0:x1]
        mask = capacity > 0
        if mask.sum() < 3:
            return (0.0, 0.0)
        values = np.zeros(capacity.shape, dtype=np.float64)
        np.divide(self.demand[y0:y1, x0:x1], capacity, out=values, where=mask)
        gy, gx = np.mgrid[y0:y1, x0:x1]
        dx = (gx - cx)[mask].astype(np.float64)
        dy = (gy - cy)[mask].astype(np.float64)
        u = values[mask]
        design = np.column_stack((np.ones_like(dx), dx, dy))
        if np.linalg.matrix_rank(design) < 3:
            return (0.0, 0.0)
        (_, slope_x, slope_y), *_ = np.linalg.lstsq(design, u, rcond=None)
        norm = hypot(float(slope_x), float(slope_y))
        scale = max(1.0, float(np.abs(u).max()))
        if norm <= 1e-9 * scale:
            return (0.0, 0.0)
        return (-float(slope_x) / norm, -float(slope_y) / norm)

    def hot_tiles(self, limit: int) -> tuple[tuple[int, int, float], ...]:
        """Up to ``limit`` tiles with positive utilisation as ``(ix, iy, u)``, hottest first, ties by ``(iy, ix)``."""
        if limit <= 0:
            return ()
        values = self.utilisation()
        iy, ix = np.nonzero(values > 0)
        if not len(iy):
            return ()
        order = np.lexsort((ix, iy, -values[iy, ix]))[:limit]
        return tuple((int(ix[k]), int(iy[k]), float(values[iy[k], ix[k]])) for k in order)

    def combined(self, other: "CongestionMap", weight: float) -> "CongestionMap":
        """``self.demand + weight * other.demand`` on ``self.capacity``; same lattice required.

        Capacity stays this map's: the first map is the authority on what is
        free (the router's raster sees real obstacles), the second only adds
        demand. ``failed_points`` keep this map's order, then the other's new ones.
        """
        if self._lattice() != other._lattice():
            raise ValueError(f"congestion lattices differ: {self._lattice()} != {other._lattice()}")
        if not np.isfinite(weight):
            raise ValueError("congestion combination weight must be finite")
        seen = set(self.failed_points)
        failed = list(self.failed_points)
        for item in other.failed_points:
            if item not in seen:
                seen.add(item)
                failed.append(item)
        return CongestionMap(
            tile_nm=self.tile_nm, x0_nm=self.x0_nm, y0_nm=self.y0_nm, nx=self.nx, ny=self.ny,
            demand=self.demand + float(weight) * other.demand, capacity=self.capacity,
            source="combined", failed_points=tuple(failed))

    def to_dict(self) -> dict:
        """Small report block: lattice, the 32 hottest tiles and failed points."""
        return {
            "source": self.source,
            "lattice": {"tile_nm": self.tile_nm, "x0_nm": self.x0_nm, "y0_nm": self.y0_nm,
                        "nx": self.nx, "ny": self.ny},
            "hot_tiles": [{"ix": ix, "iy": iy, "utilisation": round(value, 6)}
                          for ix, iy, value in self.hot_tiles(32)],
            "failed_points": [{"net": net, "x_nm": point.x_nm, "y_nm": point.y_nm}
                              for net, point in self.failed_points],
        }

    def to_json(self) -> str:
        """:meth:`to_dict` as sorted-key JSON text (the repository's report convention)."""
        return json.dumps(self.to_dict(), indent=2, sort_keys=True) + "\n"


# ---------------------------------------------------------------------------
# RUDY estimate


def _lattice(board: PhysicalBoard, tile_nm: int) -> tuple[int, int, int, int]:
    vertices = board.outline.vertices
    x0 = min(p.x_nm for p in vertices)
    y0 = min(p.y_nm for p in vertices)
    x1 = max(p.x_nm for p in vertices)
    y1 = max(p.y_nm for p in vertices)
    return x0, y0, max(1, ceil((x1 - x0) / tile_nm)), max(1, ceil((y1 - y0) / tile_nm))


def _overlap(edges: np.ndarray, low: float, high: float) -> np.ndarray:
    return np.clip(np.minimum(edges[1:], high) - np.maximum(edges[:-1], low), 0.0, None)


def _signal_layers(board: PhysicalBoard) -> tuple[CopperLayer, ...]:
    from .routing_layers import routing_layers
    # No net owns the empty name, so this is every layer that is not a
    # dedicated plane for some net, i.e. the layers ordinary signals can use.
    return routing_layers(board, "")


def _pitch_estimates(board: PhysicalBoard) -> dict[str, float]:
    """Per footprint: nearest pad-centre spacing (one pad: its larger side)."""
    result: dict[str, float] = {}
    for name, footprint in board.footprints.items():
        pads = [pad for pad in footprint.pads if pad.kind is not PadKind.APERTURE]
        if len(pads) >= 2:
            xy = np.array([(pad.position.x_nm, pad.position.y_nm) for pad in pads], dtype=np.float64)
            distance = np.hypot(xy[:, None, 0] - xy[None, :, 0], xy[:, None, 1] - xy[None, :, 1])
            distance[distance == 0] = np.inf
            nearest = float(distance.min())
            if np.isfinite(nearest):
                result[name] = nearest
                continue
        result[name] = float(max((max(pad.size.width_nm, pad.size.height_nm) for pad in pads), default=0))
    return result


def estimate_demand(
    board: PhysicalBoard, tile_nm: int, *, nets: Iterable[str] | None = None,
    routing_layers: Iterable[CopperLayer] | None = None,
    lattice: tuple[int, int, int, int] | None = None,
) -> CongestionMap:
    """RUDY wire density plus pin density against free area per signal layer.

    Demand: every net with at least two pads (and in ``nets`` when given;
    deferred zone nets excluded) spreads ``(W + H) * lane`` of wire area
    uniformly over its pad bounding box grown by half a tile (sides at least
    one tile), where ``lane`` is its rule width plus clearance (default width
    plus minimum clearance without a rule). Each pad on a net adds a stub of
    ``lane * pitch`` (pitch: nearest pad spacing in its footprint, at most one
    tile) to its tile; zone-net pads keep this stub (their tap). RUDY demand
    is dropped in tiles with no board material. Capacity: per signal layer
    (``routing_layers`` when given, else every layer that is not a dedicated
    plane), material area sampled at 4 x 4 points per tile minus pads on that
    layer and track-blocking copper keepouts. Both are divided by the default
    ``lane_nm ** 2`` (see the module docstring for units). ``lattice``
    (``x0_nm, y0_nm, nx, ny``) puts the estimate on another map's lattice,
    e.g. the unified router's, instead of the outline's bounding box.
    """
    from .unified_router import _polygon_inside, _spine_distance
    from .drc import placed_pad_shape
    from .zone_geometry import deferred_zone_nets

    if tile_nm <= 0:
        raise ValueError("estimate tile must be positive")
    x0, y0, nx, ny = _lattice(board, tile_nm) if lattice is None else (int(v) for v in lattice)
    if nx <= 0 or ny <= 0:
        raise ValueError("estimate lattice requires positive tile counts")
    lane_nm = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm
    lane_area = float(lane_nm) ** 2
    rules = {rule.net: rule for rule in board.net_routing_rules}

    def net_lane(name: str) -> float:
        rule = rules.get(name)
        if rule is None or rule.width_nm is None:
            return float(lane_nm)
        return float(rule.width_nm + (rule.clearance_nm or board.rules.minimum_clearance_nm))

    # Capacity raster: 4 x 4 sample points per tile.
    sub = tile_nm / _SUBSAMPLES
    sxs = x0 + sub * (np.arange(nx * _SUBSAMPLES) + 0.5)
    sys_ = y0 + sub * (np.arange(ny * _SUBSAMPLES) + 0.5)
    px, py = np.meshgrid(sxs, sys_)
    material = _polygon_inside(board.outline.vertices, px, py)
    for cutout in board.outline.cutouts:
        material &= ~_polygon_inside(cutout.vertices, px, py)
    layers = tuple(routing_layers) if routing_layers is not None else _signal_layers(board)
    free = {layer: material.copy() for layer in layers}

    def window(xmin: float, ymin: float, xmax: float, ymax: float):
        i0 = max(0, int((xmin - x0) // sub)); i1 = min(px.shape[1], int((xmax - x0) // sub) + 1)
        j0 = max(0, int((ymin - y0) // sub)); j1 = min(px.shape[0], int((ymax - y0) // sub) + 1)
        return (slice(j0, j1), slice(i0, i1)) if i0 < i1 and j0 < j1 else None

    net_of_pad = {(pad.component, pad.pad): net.name for net in board.nets for pad in net.pads}
    pitches = _pitch_estimates(board)
    zone_nets = deferred_zone_nets(board)
    selected = None if nets is None else set(nets)
    demand = np.zeros((ny, nx), dtype=np.float64)
    pad_points: dict[tuple[str, str], list[Point]] = {}
    for placement in board.placements:
        footprint = board.footprints[placement.footprint]
        side_layer = CopperLayer.BACK if placement.side.value == "back" else CopperLayer.FRONT
        for pad in footprint.pads:
            if pad.kind is PadKind.APERTURE:
                continue
            position = transformed_local_point(placement, pad.position)
            pad_points.setdefault((placement.reference, pad.number), []).append(position)
            shape = placed_pad_shape(position, pad, placement)
            on = (side_layer,) if pad.kind is PadKind.SMD else layers
            reach = shape.radius_nm
            box = window(min(p.x_nm for p in shape.spine) - reach, min(p.y_nm for p in shape.spine) - reach,
                         max(p.x_nm for p in shape.spine) + reach, max(p.y_nm for p in shape.spine) + reach)
            if box is not None:
                covered = _spine_distance(shape.spine, px[box], py[box]) <= reach
                for layer in on:
                    if layer in free:
                        free[layer][box] &= ~covered
            net = net_of_pad.get((placement.reference, pad.number))
            if net is None or (selected is not None and net not in selected):
                continue
            pitch = min(pitches.get(placement.footprint, 0.0) or float(tile_nm), float(tile_nm))
            ix = min(max(int((position.x_nm - x0) // tile_nm), 0), nx - 1)
            iy = min(max(int((position.y_nm - y0) // tile_nm), 0), ny - 1)
            demand[iy, ix] += net_lane(net) * pitch / lane_area
    for keepout in resolved_copper_keepouts(board):
        if not keepout.block_tracks or not any(layer in free for layer in keepout.layers):
            continue
        outer = keepout.outline.outer.vertices
        box = window(min(p.x_nm for p in outer), min(p.y_nm for p in outer),
                     max(p.x_nm for p in outer), max(p.y_nm for p in outer))
        if box is None:
            continue
        inside = _polygon_inside(outer, px[box], py[box])
        for hole in keepout.outline.holes:
            inside &= ~_polygon_inside(hole.vertices, px[box], py[box])
        for layer in keepout.layers:
            if layer in free:
                free[layer][box] &= ~inside
    sample_area = sub * sub
    capacity = np.zeros((ny, nx), dtype=np.float64)
    for mask in free.values():
        counts = mask.reshape(ny, _SUBSAMPLES, nx, _SUBSAMPLES).sum(axis=(1, 3))
        capacity += counts * sample_area / lane_area
    has_material = material.reshape(ny, _SUBSAMPLES, nx, _SUBSAMPLES).any(axis=(1, 3))

    # RUDY: per-net bounding-box slice assignment.
    xe = x0 + tile_nm * np.arange(nx + 1, dtype=np.float64)
    ye = y0 + tile_nm * np.arange(ny + 1, dtype=np.float64)
    rudy = np.zeros((ny, nx), dtype=np.float64)
    half = tile_nm / 2
    for net in board.nets:
        if len(net.pads) < 2 or net.name in zone_nets or (selected is not None and net.name not in selected):
            continue
        points = [point for pad in net.pads for point in pad_points.get((pad.component, pad.pad), ())]
        if len(points) < 2:
            continue
        bx0 = min(p.x_nm for p in points) - half; bx1 = max(p.x_nm for p in points) + half
        by0 = min(p.y_nm for p in points) - half; by1 = max(p.y_nm for p in points) + half
        if bx1 - bx0 < tile_nm:
            middle = (bx0 + bx1) / 2; bx0, bx1 = middle - half, middle + half
        if by1 - by0 < tile_nm:
            middle = (by0 + by1) / 2; by0, by1 = middle - half, middle + half
        width, height = bx1 - bx0, by1 - by0
        density = (width + height) * net_lane(net.name) / (width * height) / lane_area
        i0 = max(0, int((bx0 - x0) // tile_nm)); i1 = min(nx, int((bx1 - x0) // tile_nm) + 1)
        j0 = max(0, int((by0 - y0) // tile_nm)); j1 = min(ny, int((by1 - y0) // tile_nm) + 1)
        if i0 >= i1 or j0 >= j1:
            continue
        ox = _overlap(xe[i0:i1 + 1], bx0, bx1)
        oy = _overlap(ye[j0:j1 + 1], by0, by1)
        rudy[j0:j1, i0:i1] += density * np.outer(oy, ox)
    demand += np.where(has_material, rudy, 0.0)
    return CongestionMap(tile_nm=int(tile_nm), x0_nm=int(x0), y0_nm=int(y0), nx=int(nx), ny=int(ny),
                         demand=demand, capacity=capacity, source="rudy")


def overflow_area(congestion: CongestionMap) -> float:
    """Sum over tiles of ``max(0, utilisation - 1)``."""
    return float(np.clip(congestion.utilisation() - 1.0, 0.0, None).sum())


def _estimate_score(congestion: CongestionMap) -> tuple[float, float]:
    """Overflow area, then the sum of squared utilisation (spread below overflow)."""
    utilisation = congestion.utilisation()
    return (float(np.clip(utilisation - 1.0, 0.0, None).sum()), float(np.square(utilisation).sum()))


# ---------------------------------------------------------------------------
# Proposals


@dataclass(frozen=True, slots=True)
class Proposal:
    """One legal pose change: the moved part, its new board and why it was proposed.

    ``delta_nm`` is the part's translation and ``rotation_degrees`` its new
    absolute rotation (unchanged for a translation). ``rank`` is the
    deterministic ordering key used by :func:`propose_moves`.
    """

    reference: str
    board: PhysicalBoard
    delta_nm: tuple[int, int]
    rotation_degrees: Decimal
    reason: str
    rank: tuple


def _snap(delta: tuple[float, float], grid_nm: int) -> tuple[int, int]:
    if grid_nm <= 0:
        return (round(delta[0]), round(delta[1]))
    snapped = [round(value / grid_nm) * grid_nm for value in delta]
    if not any(snapped) and any(delta):
        # Below one grid step: take one step along the dominant component.
        axis = 0 if abs(delta[0]) >= abs(delta[1]) else 1
        snapped[axis] = grid_nm if delta[axis] > 0 else -grid_nm
    return (int(snapped[0]), int(snapped[1]))


def _rotation_order(current: Decimal, angle: Decimal) -> tuple:
    turn = (angle - current) % Decimal(360)
    return (0 if turn == 180 else 1, min(turn, (current - angle) % Decimal(360)), angle)


def _overlaps_tile(board: PhysicalBoard, placement: Placement, bounds: tuple[int, int, int, int]) -> bool:
    polygon = transformed_footprint_polygon(board, placement)
    x0, y0, x1, y1 = bounds
    return (min(p.x_nm for p in polygon) < x1 and max(p.x_nm for p in polygon) > x0
            and min(p.y_nm for p in polygon) < y1 and max(p.y_nm for p in polygon) > y0)


def propose_moves(
    board: PhysicalBoard, congestion: CongestionMap, failed_nets: Iterable[str],
    placement_options: PlacementPlannerOptions | None, *, step_nm: int,
    steps: tuple[int, ...] = (1, 2, 4), rotations: bool = True, limit: int = 24,
) -> tuple[Proposal, ...]:
    """Legal down-gradient moves (and rotations) of parts on failed nets, then on hot tiles.

    Returns ``()`` when the board carries tracks or vias: a part beneath
    accepted copper must not move, since its terminals would leave that
    copper behind (the same rule as ``routeflow.detailed_failure_trials``);
    the feedback loop proposes on the unrouted source and reroutes the tail.
    """
    if step_nm <= 0 or limit < 0 or not steps or any(step <= 0 for step in steps):
        raise ValueError("congestion proposal step, multipliers and limit must be positive")
    if board.tracks or board.vias:
        return ()
    options = placement_options or PlacementPlannerOptions()
    failed = frozenset(failed_nets)
    fixed = set(options.fixed_references) | {
        rule.reference for rule in board.placement_rules if rule.fixed_position is not None}
    poses = {pose.reference: pose for pose in board.placements}
    on_failed = sorted({pad.component for net in board.nets if net.name in failed
                        for pad in net.pads if pad.component in poses} - fixed)
    failed_net_of = {reference: min(net.name for net in board.nets if net.name in failed
                                    and any(pad.component == reference for pad in net.pads))
                     for reference in on_failed}
    hot = congestion.hot_tiles(8)
    hot_parts: dict[str, tuple[int, int]] = {}
    for reference in sorted(set(poses) - fixed - set(on_failed)):
        for ix, iy, _ in hot:
            if _overlaps_tile(board, poses[reference], congestion.tile_bounds(ix, iy)):
                hot_parts[reference] = (ix, iy)
                break
    grid = int(getattr(options, "grid_step_nm", 0) or 0)
    utilisation = congestion.utilisation()
    proposals: list[Proposal] = []
    seen: set[tuple] = set()

    def offer(reference: str, proposed: Placement, rank: tuple, reason: str) -> None:
        try:
            trial = move_placement_unit(board, poses, reference, proposed)
        except ValueError:
            return
        signature = tuple(sorted((pose.reference, pose.position, pose.rotation_degrees, pose.side)
                                 for pose in trial.values()))
        if signature in seen:
            return
        seen.add(signature)
        if not placement_solution_is_legal(board, trial, options):
            return
        moved = trial[reference]
        current = poses[reference]
        proposals.append(Proposal(
            reference=reference,
            board=replace(board, placements=tuple(trial[pose.reference] for pose in board.placements)),
            delta_nm=(moved.position.x_nm - current.position.x_nm, moved.position.y_nm - current.position.y_nm),
            rotation_degrees=moved.rotation_degrees, reason=reason, rank=rank))

    for group, references in ((0, on_failed), (1, sorted(hot_parts))):
        for reference in references:
            current = poses[reference]
            ix, iy = congestion.tile_of(current.position)
            heat = float(utilisation[iy, ix]) or 0.0
            why = (f"failed_net:{failed_net_of[reference]}" if group == 0
                   else "hot_tile:{},{}".format(*hot_parts[reference]))
            gradient = congestion.gradient_at(current.position)
            if gradient != (0.0, 0.0):
                directions = ((gradient, "gradient"),)
            else:
                directions = tuple(((float(dx), float(dy)), "axis")
                                   for dx, dy in ((-1, 0), (0, -1), (0, 1), (1, 0)))
            for multiplier in sorted(set(steps)):
                distance = step_nm * multiplier
                for order, ((dx, dy), how) in enumerate(directions):
                    delta = _snap((dx * distance, dy * distance), grid)
                    if delta == (0, 0):
                        continue
                    proposed = replace(current, position=Point(current.position.x_nm + delta[0],
                                                               current.position.y_nm + delta[1]))
                    offer(reference, proposed, (group, 0.0 - heat, multiplier, reference, 0, order),
                          f"{why};{how}x{multiplier}")
            if rotations:
                angles = sorted((angle for angle in _allowed_orientations(board, reference)
                                 if angle != current.rotation_degrees),
                                key=lambda angle: _rotation_order(current.rotation_degrees, angle))
                for order, angle in enumerate(angles):
                    offer(reference, replace(current, rotation_degrees=angle),
                          (group, 0.0 - heat, min(steps), reference, 1, order), f"{why};rotate{angle}")
    proposals.sort(key=lambda item: item.rank)
    return tuple(proposals[:limit])


# ---------------------------------------------------------------------------
# Screening


def screen(
    board: PhysicalBoard, proposals: Iterable[Proposal], global_options: GlobalRouterOptions | None,
    *, keep: int = 2, estimate_tile_nm: int | None = None,
    on_progress: ProgressCallback | None = None,
) -> tuple[tuple[Proposal, GlobalRoutingResult], ...]:
    """Best ``keep`` proposals: RUDY overflow change picks ``2 * keep``, ``route_global`` orders them.

    The cheap score is the proposal's estimated overflow area minus the
    baseline's, on one lattice (``estimate_tile_nm``, default 1 mm); its
    ties (common on a board with no estimated overflow) go to the change in
    the sum of squared utilisation, then the proposal rank. The result is
    ordered by ``routeflow._route_score`` of the global route, then the
    cheap score, then reference and proposal rank.
    """
    from .routeflow import _route_score

    if keep < 0:
        raise ValueError("screen keep must be nonnegative")
    proposals = tuple(proposals)
    tile = int(estimate_tile_nm or DEFAULT_TILE_NM)
    emit(on_progress, "congestion_screen", "started", proposal_count=len(proposals), keep=keep)
    if not proposals or keep == 0:
        emit(on_progress, "congestion_screen", "finished", proposal_count=len(proposals),
             routed_count=0, kept_count=0)
        return ()
    base_overflow, base_spread = _estimate_score(estimate_demand(board, tile))

    def delta(item: Proposal) -> tuple[float, float]:
        overflow, spread = _estimate_score(estimate_demand(item.board, tile))
        return (overflow - base_overflow, spread - base_spread)

    cheap = sorted(((delta(item), item) for item in proposals), key=lambda pair: (pair[0], pair[1].rank))
    options = global_options or GlobalRouterOptions()
    routed = [(score, item, route_global(item.board, options)) for score, item in cheap[:2 * keep]]
    routed.sort(key=lambda triple: (_route_score(triple[2]), triple[0], triple[1].reference, triple[1].rank))
    result = tuple((item, route) for _, item, route in routed[:keep])
    emit(on_progress, "congestion_screen", "finished", proposal_count=len(proposals),
         routed_count=len(routed), kept_count=len(result),
         best_overflow_delta=round(cheap[0][0][0], 6))
    return result


# ---------------------------------------------------------------------------
# Feedback loop (stage B): map, propose, screen, rerun the pipeline tail


#: Each map's utilisation is divided by this percentile of its own positive
#: utilisation before the two are added, so neither scale dominates.
NORMALISATION_PERCENTILE = 95.0

#: Smallest translation step the loop halves down to.
MINIMUM_STEP_NM = nm_from_mm("0.25")

#: Meaning of each acceptance score component (``flow._feedback_score``); lower is better.
SCORE_ORDER = ("hard_drc_errors", "open_signal_nets", "critical_failures", "unrouted_nets",
               "conflict_overflow", "total_length_nm", "vias")


def _normalised_utilisation(congestion: CongestionMap) -> np.ndarray:
    utilisation = congestion.utilisation()
    real = utilisation[(utilisation > 0) & (utilisation < UTILISATION_SENTINEL)]
    if not real.size:
        return np.zeros_like(utilisation)
    # A tile with demand but no capacity counts as this map's hottest real tile.
    utilisation = np.where(utilisation >= UTILISATION_SENTINEL, float(real.max()), utilisation)
    return utilisation / float(np.percentile(real, NORMALISATION_PERCENTILE))


def combine_normalised(primary: CongestionMap, other: CongestionMap, weight: float = 1.0) -> CongestionMap:
    """Add two maps on one lattice after scaling each to its own 95th percentile.

    The router map (lane cells of overflow and history per tile) and the
    RUDY estimate (lane cells of expected wire per tile) are on different
    scales, so each utilisation is divided by the 95th percentile of its own
    positive, finite utilisation: a tile at 1 is as hot as that map's 95th
    percentile tile, a sentinel tile (demand, no capacity) counts as the
    map's hottest real tile, and an all-zero map contributes nothing. The
    result is ``u = u_primary / p95_primary + weight * u_other / p95_other``
    on the primary map's capacity (``demand = u * capacity``); tiles where
    the primary has no capacity carry no demand, since no copper can go
    there. ``failed_points`` are the primary's, then the other's new ones.
    """
    if primary._lattice() != other._lattice():
        raise ValueError(f"congestion lattices differ: {primary._lattice()} != {other._lattice()}")
    if not np.isfinite(weight):
        raise ValueError("congestion combination weight must be finite")
    utilisation = _normalised_utilisation(primary) + float(weight) * _normalised_utilisation(other)
    capacity = primary.capacity
    demand = np.where(capacity > 0, utilisation * capacity, 0.0)
    failed = tuple(dict.fromkeys((*primary.failed_points, *other.failed_points)))
    return CongestionMap(
        tile_nm=primary.tile_nm, x0_nm=primary.x0_nm, y0_nm=primary.y0_nm, nx=primary.nx, ny=primary.ny,
        demand=demand, capacity=capacity, source="combined", failed_points=failed)


def feedback_map(board: PhysicalBoard, router_map: CongestionMap | None, *,
                 tile_nm: int | None = None, weight: float = 1.0) -> CongestionMap:
    """The loop's map: the router's contention plus the RUDY estimate on its lattice.

    Both are normalised (:func:`combine_normalised`, weight 1.0 by default).
    Without a router map (legacy router, export disabled) the estimate is
    used alone, on its own lattice.
    """
    if router_map is None:
        return estimate_demand(board, int(tile_nm or DEFAULT_TILE_NM))
    estimate = estimate_demand(board, router_map.tile_nm, lattice=(
        router_map.x0_nm, router_map.y0_nm, router_map.nx, router_map.ny))
    return combine_normalised(router_map, estimate, weight)


@dataclass(frozen=True, slots=True)
class FeedbackState:
    """One full pass of the pipeline tail on one placement.

    ``board`` is the unrouted placement source the tail started from (moves
    are proposed on it); ``critical``, ``detailed`` and ``drc`` are its
    results; ``deferred_nets`` are zone nets awaiting fill, never counted as
    failed; ``payload`` is whatever else the caller needs back on acceptance.
    """

    board: PhysicalBoard
    global_route: GlobalRoutingResult
    critical: object
    detailed: object
    drc: object
    deferred_nets: frozenset[str] = frozenset()
    payload: object = None


@dataclass(frozen=True, slots=True)
class FeedbackTrial:
    index: int
    round: int
    reference: str
    delta_nm: tuple[int, int]
    rotation_degrees: str
    reason: str
    step_nm: int
    outcome: str            # accepted | not_improved | global_failed
    score: tuple[int, ...] | None


@dataclass(frozen=True, slots=True)
class FeedbackOutcome:
    """What the loop did: every trial, the accepted moves and the final map."""

    state: FeedbackState = field(repr=False, compare=False)
    trials: tuple[FeedbackTrial, ...] = ()
    accepted_moves: int = 0
    reruns: int = 0         # pipeline-tail reruns spent (the trial budget's unit)
    rounds: int = 0
    stop_reason: str = "not_run"
    initial_score: tuple[int, ...] | None = None
    final_score: tuple[int, ...] | None = None
    initial_failed_nets: tuple[str, ...] = ()
    final_failed_nets: tuple[str, ...] = ()
    congestion: CongestionMap | None = field(default=None, repr=False, compare=False)
    initial_congestion: CongestionMap | None = field(default=None, repr=False, compare=False)

    def hot_tiles(self, limit: int = 16, *, initial: bool = False) -> list[dict]:
        """Hottest tiles of the final (or first) map: bounds in mm and utilisation."""
        congestion = self.initial_congestion if initial else self.congestion
        if congestion is None:
            return []
        result = []
        for ix, iy, value in congestion.hot_tiles(limit):
            bounds = congestion.tile_bounds(ix, iy)
            result.append({"ix": ix, "iy": iy, "utilisation": round(value, 6),
                           "bounds_mm": [float(Decimal(v) / Decimal(1_000_000)) for v in bounds]})
        return result

    def report(self) -> dict:
        return {
            "accepted_moves": self.accepted_moves,
            "reruns": self.reruns,
            "rounds": self.rounds,
            "stop_reason": self.stop_reason,
            "score_order": list(SCORE_ORDER),
            "initial_score": list(self.initial_score) if self.initial_score is not None else None,
            "final_score": list(self.final_score) if self.final_score is not None else None,
            "initial_failed_nets": list(self.initial_failed_nets),
            "final_failed_nets": list(self.final_failed_nets),
            "map_source": self.congestion.source if self.congestion is not None else None,
            "trials": [{"index": t.index, "round": t.round, "reference": t.reference,
                        "delta_nm": list(t.delta_nm), "rotation_degrees": t.rotation_degrees,
                        "reason": t.reason, "step_nm": t.step_nm, "outcome": t.outcome,
                        "score": list(t.score) if t.score is not None else None} for t in self.trials],
            "hot_tiles": self.hot_tiles(16),
        }


def failed_nets(state: FeedbackState) -> frozenset[str]:
    """Unrouted ordinary nets, failed critical nets and DRC-open nets, less deferred zone nets."""
    deferred = state.deferred_nets
    failed = {item.net for item in getattr(state.detailed, "nets", ())
              if not item.connected and item.net not in deferred}
    for item in getattr(state.critical, "nets", ()):
        if not item.connected or item.diagnostics:
            failed.update(net for net in item.nets if not net.startswith("<"))
    failed.update(net for finding in getattr(state.drc, "findings", ())
                  if finding.code == "DRC-OPEN-NET" for net in finding.nets if net not in deferred)
    return frozenset(failed)


def _signature(board: PhysicalBoard) -> tuple:
    return tuple(sorted((pose.reference, pose.position, pose.rotation_degrees, pose.side)
                        for pose in board.placements))


def run_placement_feedback(
    state: FeedbackState, *, trials: int, movement_nm: int,
    placement_options: PlacementPlannerOptions | None,
    global_options: GlobalRouterOptions | None,
    rerun, score, keep: int = 2, weight: float = 1.0,
    on_progress: ProgressCallback | None = None,
) -> FeedbackOutcome:
    """Bounded routability-driven placement feedback around ``state``.

    Each round builds :func:`feedback_map` from the incumbent's router map
    (``state.detailed.congestion``) and board, proposes moves of parts on
    the failed nets and on hot tiles (:func:`propose_moves`, ``step_nm``
    starting at ``movement_nm``), keeps ``keep`` by :func:`screen`, and calls
    ``rerun(board, global_route) -> FeedbackState`` on each kept proposal
    whose global route succeeded; each rerun is one trial against ``trials``
    (a failed global route is recorded but costs none). The round's best
    trial is accepted when
    ``score(critical, detailed, drc)`` is strictly lower than the
    incumbent's; a round without acceptance halves the step (floor
    0.25 mm). Stops when the budget is spent, when no failed nets remain or
    after two consecutive rounds without acceptance. A pose is tried once.
    """
    from .routing import GlobalRoutingStatus

    if trials < 0 or movement_nm <= 0 or keep <= 0:
        raise ValueError("placement feedback bounds are invalid")
    incumbent = state
    initial_score = incumbent_score = tuple(score(state.critical, state.detailed, state.drc))
    initial_failed = failed_nets(state)
    records: list[FeedbackTrial] = []
    accepted = rounds = stale = spent = 0
    step = int(movement_nm)
    floor = min(MINIMUM_STEP_NM, step)
    tried = {_signature(state.board)}
    first_map = last_map = map_owner = None
    emit(on_progress, "placement_feedback", "started", trials=trials, failed_nets=sorted(initial_failed),
         score=list(incumbent_score))
    while True:
        failed = failed_nets(incumbent)
        if not failed:
            stop = "no_failed_nets"
            break
        if spent >= trials:
            stop = "trial_budget"
            break
        if stale >= 2:
            stop = "no_improvement"
            break
        rounds += 1
        congestion = feedback_map(incumbent.board, getattr(incumbent.detailed, "congestion", None),
                                  weight=weight)
        first_map = first_map or congestion
        last_map, map_owner = congestion, incumbent
        proposals = tuple(item for item in propose_moves(incumbent.board, congestion, failed,
                                                         placement_options, step_nm=step)
                          if _signature(item.board) not in tried)
        kept = screen(incumbent.board, proposals, global_options, keep=keep,
                      estimate_tile_nm=congestion.tile_nm, on_progress=on_progress)
        best = None   # (score, state, record index)
        for proposal, global_route in kept:
            if spent >= trials:
                break
            tried.add(_signature(proposal.board))
            index = len(records) + 1
            emit(on_progress, "placement_feedback_trial", "started", index=index, round=rounds,
                 reference=proposal.reference, delta_nm=list(proposal.delta_nm), reason=proposal.reason)
            result = None
            if global_route.status is GlobalRoutingStatus.SUCCESS:
                spent += 1
                result = rerun(proposal.board, global_route)
            trial_score = (None if result is None
                           else tuple(score(result.critical, result.detailed, result.drc)))
            if trial_score is not None and trial_score < (best[0] if best else incumbent_score):
                best = (trial_score, result, len(records))
            records.append(FeedbackTrial(index, rounds, proposal.reference, proposal.delta_nm,
                                         str(proposal.rotation_degrees), proposal.reason, step,
                                         "global_failed" if result is None else "not_improved", trial_score))
            emit(on_progress, "placement_feedback_trial", "finished", index=index,
                 outcome=records[-1].outcome if result is None else (
                     "improved" if best is not None and best[2] == len(records) - 1 else "not_improved"),
                 score=list(trial_score) if trial_score is not None else None)
        if best is not None:
            incumbent_score, incumbent = best[0], best[1]
            records[best[2]] = replace(records[best[2]], outcome="accepted")
            accepted += 1
            stale = 0
        else:
            stale += 1
            step = max(step // 2, floor)
    if last_map is not None and map_owner is not incumbent:
        last_map = feedback_map(incumbent.board, getattr(incumbent.detailed, "congestion", None),
                                weight=weight)
    final_failed = failed_nets(incumbent)
    emit(on_progress, "placement_feedback", "finished", stop_reason=stop, trials=spent,
         accepted_moves=accepted, score=list(incumbent_score), failed_nets=sorted(final_failed))
    return FeedbackOutcome(
        state=incumbent, trials=tuple(records), accepted_moves=accepted, reruns=spent, rounds=rounds,
        stop_reason=stop, initial_score=initial_score, final_score=incumbent_score,
        initial_failed_nets=tuple(sorted(initial_failed)), final_failed_nets=tuple(sorted(final_failed)),
        congestion=last_map, initial_congestion=first_map)
