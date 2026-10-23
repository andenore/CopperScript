"""Exact ball-grid escape (exact_escape), with the optional OR-Tools solver."""
import pytest

pytest.importorskip("ortools")

from pcbir.escape_capacity import GridGeometry
from pcbir.exact_escape import solve_exact_escape


def test_inner_balls_take_their_vias_down_when_the_surface_is_closed():
    # 6x6 at 0.8 mm with 0.65 mm lands: no track between balls; inner balls must use their via.
    geometry = GridGeometry(6, 6, 800_000, 650_000, 450_000, 100_000, 100_000)
    inner = [(x, y) for x in range(1, 5) for y in range(1, 5)]
    outward = {(x, y): (x - (2 * x < 5), y - (2 * y < 5)) for x, y in inner}
    balls = {ball: (f"N{k}", "left") for k, ball in enumerate(inner)}
    result = solve_exact_escape(geometry, balls, outward, set(outward.values()), ("In1",), "F", time_limit_s=30)
    assert result.escaped == len(inner) and all(path.layer == "In1" for path in result.paths)
    assert all(path.nodes[-1][0] in (-1, 6) or path.nodes[-1][1] in (-1, 6) for path in result.paths)


def test_a_full_edge_load_blocks_a_crossing():
    geometry = GridGeometry(4, 4, 800_000, 450_000, 450_000, 100_000, 100_000)
    balls = {(1, 1): ("A", "left")}
    # Ball (1, 1) has no via; block every ball-layer edge: it cannot leave.
    load = {("F", (cx, cy), (cx + dx, cy + dy)): 9 for cx in range(-1, 4) for cy in range(-1, 4)
            for dx, dy in ((1, 0), (0, 1))}
    result = solve_exact_escape(geometry, balls, {}, set(), ("In1",), "F", time_limit_s=10, edge_load=load)
    assert result.escaped == 0 and result.unescaped == ((1, 1),)
