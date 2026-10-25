"""Spatial drill broad phase agrees with the prior complete linear scan."""
from dataclasses import replace
from pcbir import BoardOutline, CopperLayer, Point, Via
from pcbir.routing_clearance import RoutingClearanceIndex
from test_detailed_routing import _board


def test_spatial_drill_queries_match_linear_checks_and_blocker_records(monkeypatch):
    board = replace(_board(), outline=BoardOutline.rectangle(100, 100))
    spatial = RoutingClearanceIndex(board, bin_size_nm=1_000_000)
    linear = RoutingClearanceIndex(board, bin_size_nm=1_000_000)
    monkeypatch.setattr(linear, "_nearby_holes", lambda *args: iter(linear._holes))
    for i in range(80):
        via = Via(f"N{i}", Point(1_000_000+i*1_000_000, 20_000_000+(i%7)*1_000_000),
                  600_000, 300_000)
        spatial.add_via(via, locked=bool(i%2))
        linear.add_via(via, locked=bool(i%2))
    for x in range(1_000_000, 81_000_000, 375_000):
        for radius in (100_000, 350_000, 1_500_000):
            point = Point(x, 22_000_000)
            assert spatial._hole_clear("QUERY", point, radius, (CopperLayer.FRONT, CopperLayer.BACK)) == \
                   linear._hole_clear("QUERY", point, radius, (CopperLayer.FRONT, CopperLayer.BACK))
        via = Via("QUERY", point, 600_000, 300_000)
        assert spatial.can_via(via.net, point, via.size_nm, via.from_layer, via.to_layer, via.drill_nm) == \
               linear.can_via(via.net, point, via.size_nm, via.from_layer, via.to_layer, via.drill_nm)
        assert spatial.via_blocker_records(via) == linear.via_blocker_records(via)
        assert spatial.blocking_via_nets(via) == linear.blocking_via_nets(via)
    assert spatial.additions() == linear.additions()


def test_drill_lookup_is_local_and_preserves_same_via_reuse():
    board = replace(_board(), outline=BoardOutline.rectangle(2000, 20))
    index = RoutingClearanceIndex(board, bin_size_nm=1_000_000)
    for i in range(1000):
        index.add_via(Via(f"N{i}", Point.mm(i+1, 15), 600_000, 300_000))
    assert len(tuple(index._nearby_holes(Point.mm(500, 15), 400_000))) < 5
    assert index._hole_clear("N499", Point.mm(500, 15), 150_000,
                             (CopperLayer.FRONT, CopperLayer.BACK))
    assert not index._hole_clear("FOREIGN", Point.mm(500, 15), 150_000,
                                 (CopperLayer.FRONT, CopperLayer.BACK))


def test_large_hole_and_large_query_fallback_remain_visible():
    index = RoutingClearanceIndex(replace(_board(), outline=BoardOutline.rectangle(200, 200)),
                                  bin_size_nm=1_000_000)
    index.add_via(Via("LARGE", Point.mm(100, 100), 60_000_000, 50_000_000))
    assert not index._hole_clear("QUERY", Point.mm(124, 100), 100_000,
                                 (CopperLayer.FRONT, CopperLayer.BACK))
    assert not index._hole_clear("QUERY", Point.mm(150, 100), 30_000_000,
                                 (CopperLayer.FRONT, CopperLayer.BACK))
