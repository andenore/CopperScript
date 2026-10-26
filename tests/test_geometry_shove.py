from pcbir import CopperLayer, Point, TrackSegment, nm_from_mm
from pcbir.geometry import capsules_clear, point_in_polygon, segment_distance_squared, segments_intersect


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


