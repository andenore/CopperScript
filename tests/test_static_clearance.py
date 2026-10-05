"""Exact-preserving cached geometry and snapshot-local static broad phase."""
from copy import copy, deepcopy
from dataclasses import replace
import pickle
from random import Random

import pytest

from pcbir import (BoardOutline, BoardSide, CopperKeepout, CopperLayer, FootprintPad,
    PhysicalBoard, PhysicalFootprint, Placement, Point, PolygonRing, PolygonWithHoles,
    Size, Stackup, TrackSegment, nm_from_mm)
from pcbir.geometry import Bounds, RoundedConvexShape, shapes_clear
from pcbir.routing_clearance import RoutingClearanceIndex


def rectangle(name, x, y, width=1, height=1, *, layers=(CopperLayer.FRONT,),
              tracks=True, vias=True, holes=()):
    return CopperKeepout(name, layers, PolygonWithHoles(PolygonRing((
        Point.mm(x, y), Point.mm(x + width, y), Point.mm(x + width, y + height),
        Point.mm(x, y + height))), holes), block_tracks=tracks, block_vias=vias)


def board_fixture(keepouts=()):
    return PhysicalBoard("static", BoardOutline.rectangle(100, 100), {}, (), (),
        stackup=Stackup(copper_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                                      CopperLayer.INTERNAL_2, CopperLayer.BACK)),
        copper_keepouts=keepouts)


def linear_oracle(index, shape, layers, for_via):
    """The pre-optimization implementation, deliberately without the index."""
    board = index.board
    if (board.outline.circular_boundary or board.outline.boundary_path or board.outline.cutouts
            or board.mechanical_holes or board.mechanical_slots):
        from pcbir.mechanical import shape_in_board
        if not shape_in_board(board, shape, board.rules.minimum_clearance_nm,
                              board.rules.minimum_hole_clearance_nm):
            return False
    selected = set(layers)
    if any(selected.intersection(region_layers) and not shapes_clear(shape, region, 1)
           for region_layers, region in index._macro_regions):
        return False
    for keepout in index._keepouts:
        if not (keepout.block_vias if for_via else keepout.block_tracks):
            continue
        if not selected.intersection(keepout.layers):
            continue
        if keepout.has_holes:
            return False
        if shape.bounds.intersects(keepout.shape.bounds) and not shapes_clear(shape, keepout.shape):
            return False
    return True


def test_immutable_bounds_computed_once_and_copy_replace_pickle_stay_correct(monkeypatch):
    import pcbir.geometry as geometry
    real, calls = geometry.bounds, []
    def counted(points):
        calls.append(tuple(points))
        return real(points)
    monkeypatch.setattr(geometry, "bounds", counted)
    shape = RoundedConvexShape((Point(-5, 2), Point(10, 7)), 3)
    expected = Bounds(-8, -1, 13, 10)
    assert all(shape.bounds is shape.bounds and shape.bounds == expected for _ in range(1000))
    assert len(calls) == 1
    assert "_bounds" not in repr(shape)
    for cloned in (copy(shape), deepcopy(shape), pickle.loads(pickle.dumps(shape))):
        assert cloned == shape and hash(cloned) == hash(shape) and cloned.bounds == expected
    changed = replace(shape, radius_nm=4)
    assert changed.bounds == Bounds(-9, -2, 14, 11) and changed != shape
    assert replace(shape, spine=(Point(100, 100),)).bounds == Bounds(97, 97, 103, 103)


def test_index_matches_linear_oracle_for_layers_flags_negative_coordinates_and_queries():
    rng = Random(421)
    layers = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)
    keepouts = tuple(rectangle(str(i), rng.randrange(-5, 30), rng.randrange(-5, 30),
        width=rng.choice((.2, 1, 3)), height=rng.choice((.2, 1, 3)),
        layers=(layers[i % 4],), tracks=i % 3 != 0, vias=i % 3 != 1) for i in range(60))
    index = RoutingClearanceIndex(board_fixture(keepouts))
    for i in range(1200):
        start = Point.mm(rng.randrange(-7, 35) + rng.choice((0, .001, .5)), rng.randrange(-7, 35))
        end = Point(start.x_nm + rng.randrange(-3, 4) * 250000, start.y_nm + rng.randrange(-3, 4) * 250000)
        shape = RoundedConvexShape((start, end), rng.choice((0, 1, 100000, 400000)))
        selected = (layers[i % 4],) if i % 2 else layers
        for via in (False, True):
            assert index._keepout_clear(shape, selected, for_via=via) == linear_oracle(index, shape, selected, via)


@pytest.mark.parametrize("offset,clear", ((-1, True), (0, True), (1, False)))
def test_exact_keepout_tangency_across_bin_boundary(offset, clear):
    index = RoutingClearanceIndex(board_fixture((rectangle("edge", 2, 0),)))
    # A rectangle's zero-clearance boundary contact is legal under the existing
    # exact predicate; a one-nm penetration is not. Do not "fix" that policy.
    shape = RoundedConvexShape((Point(2_000_000 + offset - 100_000, 500_000),), 100_000)
    assert index._keepout_clear(shape, (CopperLayer.FRONT,), for_via=False) is clear
    assert clear == linear_oracle(index, shape, (CopperLayer.FRONT,), False)


