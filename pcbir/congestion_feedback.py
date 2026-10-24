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
from dataclasses import dataclass, replace
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
    ``lane_nm ** 2`` (see the module docstring for units).
    """
    from .unified_router import _polygon_inside, _spine_distance
    from .drc import placed_pad_shape
    from .zone_geometry import deferred_zone_nets

    if tile_nm <= 0:
        raise ValueError("estimate tile must be positive")
    x0, y0, nx, ny = _lattice(board, tile_nm)
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
