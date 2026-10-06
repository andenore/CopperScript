"""Hot-path shortcuts must answer exactly as the computations they replace."""
from dataclasses import replace

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, PhysicalBoard, Point,
                   PolygonRing, PolygonWithHoles, nm_from_mm)
from pcbir.detailed import DetailedNode, DetailedRouterOptions, _build_grid
from pcbir.fanout import _OutlineSites
from pcbir.geometry import point_in_polygon, point_segment_distance_squared
from pcbir.mechanical import point_in_material
from pcbir.physical import MechanicalHole, Stackup


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


def _reference_blocked(board: PhysicalBoard, xs, ys) -> frozenset[DetailedNode]:
    """The original per-layer construction of the blocked node set."""
    keepouts = tuple((item.layers, item.outline.outer.vertices) for item in board.copper_keepouts
                     if item.block_tracks)
    blocked = set()
    for layer_index, layer in enumerate(board.stackup.copper_layers):
        for x_index, x in enumerate(xs):
            for y_index, y in enumerate(ys):
                point = Point(x, y)
                if not point_in_material(board, point) or any(
                        layer in layers and point_in_polygon(point, polygon)
                        for layers, polygon in keepouts):
                    blocked.add(DetailedNode(layer_index, x_index, y_index))
    return frozenset(blocked)


def test_grid_blocked_nodes_match_the_per_layer_construction() -> None:
    ring = PolygonRing((Point.mm(2, 2), Point.mm(4, 2), Point.mm(4, 4), Point.mm(2, 4)))
    inner = PolygonRing((Point.mm(5, 1), Point.mm(7, 1), Point.mm(7, 3), Point.mm(5, 3)))
    layers = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)
    board = PhysicalBoard(
        "grid", BoardOutline.rectangle(10, 8), {}, (), (),
        stackup=Stackup(copper_layers=layers),
        copper_keepouts=(CopperKeepout("TOP", (CopperLayer.FRONT,), PolygonWithHoles(ring)),
                         CopperKeepout("MID", (CopperLayer.INTERNAL_1, CopperLayer.BACK),
                                       PolygonWithHoles(inner))),
        mechanical_holes=(MechanicalHole("H1", Point.mm(8, 6), nm_from_mm("1.2")),),
    )
    grid = _build_grid(board, DetailedRouterOptions(pitch_nm=nm_from_mm("0.5")), ())
    expected = _reference_blocked(board, grid.xs, grid.ys)
    assert grid.blocked == expected
    # Every layer differs: the keepouts name different layers and the hole blocks all.
    assert len({frozenset((n.x_index, n.y_index) for n in expected if n.layer_index == i)
                for i in range(len(layers))}) == 3


def test_grid_points_are_cached_per_grid_instance() -> None:
    board = PhysicalBoard("grid", BoardOutline.rectangle(4, 4), {}, (), ())
    grid = _build_grid(board, DetailedRouterOptions(pitch_nm=nm_from_mm(1)), ())
    node = DetailedNode(0, 2, 3)
    assert grid.point(node) == Point(grid.xs[2], grid.ys[3])
    assert grid.point(node) is grid.point(node)
    shifted = replace(grid, xs=tuple(x + 1 for x in grid.xs))
    # A replaced grid with different axes never reads the original's points.
    assert shifted.point(node) == Point(grid.xs[2] + 1, grid.ys[3])
