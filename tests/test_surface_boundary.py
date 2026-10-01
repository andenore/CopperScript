import random

from pcbir import BoardOutline, PhysicalBoard, Point
from pcbir.geometry import point_in_polygon, segment_distance_squared
from pcbir.surface_path import _track_inside_board, _rectangle_bounds, via_inside_board


def exact(board, start, end, width):
    vertices = board.outline.vertices
    required = width + 2 * board.rules.minimum_clearance_nm
    return (point_in_polygon(start, vertices) and point_in_polygon(end, vertices)
            and all(4 * segment_distance_squared(start, end, first, second) >= required * required
                    for first, second in zip(vertices, (*vertices[1:], vertices[0]))))


def test_rectangular_fast_path_matches_exact_capsule_clearance():
    board = PhysicalBoard("Boundary", BoardOutline.rectangle(30, 20, origin=Point.mm(-2, 3)), {}, (), ())
    generator = random.Random(17)
    for width in (1, 2, 199999, 200000, 200001):
        for _ in range(80):
            start = Point(generator.randrange(-3000000, 29000000), generator.randrange(2000000, 24000000))
            end = Point(generator.randrange(-3000000, 29000000), generator.randrange(2000000, 24000000))
            assert _track_inside_board(board, start, end, width) == exact(board, start, end, width)
            assert via_inside_board(board, start, width) == exact(board, start, start, width)


def test_odd_nm_and_edge_tangency_are_not_rounded_down():
    board = PhysicalBoard("Boundary", BoardOutline.rectangle(30, 20), {}, (), ())
    margin = board.rules.minimum_clearance_nm + 100000
    start, end = Point(margin, 2000000), Point(margin, 4000000)
    assert _track_inside_board(board, start, end, 200000)
    assert not _track_inside_board(board, start, end, 200001)
    assert not via_inside_board(board, start, 200001)


def test_concave_and_bowtie_outlines_never_use_rectangle_bounds():
    vertices = (Point.mm(0, 0), Point.mm(10, 0), Point.mm(10, 10),
                Point.mm(6, 10), Point.mm(6, 3), Point.mm(4, 3), Point.mm(4, 10), Point.mm(0, 10))
    board = PhysicalBoard("Boundary", BoardOutline(vertices), {}, (), ())
    assert _rectangle_bounds(vertices) is None
    assert not _track_inside_board(board, Point.mm(2, 8), Point.mm(8, 8), 200000)
    assert _rectangle_bounds((Point.mm(0, 0), Point.mm(10, 10), Point.mm(0, 10), Point.mm(10, 0))) is None


def test_rectangle_recognition_preserves_vertex_order_and_origin():
    vertices = BoardOutline.rectangle(30, 20, origin=Point.mm(-2, 3)).vertices
    expected = (-2000000, 3000000, 28000000, 23000000)
    for index in range(4):
        shifted = vertices[index:] + vertices[:index]
        assert _rectangle_bounds(shifted) == expected
        assert _rectangle_bounds(tuple(reversed(shifted))) == expected
