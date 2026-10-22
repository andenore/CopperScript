"""Package via-site lattices (escape plan E7)."""
from decimal import Decimal

from pcbir.drc import run_physical_drc
from pcbir.physical import CopperKeepout, Point, PolygonRing, PolygonWithHoles
from pcbir.via_lattice import plan_via_lattices

from test_escape_plan import ball_grid


def cell_centre(cx, cy):
    """Board position of cell (cx, cy) of ``ball_grid`` (0.8 mm pitch, ball (0, 0) at (10, 10) mm)."""
    return Point.mm(Decimal(10) + Decimal("0.8") * cx + Decimal("0.4"), Decimal(10) + Decimal("0.8") * cy + Decimal("0.4"))


def blocking(board, name, *points, half="0.1"):
    """A keepout on every copper layer of ``board`` over small squares around ``points``."""
    keepouts = []
    for number, point in enumerate(points):
        d = Decimal(half)
        x, y = Decimal(point.x_nm) / 1_000_000, Decimal(point.y_nm) / 1_000_000
        ring = PolygonRing((Point.mm(x - d, y - d), Point.mm(x + d, y - d), Point.mm(x + d, y + d), Point.mm(x - d, y + d)))
        keepouts.append(CopperKeepout(f"{name}-{number}", board.stackup.copper_layers, PolygonWithHoles(ring)))
    return tuple(keepouts)


def test_every_plane_ball_gets_its_own_outward_site_and_the_lattice_is_legal():
    board = ball_grid(8, "0.45", plane=frozenset(range(19, 47)))
    result = plan_via_lattices(board)
    (lattice,) = result.lattices
    # The 28 plane balls need a via (the edge signal balls escape on the surface).
    assert len(lattice.sites) == len(result.vias) == 28 and lattice.unsited == ()
    assert len({site.position for site in lattice.sites}) == 28       # one via per cell
    assert all(site.preferred for site in lattice.sites)              # quadrant to corner
    codes = {finding.code for finding in run_physical_drc(result.board).findings}
    assert codes <= {finding.code for finding in run_physical_drc(board).findings}


def test_an_edge_supply_ball_takes_the_cell_outside_the_array():
    result = plan_via_lattices(ball_grid(6, "0.45", plane=frozenset({1})))
    (site,) = [site for site in result.lattices[0].sites if site.pad.pad == "1"]
    assert site.cell == (-1, -1) and site.preferred


def test_the_assignment_gives_a_contested_cell_to_the_ball_that_has_no_other():
    # Ball 29 (column 4, row 4) keeps only cell (3, 3), the outward cell of ball 22 (3, 3).
    board = ball_grid(6, "0.45")
    keepouts = blocking(board, "block", cell_centre(4, 4), cell_centre(4, 3), cell_centre(3, 4))
    from dataclasses import replace
    board = replace(board, copper_keepouts=keepouts)
    sites = {site.pad.pad: site for site in plan_via_lattices(board, surface_rings=1, band_rings=0).lattices[0].sites}
    assert sites["29"].cell == (3, 3)
    assert "22" in sites and not sites["22"].preferred


def test_the_outer_band_keeps_signal_vias_out_when_inner_cells_are_free():
    def in_outer_ring(band):
        (lattice,) = plan_via_lattices(ball_grid(8, "0.45"), surface_rings=1, band_rings=band).lattices
        assert len(lattice.sites) == 36 and lattice.unsited == ()
        return sum(min(site.cell[0], site.cell[1], 6 - site.cell[0], 6 - site.cell[1]) == 0
                   for site in lattice.sites)
    assert in_outer_ring(1) < in_outer_ring(0)


def test_a_ball_without_a_legal_site_is_reported():
    from dataclasses import replace
    board = ball_grid(6, "0.45")
    keepouts = blocking(board, "block", *(cell_centre(cx, cy) for cx in (1, 2) for cy in (1, 2)))
    board = replace(board, copper_keepouts=keepouts)
    (lattice,) = plan_via_lattices(board).lattices
    unsited = dict((pad.pad, reason) for pad, reason in lattice.unsited)
    assert unsited.get("15") == "no legal site"   # ball (2, 2): all four of its cells are blocked
