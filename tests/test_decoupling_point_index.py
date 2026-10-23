"""Candidate pruning must leave the exact, ordered original-edge graph alone."""
from random import Random
from pcbir.physical import Point, TrackSegment, CopperLayer
from pcbir.decoupling_paths import _PathPoints, _on


def test_index_matches_full_scan_for_inclusive_reversed_and_diagonal_edges():
    random = Random(987)
    points = {Point(random.randrange(-100, 101), random.randrange(-100, 101)) for _ in range(600)}
    ends = [(Point(-100, 0), Point(100, 0)), (Point(0, 100), Point(0, -100)),
            (Point(50, 50), Point(-50, -50)), (Point(5, 7), Point(6, 7))]
    ends += [(Point(random.randrange(-100, 101), random.randrange(-100, 101)),
              Point(random.randrange(-100, 101), random.randrange(-100, 101))) for _ in range(120)]
    points.update(p for pair in ends for p in pair)
    indexed = _PathPoints(points)
    for a, b in ends:
        edge = TrackSegment('N', a, b, 1, CopperLayer.FRONT)
        expected = sorted((p for p in points if _on(p, edge)),
                          key=lambda p: (p.x_nm-a.x_nm)**2+(p.y_nm-a.y_nm)**2)
        assert indexed.on(edge) == expected


def test_short_edge_queries_do_not_scan_unrelated_net_points(monkeypatch):
    import pcbir.decoupling_paths as module
    points = {Point(x*100, y*100) for x in range(80) for y in range(80)}
    indexed = _PathPoints(points)
    calls = []
    exact = module._on
    def observe(point, edge):
        calls.append(point)
        return exact(point, edge)
    monkeypatch.setattr(module, '_on', observe)
    edge = TrackSegment('N', Point(1000, 2000), Point(1200, 2000), 1, CopperLayer.FRONT)
    assert indexed.on(edge) == [Point(x, 2000) for x in (1000, 1100, 1200)]
    assert len(calls) == 80  # narrow Y interval; full scan would test 6400.
    assert _PathPoints(()).on(edge) == []
