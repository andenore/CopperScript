"""Escape capacity certificate (escape_capacity)."""
from pcbir.escape_capacity import GridGeometry, escape_certificate, ring_of


def geometry(n, land="0.45", via=450_000):
    from pcbir import nm_from_mm
    return GridGeometry(n, n, 800_000, nm_from_mm(land), via, 100_000, 100_000)


def balls(n):
    return {(x, y) for x in range(n) for y in range(n)}


def test_open_gaps_let_every_ball_of_a_small_grid_out_on_the_surface():
    # 0.8 mm pitch, 0.45 mm lands: 0.35 mm gaps hold one 0.1/0.1 track each.
    cert = escape_certificate(geometry(4), balls(4), (), (), ("In1",), "F")
    assert cert.feasible and cert.demand == 16
    cut = next(c for c in cert.cuts if c.layer == "F" and c.ring == 1)
    assert cut.capacity == 12 and cut.used == 4        # the 4 inner balls cross the 12 outer gaps


def test_closed_gaps_keep_inner_balls_without_vias_in():
    cert = escape_certificate(geometry(6, land="0.65"), balls(6), (), (), ("In1",), "F")
    outer = sum(1 for ball in balls(6) if ring_of(*ball, 6, 6) == 0)
    assert cert.escaped == outer and not cert.feasible


def test_vias_take_inner_balls_down_and_close_the_surface_gaps_beside_them():
    n = 6
    inner = {ball for ball in balls(n) if ring_of(*ball, n, n) >= 1}
    # One via per inner ball, in its outward cell.
    def outward(ball):
        x, y = ball
        return (x - (2 * x < n - 1), y - (2 * y < n - 1))
    vias = {outward(ball) for ball in inner}
    cert = escape_certificate(geometry(n, land="0.65"), balls(n), inner, vias, ("In1",), "F")
    assert cert.feasible
    assert cert.by_layer.get("In1") == len(inner)
    # The ball-layer gaps next to the vias are closed.
    assert next(c for c in cert.cuts if c.layer == "F" and c.ring == 1).capacity == 0


def test_a_saturated_via_layer_is_reported():
    n = 6
    vias = {(cx, cy) for cx in range(n - 1) for cy in range(n - 1)}          # every site taken
    inner = {ball for ball in balls(n) if ring_of(*ball, n, n) >= 1}
    tight = GridGeometry(n, n, 800_000, 650_000, 700_000, 100_000, 100_000)  # 0.1 mm between vias: no track
    cert = escape_certificate(tight, inner, inner, vias, ("In1",), "F")
    assert cert.escaped == 0 and all(c.capacity == 0 for c in cert.cuts if c.layer == "In1")