@pytest.mark.parametrize("offset,clear", ((-2, True), (-1, True), (0, False), (1, False)))
def test_macro_one_nm_margin_is_not_lost_at_bin_boundary(monkeypatch, offset, clear):
    region = rectangle("macro", 2, 0)
    monkeypatch.setattr("pcbir.hard_macros.macro_reservations", lambda _: (region,))
    index = RoutingClearanceIndex(board_fixture())
    shape = RoundedConvexShape((Point(2_000_000 + offset, 500_000),))
    assert index._keepout_clear(shape, (CopperLayer.FRONT,), for_via=False) is clear
    assert clear == linear_oracle(index, shape, (CopperLayer.FRONT,), False)


def test_huge_regions_and_huge_queries_use_conservative_layer_fallback():
    keepouts = (rectangle("huge", -50, -50, 500, 500, layers=(CopperLayer.INTERNAL_2,)),
                rectangle("small", 10, 10))
    index = RoutingClearanceIndex(board_fixture(keepouts))
    assert index._static_large[CopperLayer.INTERNAL_2] == (0,)
    assert sum(len(ids) for ids in index._static_bins.values()) < 10
    shapes = (RoundedConvexShape((Point.mm(12, 12),)),
              RoundedConvexShape((Point.mm(-500, -500), Point.mm(500, 500)), 100))
    for shape in shapes:
        for layer in index.board.stackup.copper_layers:
            assert index._keepout_clear(shape, (layer,), for_via=False) == linear_oracle(index, shape, (layer,), False)


def test_unsupported_holes_remain_globally_fail_closed_with_object_flags():
    hole = PolygonRing((Point.mm(2, 2), Point.mm(3, 2), Point.mm(3, 3), Point.mm(2, 3)))
    index = RoutingClearanceIndex(board_fixture((rectangle("holes", 1, 1, 4, 4,
        layers=(CopperLayer.INTERNAL_1,), tracks=False, holes=(hole,)),)))
    far = RoundedConvexShape((Point.mm(90, 90),))
    assert not index._keepout_clear(far, (CopperLayer.INTERNAL_1,), for_via=True)
    assert index._keepout_clear(far, (CopperLayer.INTERNAL_1,), for_via=False)
    assert index._keepout_clear(far, (CopperLayer.FRONT,), for_via=True)
    assert not index.can_via("N", Point.mm(90, 90), 800000, CopperLayer.FRONT, CopperLayer.BACK)


@pytest.mark.parametrize("rotation,side", ((0, BoardSide.FRONT), (45, BoardSide.FRONT), (90, BoardSide.BACK)))
def test_transformed_keepouts_and_new_snapshot_do_not_reuse_xy_legality(rotation, side):
    fp = PhysicalFootprint("local", (FootprintPad("1", Point(0, 0), Size.mm(.3, .3)),),
        Size.mm(1, 1), keepouts=(rectangle("offset", 2, 0, 2, 1),))
    board = replace(board_fixture(), footprints={fp.name: fp},
        placements=(Placement("U", fp.name, Point.mm(20, 20), rotation_degrees=rotation, side=side),))
    index = RoutingClearanceIndex(board)
    moved = RoutingClearanceIndex(replace(board, placements=(replace(board.placements[0], position=Point.mm(50, 50)),)))
    region = index._keepouts[0]
    center = Point(sum(p.x_nm for p in region.shape.spine) // 4, sum(p.y_nm for p in region.shape.spine) // 4)
    shape = RoundedConvexShape((center,), 10000)
    for via in (False, True):
        assert not index._keepout_clear(shape, tuple(region.layers), for_via=via)
        assert moved._keepout_clear(shape, tuple(region.layers), for_via=via)
        assert linear_oracle(index, shape, tuple(region.layers), via) is False


def test_static_index_stays_separate_while_copper_is_added():
    index = RoutingClearanceIndex(board_fixture((rectangle("remote", 80, 80),)))
    bins = dict(index._static_bins)
    start, end = Point.mm(10, 10), Point.mm(20, 10)
    assert index.can_track("N", start, end, 250000, CopperLayer.FRONT)
    index.add_track(TrackSegment("other", Point.mm(15, 9), Point.mm(15, 11), 250000, CopperLayer.FRONT))
    assert not index.can_track("N", start, end, 250000, CopperLayer.FRONT)
    assert index._static_bins == bins
    with pytest.raises(ValueError, match="bin size"):
        RoutingClearanceIndex(index.board, 0)


def test_local_query_reduces_bound_checks_without_changing_answer(monkeypatch):
    index = RoutingClearanceIndex(board_fixture(tuple(rectangle(str(i), i * 3, 0) for i in range(1000))))
    shape = RoundedConvexShape((Point.mm(.5, 2),), 100000)
    real, counts = Bounds.intersects, [0]
    def counted(first, second):
        counts[0] += 1
        return real(first, second)
    monkeypatch.setattr(Bounds, "intersects", counted)
    expected = linear_oracle(index, shape, (CopperLayer.FRONT,), False)
    linear_checks = counts[0]
    counts[0] = 0
    assert index._keepout_clear(shape, (CopperLayer.FRONT,), for_via=False) == expected
    assert linear_checks == 1000 and counts[0] < 10
