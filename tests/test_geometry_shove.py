from pcbir import BoardOutline, CopperLayer, PhysicalBoard, PhysicalNet, Point, PolygonRing, TrackSegment, Via, cleanup_acute_angles, nm_from_mm, route_any_angle, shove_bundle, shove_via
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


def test_via_shove_moves_a_collision_chain_or_rolls_back() -> None:
    nets = tuple(PhysicalNet(name, ()) for name in ("A", "B"))
    board = PhysicalBoard("ViaShove", BoardOutline.rectangle(10, 10), {}, (), nets,
                          vias=(Via("A", Point.mm(2, 2), nm_from_mm("0.6"), nm_from_mm("0.3")),
                                Via("B", Point.mm(3, 2), nm_from_mm("0.6"), nm_from_mm("0.3"))))
    moved = shove_via(board, 0, Point.mm("0.5", 0))
    assert moved.committed and moved.moved_via_indexes == (0, 1)
    rejected = shove_via(board, 0, Point.mm("0.5", 0), locked_via_indexes=frozenset({1}))
    assert not rejected.committed and rejected.board is board


def test_bundle_shove_preserves_all_members_and_propagates_by_net() -> None:
    nets = tuple(PhysicalNet(name, ()) for name in ("P", "N", "X"))
    board = PhysicalBoard("Bundle", BoardOutline.rectangle(12, 10), {}, (), nets,
                          tracks=(_track("P", "2"), _track("N", "2.5"), _track("X", "3")))
    result = shove_bundle(board, (0, 1), (), Point.mm(0, "0.4"))
    assert result.committed and result.moved_track_indexes == (0, 1, 2)
    assert result.board.tracks[0].start.y_nm - board.tracks[0].start.y_nm == nm_from_mm("0.4")


def test_acute_cleanup_removes_a_safe_degree_two_spike() -> None:
    nets = (PhysicalNet("A", ()),)
    tracks = (
        TrackSegment("A", Point.mm(2, 2), Point.mm(5, 5), nm_from_mm("0.2"), CopperLayer.FRONT),
        TrackSegment("A", Point.mm(5, 5), Point.mm(3, 2), nm_from_mm("0.2"), CopperLayer.FRONT),
    )
    board = PhysicalBoard("Cleanup", BoardOutline.rectangle(10, 10), {}, (), nets, tracks=tracks)
    result = cleanup_acute_angles(board)
    assert result.removed_track_count == 1 and len(result.board.tracks) == 1
