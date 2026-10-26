"""Geometric and transaction regressions from the six-layer routing review."""

from pcbir import Point
from pcbir.routing_costs import length_cost, preference_cost


def test_costs_are_physical_and_invariant_to_edge_splitting():
    a, b, c = Point.mm(0, 0), Point.mm("0.3", 0), Point.mm(1, 0)
    def cost(first, second):
        return length_cost(first, second) + preference_cost(first, second, 2, 4, 2)
    assert cost(a, c) == cost(a, b) + cost(b, c)
    assert cost(a, c) == 20_000_000
