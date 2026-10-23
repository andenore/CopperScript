"""Tolerant ball-grid detection (package_patterns.ball_grid)."""
from dataclasses import replace

from pcbir.package_patterns import ball_grid, circular_array
from pcbir.physical import FootprintPad, PadKind, PadShape, Point, Size

from test_escape_plan import ball_grid as grid_board


def with_pads(board, pads):
    """``board`` with these pads on its ball grid; nets of removed balls are dropped."""
    footprint = board.footprints["bga"]
    numbers = {p.number for p in pads}
    nets = tuple(net for net in board.nets if all(pad.component != "U" or pad.pad in numbers for pad in net.pads))
    return replace(board, footprints={**board.footprints, "bga": replace(footprint, pads=tuple(pads))}, nets=nets)


def test_a_complete_grid_is_found_as_before():
    board = grid_board(6, "0.45")
    grid = ball_grid(board, "U")
    strict = circular_array(board, "U")
    assert grid.xs_nm == strict.xs_nm and grid.pitch_nm == strict.pitch_nm == 800_000
    assert len(grid.balls) == 36


def test_rounding_in_footprint_coordinates_is_tolerated():
    board = grid_board(6, "0.45")
    # KiCad footprints round: some columns land a micrometre off the lattice.
    pads = [replace(p, position=Point(p.position.x_nm + (1_000 if int(p.number) % 3 == 0 else 0),
                                      p.position.y_nm - (1_000 if int(p.number) % 4 == 0 else 0)))
            for p in board.footprints["bga"].pads]
    board = with_pads(board, pads)
    assert circular_array(board, "U") is None
    grid = ball_grid(board, "U")
    assert grid is not None and abs(grid.pitch_nm - 800_000) <= 1_000 and len(grid.balls) == 36
    assert all(grid.index(p.position) is not None for p in pads)


def test_a_depopulated_grid_with_extra_pads_is_found():
    board = grid_board(6, "0.45")
    balls = [p for p in board.footprints["bga"].pads if p.number not in {"15", "16", "21", "22"}]  # empty centre
    extra = [FootprintPad("A1", Point.mm(-1, -1), Size.mm("0.13", "0.5"), kind=PadKind.SMD, shape=PadShape.OVAL),
             FootprintPad("A2", Point.mm(5, 5), Size.mm("0.13", "0.5"), kind=PadKind.SMD, shape=PadShape.OVAL)]
    board = with_pads(board, balls + extra)
    assert circular_array(board, "U") is None
    grid = ball_grid(board, "U")
    assert grid is not None and len(grid.xs_nm) == 6 and len(grid.balls) == 32
    assert "A1" not in grid.balls and grid.index(Point.mm(-1, -1)) is None


def test_sparse_or_rectangular_grids_are_not_ball_grids():
    board = grid_board(6, "0.45")
    sparse = [p for p in board.footprints["bga"].pads if int(p.number) % 3 == 0]  # 12 of 36
    assert ball_grid(with_pads(board, sparse), "U") is None
    stretched = [replace(p, position=Point(p.position.x_nm, p.position.y_nm * 65 // 80))
                 for p in board.footprints["bga"].pads]  # 0.8 x 0.65 mm
    assert ball_grid(with_pads(board, stretched), "U") is None
