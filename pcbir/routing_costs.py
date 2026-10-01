"""Grid-independent routing costs shared by global and detailed search.

One fixed-event cost unit is one million integer micro-units. Planar costs
are per millimetre (10 baseline units/mm). Keeping nanometre precision avoids
rounding each subdivided edge and makes straight-edge splitting invariant.
"""
from math import isqrt

from .physical import Point

COST_UNIT = 1_000_000


def length_cost(first: Point, second: Point, rate: int = 10) -> int:
    dx, dy = second.x_nm - first.x_nm, second.y_nm - first.y_nm
    return rate * isqrt(dx * dx + dy * dy)


def preference_cost(first: Point, second: Point, rank: int, layer_cost: int,
                    wrong_way_cost: int = 0, diagonal: bool = False) -> int:
    # Half-rate diagonal heading penalties retain integer-nanometre precision.
    rate_twice = 2 * rank * layer_cost + (wrong_way_cost if diagonal else 2 * wrong_way_cost)
    return length_cost(first, second, rate_twice) // 2
