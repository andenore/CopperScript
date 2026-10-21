"""Grid-independent routing costs shared by global and detailed search.

One fixed-event cost unit is one million integer micro-units. Planar costs
are per millimetre (10 baseline units/mm). Keeping nanometre precision avoids
rounding each subdivided edge and makes straight-edge splitting invariant.
"""
from dataclasses import dataclass
from math import isqrt

from .geometry import Bounds
from .physical import CopperLayer, Point

COST_UNIT = 1_000_000


@dataclass(frozen=True, slots=True)
class LayerBias:
    """An escape plan's soft layer assignment: ``net`` pays extra on ``layer`` inside ``area``.

    ``rate`` is in cost units per millimetre (10 doubles the cost of track).
    A penalty, never a discount, so search heuristics stay admissible.
    """

    net: str
    layer: CopperLayer
    area: Bounds
    rate: int

    def cost(self, first: Point, second: Point) -> int:
        """The penalty of a track from ``first`` to ``second`` on ``layer`` whose midpoint is in ``area``."""
        x, y = (first.x_nm + second.x_nm) // 2, (first.y_nm + second.y_nm) // 2
        if not (self.area.min_x <= x <= self.area.max_x and self.area.min_y <= y <= self.area.max_y):
            return 0
        return length_cost(first, second, self.rate)


def length_cost(first: Point, second: Point, rate: int = 10) -> int:
    dx, dy = second.x_nm - first.x_nm, second.y_nm - first.y_nm
    return rate * isqrt(dx * dx + dy * dy)


def preference_cost(first: Point, second: Point, rank: int, layer_cost: int,
                    wrong_way_cost: int = 0, diagonal: bool = False) -> int:
    # Half-rate diagonal heading penalties retain integer-nanometre precision.
    rate_twice = 2 * rank * layer_cost + (wrong_way_cost if diagonal else 2 * wrong_way_cost)
    return length_cost(first, second, rate_twice) // 2


def demand_cost(first: Point, second: Point, rate: int, usage: int, capacity: int) -> int:
    """Soft run cost of already-committed local demand, before any overflow.

    ``rate`` is charged per millimetre at full utilization, in the same units
    as ``length_cost``. Saturating at capacity keeps overflow a separate,
    negotiated penalty rather than double counting it here.
    """
    if rate <= 0 or usage <= 0 or capacity <= 0:
        return 0
    return length_cost(first, second, rate) * min(usage, capacity) // capacity
