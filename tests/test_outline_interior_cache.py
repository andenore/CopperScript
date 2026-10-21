"""The interior-cell fast path of shape_in_outline never changes an answer."""
import random

import pcbir.mechanical as M
from pcbir.geometry import RoundedConvexShape
from pcbir.physical import BoardOutline, Point, nm_from_mm

MM = nm_from_mm(1)


def _outlines():
    rect = BoardOutline((Point(0, 0), Point(22 * MM, 0), Point(22 * MM, 8 * MM), Point(0, 8 * MM)))
    # Concave L with an off-grid notch, so cells straddle edges at odd positions.
    ell = BoardOutline((Point(0, 0), Point(30 * MM, 0), Point(30 * MM, 12_345_678), Point(13_300_000, 12_345_678),
                        Point(13_300_000, 30 * MM), Point(0, 30 * MM)))
    slanted = BoardOutline((Point(0, 0), Point(25 * MM, 3 * MM), Point(20 * MM, 21 * MM), Point(2 * MM, 17 * MM)))
    return rect, ell, slanted


def test_fast_and_exact_answers_agree():
    rng = random.Random(7)
    exact = M._box_in_interior
    for outline in _outlines():
        xs = [p.x_nm for p in outline.vertices]
        ys = [p.y_nm for p in outline.vertices]
        fast_hits = 0
        for _ in range(1500):
            spine = tuple(Point(rng.randint(min(xs) - MM, max(xs) + MM), rng.randint(min(ys) - MM, max(ys) + MM))
                          for _ in range(rng.choice((1, 2))))
            shape = RoundedConvexShape(spine, rng.randint(0, 600_000))
            clearance = rng.choice((0, 200_000, 500_000))
            M._box_in_interior = lambda *args: False
            try:
                reference = M.shape_in_outline(shape, outline, clearance)
            finally:
                M._box_in_interior = exact
            fast_hits += exact(outline, min(p.x_nm for p in spine) - shape.radius_nm - clearance,
                               min(p.y_nm for p in spine) - shape.radius_nm - clearance,
                               max(p.x_nm for p in spine) + shape.radius_nm + clearance,
                               max(p.y_nm for p in spine) + shape.radius_nm + clearance)
            assert M.shape_in_outline(shape, outline, clearance) == reference, (outline, shape, clearance)
        assert fast_hits > 100  # the fast path is actually taken


def test_a_cell_touching_the_edge_is_not_interior():
    rect = _outlines()[0]
    assert M._cell_inside(rect, (3, 3))
    assert not M._cell_inside(rect, (0, 3))      # its left side lies on the edge x = 0
    assert not M._cell_inside(rect, (21, 7))     # corner cell
    assert not M._cell_inside(rect, (25, 3))     # outside
