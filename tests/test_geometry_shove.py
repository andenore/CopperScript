from pcbir import BoardOutline, CopperLayer, PhysicalBoard, PhysicalNet, Point, PolygonRing, TrackSegment, nm_from_mm, route_any_angle
from pcbir.geometry import capsules_clear, point_in_polygon, segment_distance_squared, segments_intersect
from pcbir.shove import shove_track


def _track(net: str, y: str) -> TrackSegment:
    return TrackSegment(net, Point.mm(2, y), Point.mm(8, y), nm_from_mm("0.2"), CopperLayer.FRONT)


def test_exact_integer_predicates_cover_crossing_clearance_and_concavity() -> None:
    a, b, c, d = Point.mm(0, 0), Point.mm(10, 10), Point.mm(0, 10), Point.mm(10, 0)
    assert segments_intersect(a, b, c, d)
    assert segment_distance_squared(a, b, c, d) == 0
    assert not capsules_clear(a, b, 100, c, d, 100)
    concave = (Point.mm(0, 0), Point.mm(4, 0), Point.mm(4, 4), Point.mm(2, 2), Point.mm(0, 4))
    assert point_in_polygon(Point.mm(1, 1), concave)
    assert not point_in_polygon(Point.mm(2, 3), concave)


def test_shove_is_recursive_deterministic_and_atomic() -> None:
    board = PhysicalBoard("Shove", BoardOutline.rectangle(10, 10), {}, (),
                          tuple(PhysicalNet(name, ()) for name in ("A", "B", "C")),
                          tracks=(_track("A", "2.0"), _track("B", "2.5"), _track("C", "3.0")))
    delta = Point.mm(0, "0.4")
    first = shove_track(board, 0, delta)
    assert first == shove_track(board, 0, delta)
    assert first.committed and first.moved_track_indexes == (0, 1, 2)
    rejected = shove_track(board, 0, delta, locked_track_indexes=frozenset({1}))
    assert not rejected.committed and rejected.board is board and rejected.moved_track_indexes == ()


def test_any_angle_visibility_route_walks_around_expanded_obstacle() -> None:
    obstacle = PolygonRing((Point.mm(4, 3), Point.mm(6, 3), Point.mm(6, 7), Point.mm(4, 7)))
    first = route_any_angle(Point.mm(1, 5), Point.mm(9, 5), (obstacle,))
    assert first == route_any_angle(Point.mm(1, 5), Point.mm(9, 5), (obstacle,))
    assert first is not None and len(first) == 4
    assert first[0] == Point.mm(1, 5) and first[-1] == Point.mm(9, 5)
