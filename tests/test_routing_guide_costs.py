from math import sqrt

import pytest

from pcbir.physical import CopperLayer, PadReference, Point
from pcbir.routing import GlobalNetRoute, GlobalRouteSegment, GridNode, PinAccess
from pcbir.routing_guides import GuideExposure, guide_transition_cost
from pcbir.routing_costs import COST_UNIT

F = CopperLayer.FRONT
B = CopperLayer.BACK


def exposure(*segments, accesses=(), margin=0, projected=False):
    guide = GlobalNetRoute("N", True, accesses, tuple(
        GlobalRouteSegment("N", layer, Point.mm(*a), Point.mm(*b),
                           round(radius * COST_UNIT), str(index))
        for index, (layer, a, b, radius) in enumerate(segments)
    ), (), 0)
    return GuideExposure(guide, round(margin * COST_UNIT), ignore_layer=projected)


@pytest.mark.parametrize("step", [1, .5, .1, .025])
def test_outside_run_cost_is_pitch_invariant(step):
    region = exposure((F, (0, 0), (4, 0), .5))
    whole = region.outside_length_nm(Point.mm(0, 2), Point.mm(4, 2), F)
    count = round(4 / step)
    split = sum(region.outside_length_nm(Point.mm(i * step, 2),
                Point.mm((i + 1) * step, 2), F) for i in range(count))
    assert whole == split == 4 * COST_UNIT
    assert 50 * split == 200 * COST_UNIT


def test_partial_edge_exposure_and_nonuniform_boundary_splits():
    region = exposure((F, (0, 0), (4, 0), 1))
    points = [Point.mm(2, y) for y in (-3, -1, -.73, 0, .13, 1, 3)]
    whole = region.outside_length_nm(points[0], points[-1], F)
    assert whole == 4 * COST_UNIT
    assert sum(region.outside_length_nm(a, b, F)
               for a, b in zip(points, points[1:])) == whole
    # Neighbor inside does not make the entire approach free.
    assert region.outside_length_nm(Point.mm(2, -3), Point.mm(2, 0), F) == 2 * COST_UNIT


def test_capsule_round_ends_and_overlapping_union_not_double_counted():
    region = exposure((F, (0, 0), (4, 0), 1), (F, (2, 0), (6, 0), 1))
    assert region.outside_length_nm(Point.mm(-3, 0), Point.mm(9, 0), F) == 4 * COST_UNIT
    assert region.outside_length_nm(Point.mm(-3, 1), Point.mm(9, 1), F) == 6 * COST_UNIT


def test_diagonal_round_boundary_subdivision_precision():
    region = exposure((F, (0, 0), (0, 0), 1))
    points = [Point.mm(i / 10, i / 10) for i in range(-20, 21)]
    whole = region.outside_length_nm(points[0], points[-1], F)
    assert whole == round((4 * sqrt(2) - 2) * COST_UNIT)
    assert abs(sum(region.outside_length_nm(a, b, F)
                   for a, b in zip(points, points[1:])) - whole) <= len(points) - 2


def test_diagonal_capsule_and_board_offset():
    for offset in (0, 10000):
        region = exposure((F, (offset, offset), (offset + 4, offset + 4), 1))
        assert region.outside_length_nm(Point.mm(offset, offset),
                                       Point.mm(offset + 4, offset + 4), F) == 0
        assert region.outside_length_nm(Point.mm(offset - 2, offset - 2),
                Point.mm(offset + 6, offset + 6), F) == round((4 * sqrt(2) - 2) * COST_UNIT)


def test_access_square_margin_and_projected_layers():
    access = PinAccess(PadReference("U", "1"), Point.mm(2, 0), GridNode(0, 0, 0),
                       Point.mm(2, 0), F)
    region = exposure(accesses=(access,), margin=1)
    first, last = Point.mm(-1, 0), Point.mm(5, 0)
    assert region.outside_length_nm(first, last, F) == 4 * COST_UNIT
    assert region.outside_length_nm(first, last, B) == 6 * COST_UNIT
    assert exposure(accesses=(access,), margin=1, projected=True).outside_length_nm(
        first, last, B) == 4 * COST_UNIT


def test_empty_zero_length_reverse_and_margin():
    first, last = Point.mm(0, 0), Point.mm(3, 4)
    region = exposure()
    assert region.outside_length_nm(first, last, F) == 5 * COST_UNIT
    assert region.outside_length_nm(first, first, F) == 0
    assert region.outside_length_nm(last, first, F) == 5 * COST_UNIT
    wide = exposure((F, (0, 0), (4, 0), 1), margin=1)
    assert wide.outside_length_nm(Point.mm(0, 2), Point.mm(4, 2), F) == 0


def test_via_guide_exit_is_one_event_not_planar_length():
    assert guide_transition_cost(True, False, 50 * COST_UNIT) == 50 * COST_UNIT
    for before, after in ((True, True), (False, True), (False, False)):
        assert guide_transition_cost(before, after, 50 * COST_UNIT) == 0


@pytest.mark.parametrize("axis", [(2, 4), (2, 2.13, 2.73, 3, 3.1, 4)])
def test_search_charges_physical_exposure_including_inside_target(monkeypatch, axis):
    # A controlled one-way search isolates guide cost from congestion/history,
    # bends and clearance. The target is inside; the first millimetre is not.
    import pcbir.detailed as detailed
    from pcbir.physical import PhysicalBoard, BoardOutline, nm_from_mm
    base = PhysicalBoard("cost", BoardOutline.rectangle(10, 10), {}, (), ())
    grid = detailed._Grid((F,), (nm_from_mm(5),), tuple(nm_from_mm(y) for y in axis),
                          base, frozenset(), nm_from_mm(1))
    nodes = tuple(detailed.DetailedNode(0, 0, index) for index in range(len(axis)))
    guide = GlobalNetRoute("N", True, (), (GlobalRouteSegment(
        "N", F, Point.mm(4, 4), Point.mm(6, 4), nm_from_mm(1), "g"),), (), 0)
    monkeypatch.setattr(detailed, "_neighbors", lambda grid, node, *a, **kw:
                        (nodes[node.y_index + 1],) if node != nodes[-1] else ())
    class Clear:
        def can_track(self, *args):
            return True
        can_route = can_track
    costs = []
    push = detailed.heappush
    def observe(queue, item):
        if item[4] == nodes[-1]:
            costs.append(item[1])
        push(queue, item)
    monkeypatch.setattr(detailed, "heappush", observe)
    result = detailed._search_once(grid, {nodes[0]}, frozenset({nodes[-1]}), (F,),
        guide, {}, {}, Clear(), "N", nm_from_mm(.2), detailed.DetailedRouterOptions(
            layer_preference_cost=0, direction_preference_cost=0),
        corridor_only=False, state_budget=100, guide_margin_nm=0,
        allow_movable_conflicts=False, forbidden_via_positions=frozenset())
    assert result is not None
    assert costs == [70 * COST_UNIT]  # 2 mm baseline + 1 mm outside guide.
