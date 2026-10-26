"""Hot-path shortcuts must answer exactly as the computations they replace."""

from pcbir import BoardOutline, Point, nm_from_mm
from pcbir.fanout import _OutlineSites
from pcbir.geometry import point_in_polygon, point_segment_distance_squared


def _reference_admits(point: Point, vertices: tuple[Point, ...], margin_nm: int) -> bool:
    """Inside the outline and at least the margin from every edge, exactly."""
    edges = zip(vertices, (*vertices[1:], vertices[0]))
    return point_in_polygon(point, vertices) and all(
        point_segment_distance_squared(point, first, second) >= margin_nm * margin_nm
        for first, second in edges)


def _sites(outline: BoardOutline) -> list[Point]:
    xs = [p.x_nm for p in outline.vertices]
    ys = [p.y_nm for p in outline.vertices]
    step = nm_from_mm("0.25")
    return [Point(x, y) for x in range(min(xs) - step, max(xs) + 2 * step, step)
            for y in range(min(ys) - step, max(ys) + 2 * step, step)]


def test_outline_sites_match_exact_inside_and_margin_tests() -> None:
    margin = nm_from_mm("0.315")
    for outline in (BoardOutline.rectangle(10, 8), BoardOutline.circle(6)):
        sites = _OutlineSites(outline.vertices)
        # Points exactly one margin inside an edge (tangency) and one nanometre short of it.
        probes = [*_sites(outline), Point(margin, nm_from_mm(4)), Point(margin - 1, nm_from_mm(4))]
        for point in probes:
            expected = _reference_admits(point, outline.vertices, margin)
            assert sites.admits(point, margin) is expected, point
            assert sites.admits(point, margin) is expected  # cached answer
        assert sites.admits(Point(margin, nm_from_mm(4)), margin) is (outline.circular_boundary is None)


