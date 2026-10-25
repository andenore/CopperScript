from __future__ import annotations

import json
from dataclasses import replace

import pytest

from pcbir import (
    BoardOutline,
    CopperLayer,
    FootprintPad,
    PhysicalBoard,
    PhysicalFootprint,
    Placement,
    Point,
    Size,
    Stackup,
    nm_from_mm,
)
from pcbir.fine_regions import (
    FineRegionOptions,
    base_lattice_bounds,
    candidate_factors,
    plan_fine_regions,
)
from pcbir.physical import PadShape
from pcbir.unified_router import choose_pitch

BASE = nm_from_mm("0.25")
# Non-array dense packages are planned only with dense_packages (the router's
# fine_regions="dense"); ball arrays always qualify.
DENSE = FineRegionOptions(dense_packages=True)


def _ball_array(n: int, pitch: str, name: str | None = None) -> PhysicalFootprint:
    # Full n x n grid of circular SMD balls centred on the origin.
    step = nm_from_mm(pitch)
    half = (n - 1) * step // 2
    pads = tuple(FootprintPad(f"R{row}C{col}", Point(col * step - half, row * step - half),
                              Size.mm("0.35", "0.35"), shape=PadShape.CIRCLE)
                 for row in range(n) for col in range(n))
    width = nm_from_mm(1) + 2 * half
    return PhysicalFootprint(name or f"test/bga{n * n}_{pitch}", pads, Size(width, width))


def _quad(pins_per_side: int, pitch: str, row_offset: str, pad: tuple[str, str], name: str,
          exposed: str | None = None) -> PhysicalFootprint:
    # Quad perimeter package: pins numbered counter-clockwise from the left side.
    step, offset = nm_from_mm(pitch), nm_from_mm(row_offset)
    half = (pins_per_side - 1) * step // 2
    length, width = Size.mm(*pad).width_nm, Size.mm(*pad).height_nm
    pads, number = [], 1
    for side in range(4):
        for i in range(pins_per_side):
            along = i * step - half
            if side == 0:
                position, size = Point(-offset, along), Size(length, width)
            elif side == 1:
                position, size = Point(along, offset), Size(width, length)
            elif side == 2:
                position, size = Point(offset, -along), Size(length, width)
            else:
                position, size = Point(-along, -offset), Size(width, length)
            pads.append(FootprintPad(str(number), position, size))
            number += 1
    if exposed:
        pads.append(FootprintPad(str(number), Point(0, 0), Size.mm(exposed, exposed)))
    body = 2 * offset
    return PhysicalFootprint(name, tuple(pads), Size(body, body))


def _qfn32() -> PhysicalFootprint:
    # KiCad QFN-32-1EP_5x5mm_P0.5mm: pads 0.875 x 0.25 at 2.4375 mm from the centre.
    return _quad(8, "0.5", "2.4375", ("0.875", "0.25"), "test/qfn32_0.5", exposed="3.45")


def _qfp32_1mm() -> PhysicalFootprint:
    return _quad(8, "1.0", "5.0", ("1.5", "0.55"), "test/qfp32_1.0")


def _r0402() -> PhysicalFootprint:
    return PhysicalFootprint("test/0402", (FootprintPad("1", Point.mm("-0.5", 0), Size.mm("0.5", "0.55")),
                                           FootprintPad("2", Point.mm("0.5", 0), Size.mm("0.5", "0.55"))),
                             Size.mm("1.0", "0.5"))


def _board(parts, *, width=100, height=80, layers=6) -> PhysicalBoard:
    """parts: (reference, footprint, x_mm, y_mm[, rotation])."""
    footprints, placements = {}, []
    for reference, footprint, x, y, *rest in parts:
        footprints[footprint.name] = footprint
        placements.append(Placement(reference, footprint.name, Point.mm(x, y), rest[0] if rest else 0))
    return PhysicalBoard(
        "FineRegions", BoardOutline.rectangle(width, height), footprints, tuple(placements), (),
        stackup=Stackup(copper_layers=(CopperLayer.FRONT,
                                       *(CopperLayer(f"In{i}.Cu") for i in range(1, layers - 1)),
                                       CopperLayer.BACK)))


def _grid_nodes(board: PhysicalBoard, pitch: int) -> int:
    # The _Grid formula, written out: x0 = floor(min_x / pitch) * pitch,
    # nx = (max_x - x0) // pitch + 2, times the copper layers.
    xs = [p.x_nm for p in board.outline.vertices]
    ys = [p.y_nm for p in board.outline.vertices]
    x0 = (min(xs) // pitch) * pitch
    y0 = (min(ys) // pitch) * pitch
    return ((max(xs) - x0) // pitch + 2) * ((max(ys) - y0) // pitch + 2) * len(board.stackup.copper_layers)


def _snapped(patch, board, base=BASE) -> bool:
    lattice = base_lattice_bounds(board, base)
    return all((v - origin) % base == 0 for v, origin in ((patch.x0_nm, lattice.x0_nm), (patch.x1_nm, lattice.x0_nm),
                                                          (patch.y0_nm, lattice.y0_nm), (patch.y1_nm, lattice.y0_nm)))


def test_base_lattice_bounds_match_the_router_grid() -> None:
    board = _board((("U1", _ball_array(13, "0.75"), 50, 40),), width="100.1", height="79.9")
    bounds = base_lattice_bounds(board, BASE)
    try:
        from pcbir.unified_router import _Grid
    except ImportError:  # the lattice refactor may retire _Grid
        _Grid = None
    if _Grid is not None:
        grid = _Grid(board, BASE)
        assert (bounds.x0_nm, bounds.y0_nm, bounds.nx, bounds.ny) == (grid.x0, grid.y0, grid.nx, grid.ny)
    assert bounds.plane * 6 == _grid_nodes(board, BASE)
    assert candidate_factors(BASE, 8) == (2, 4, 5, 8)


def test_075_mm_169_ball_array_gets_one_k2_patch_with_margin() -> None:
    board = _board((("U1", _ball_array(13, "0.75"), 50, 40),))
    plan = plan_fine_regions(board, BASE)
    assert len(plan.patches) == 1
    patch = plan.patches[0]
    assert patch.factor == 2 and patch.pitch_nm(BASE) == nm_from_mm("0.125") == choose_pitch(board)
    assert patch.references == ("U1",) and patch.layers is None
    # Balls span 45.5..54.5 mm, copper +-0.175 mm, margin 1.5 mm, snapped outward to 0.25 mm.
    assert (patch.x0_nm, patch.y0_nm, patch.x1_nm, patch.y1_nm) == (
        nm_from_mm("43.75"), nm_from_mm("33.75"), nm_from_mm("56.25"), nm_from_mm("46.25"))
    assert _snapped(patch, board)
    assert patch.waste_nodes == 0


def test_04_mm_array_gets_the_pitch_choose_pitch_picks_for_the_whole_board() -> None:
    board = _board((("U1", _ball_array(12, "0.4"), 50, 40),))
    plan = plan_fine_regions(board, BASE)
    assert len(plan.patches) == 1
    patch = plan.patches[0]
    assert patch.pitch_nm(BASE) == choose_pitch(board) == nm_from_mm("0.05")
    assert patch.factor == 5 and "k=5" in patch.reason
    # The plan's motivating number: a 100 x 80 mm six-layer board at 0.05 mm is about 19 M nodes.
    assert plan.uniform_nodes_at_finest == _grid_nodes(board, nm_from_mm("0.05"))
    assert abs(plan.uniform_nodes_at_finest - 19_000_000) / 19_000_000 < 0.1
    assert plan.total_nodes < plan.uniform_nodes_at_finest / 10


@pytest.mark.parametrize("pitch", ["0.75", "0.8", "0.5", "1.0"])
def test_array_factor_agrees_with_choose_pitch_on_single_package_boards(pitch) -> None:
    board = _board((("U1", _ball_array(9, pitch), 50, 40),))
    plan = plan_fine_regions(board, BASE)
    assert plan.finest_pitch_nm == choose_pitch(board)
    if choose_pitch(board) == BASE:
        assert not plan.patches and "aligned at the base pitch" in plan.skipped[0][1]


def test_off_lattice_array_falls_back_to_half_the_ball_pitch() -> None:
    board = _board((("U1", _ball_array(13, "0.75"), "50.01", "40.02"),))
    plan = plan_fine_regions(board, BASE)
    assert choose_pitch(board) == BASE          # the board-wide rule gives up here
    assert [p.factor for p in plan.patches] == [2]
    assert "fallback k=2" in plan.patches[0].reason


def test_05_mm_qfn32_gets_a_patch() -> None:
    board = _board((("U2", _qfn32(), 30, 30),))
    plan = plan_fine_regions(board, BASE, DENSE)
    assert len(plan.patches) == 1
    patch = plan.patches[0]
    # Pins sit 2.4375 mm from the centre: only 0.0625 mm puts nodes on them.
    assert patch.factor == 4 and patch.references == ("U2",)
    assert "32 pins at 0.5 mm" in patch.reason
    assert _snapped(patch, board)
    # Pad copper reaches 2.875 mm from the centre; plus the margin, snapped outward.
    assert (patch.x0_nm, patch.x1_nm) == (nm_from_mm("25.5"), nm_from_mm("34.5"))
    rotated = plan_fine_regions(_board((("U2", _qfn32(), 30, 30, 90),)), BASE, DENSE)
    assert [(p.factor, p.x0_nm, p.y0_nm, p.x1_nm, p.y1_nm) for p in rotated.patches] == \
        [(p.factor, p.x0_nm, p.y0_nm, p.x1_nm, p.y1_nm) for p in plan.patches]


def test_coarse_qfp_and_0402_field_get_no_patch() -> None:
    parts = [("U3", _qfp32_1mm(), 30, 30)]
    parts += [(f"R{i}", _r0402(), 50 + (i % 10) * 1.2, 50 + (i // 10) * 0.8) for i in range(30)]
    board = _board(parts)
    plan = plan_fine_regions(board, BASE)
    assert plan.patches == () and plan.skipped == () and plan.fine_nodes == 0
    # Even with the spacing and pin-count tests relaxed, an 0402 pad field is too small.
    loose = plan_fine_regions(board, BASE, FineRegionOptions(pad_pitch_threshold_nm=nm_from_mm("1.2"),
                                                             minimum_pads=2))
    assert loose.patches == ()
    assert {ref for ref, why in loose.skipped if "minimum area" in why} == {f"R{i}" for i in range(30)}


def test_two_packages_03_mm_apart_merge_with_the_larger_factor() -> None:
    # The QFN sits on the 0.0625 mm lattice (k=4); the BGA is 0.15 mm off its
    # 0.125 mm lattice (fallback k=2) so that its copper (ending at 54.825 mm)
    # is exactly 0.3 mm from the QFN's (starting at 58 - 2.875 = 55.125 mm).
    bga = _ball_array(13, "0.75")
    board = _board((("U1", bga, "50.15", 40), ("U2", _qfn32(), 58, 40)))
    plan = plan_fine_regions(board, BASE, DENSE)
    assert len(plan.patches) == 1
    patch = plan.patches[0]
    assert patch.references == ("U1", "U2") and patch.factor == 4
    assert patch.reason.startswith("merged 2 packages")
    single = plan_fine_regions(_board((("U1", bga, "50.15", 40),)), BASE).patches[0]
    assert single.factor == 2
    assert patch.x0_nm == single.x0_nm and patch.x1_nm > single.x1_nm
    assert patch.waste_nodes > 0      # the QFN patch is shorter than the BGA's
    assert _snapped(patch, board)


def test_merging_factors_without_a_common_lattice_keeps_the_largest_and_says_so() -> None:
    board = _board((("U1", _ball_array(13, "0.75"), 50, 40), ("U2", _ball_array(12, "0.4"), 59, 40)))
    plan = plan_fine_regions(board, BASE)
    assert len(plan.patches) == 1
    patch = plan.patches[0]
    assert patch.factor == 5 and "U1 lose exact alignment" in patch.reason


def test_package_half_off_the_outline_is_clipped() -> None:
    board = _board((("U1", _ball_array(13, "0.75"), 2, 40),))
    plan = plan_fine_regions(board, BASE)
    patch = plan.patches[0]
    assert patch.x0_nm == 0 and patch.x1_nm == nm_from_mm("8.25")
    assert _snapped(patch, board)
    corner = _board((("U1", _ball_array(13, "0.75"), 99, 79),))
    patch = plan_fine_regions(corner, BASE).patches[0]
    assert (patch.x1_nm, patch.y1_nm) == (nm_from_mm(100), nm_from_mm(80))
    lattice = base_lattice_bounds(corner, BASE)
    assert patch.x1_nm <= lattice.x1_nm and patch.y1_nm <= lattice.y1_nm


def test_one_bga_on_a_100_by_80_board_costs_a_third_of_uniform_refinement() -> None:
    board = _board((("U1", _ball_array(13, "0.75"), 50, 40),))
    plan = plan_fine_regions(board, BASE)
    assert plan.base_nodes == _grid_nodes(board, BASE) == 402 * 322 * 6
    assert plan.uniform_nodes_at_finest == _grid_nodes(board, nm_from_mm("0.125")) == 802 * 642 * 6
    patch = plan.patches[0]
    cx, cy = patch.cells(BASE)
    assert plan.fine_nodes == ((2 * cx + 1) * (2 * cy + 1) - (cx + 1) * (cy + 1)) * 6
    ratio = plan.uniform_nodes_at_finest / (plan.base_nodes + plan.fine_nodes)
    assert 3 <= ratio <= 4.5
    report = plan.report()
    assert report["uniform_to_nested_ratio"] == round(ratio, 3)


def test_report_is_json_ready() -> None:
    board = _board((("U1", _ball_array(13, "0.75"), 50, 40), ("U2", _qfn32(), 20, 20),
                    ("U3", _ball_array(9, "1.0"), 80, 60)))
    report = plan_fine_regions(board, BASE, DENSE).report()
    assert json.loads(json.dumps(report)) == report
    assert [row["references"] for row in report["patches"]] == [["U1"], ["U2"]]
    assert report["patches"][0]["pitch_mm"] == 0.125 and report["patches"][0]["width_mm"] == 12.5
    assert report["skipped"] == [{"reference": "U3", "reason": "9x9 ball array at 1 mm: aligned at the base pitch"}]
    assert report["finest_pitch_mm"] == 0.0625

    def plain(value):
        if isinstance(value, dict):
            return all(isinstance(k, str) and plain(v) for k, v in value.items())
        if isinstance(value, list):
            return all(plain(v) for v in value)
        return type(value) in (int, float, str, bool)
    assert plain(report)


def test_disabled_plan_is_empty() -> None:
    board = _board((("U1", _ball_array(13, "0.75"), 50, 40),))
    plan = plan_fine_regions(board, BASE, FineRegionOptions(enabled=False))
    assert plan.patches == () and plan.fine_nodes == 0
    assert plan.base_nodes == plan.uniform_nodes_at_finest == _grid_nodes(board, BASE)
    assert plan.finest_pitch_nm == BASE


def test_plan_is_deterministic_and_independent_of_placement_order() -> None:
    parts = (("U1", _ball_array(13, "0.75"), 50, 40), ("U2", _qfn32(), 20, 20),
             ("U4", _qfn32(), "24.5", 20), ("U3", _ball_array(12, "0.4"), 80, 60))
    board = _board(parts)
    first = plan_fine_regions(board, BASE, DENSE)
    assert first == plan_fine_regions(board, BASE, DENSE)
    shuffled = replace(board, placements=tuple(reversed(board.placements)))
    assert plan_fine_regions(shuffled, BASE, DENSE) == first
    assert [p.references for p in first.patches] == [("U1",), ("U2", "U4"), ("U3",)]


def _first_ball_on_lattice(footprint, rotation) -> PhysicalBoard:
    """A rotated array placed so that its first pad lands exactly on the base lattice."""
    from decimal import Decimal
    from pcbir.placement import transformed_local_point
    board = _board((("U1", footprint, 50, 40, Decimal(rotation)),))
    pose = board.placements[0]
    corner = transformed_local_point(pose, footprint.pads[0].position)
    shift = Point(pose.position.x_nm + (-corner.x_nm) % BASE, pose.position.y_nm + (-corner.y_nm) % BASE)
    board = replace(board, placements=(replace(pose, position=shift),))
    moved = transformed_local_point(board.placements[0], footprint.pads[0].position)
    assert moved.x_nm % BASE == 0 and moved.y_nm % BASE == 0
    return board


def test_rotated_array_alignment_checks_every_ball_not_the_first_one() -> None:
    # Unrotated, a 1.0 mm array whose corner sits on the lattice is aligned at
    # the base pitch (channel centre lines at 0.5 mm are lattice points).
    footprint = _ball_array(9, "1.0")
    aligned = plan_fine_regions(_board((("U1", footprint, 50, 40),)), BASE)
    assert aligned.patches == () and aligned.skipped[0][1].endswith("aligned at the base pitch")
    # Rotated 45 degrees with the first ball on the lattice, every other ball
    # and channel sits at irrational offsets: the array needs a patch.
    rotated = plan_fine_regions(_first_ball_on_lattice(footprint, 45), BASE)
    assert len(rotated.patches) == 1 and "fallback" in rotated.patches[0].reason
    # The verdict must not depend on which pad is listed first.
    reversed_pads = replace(footprint, pads=tuple(reversed(footprint.pads)))
    board = _first_ball_on_lattice(footprint, 45)
    board = replace(board, footprints={reversed_pads.name: reversed_pads})
    permuted = plan_fine_regions(board, BASE)
    assert [(p.factor, p.x0_nm, p.y0_nm, p.x1_nm, p.y1_nm) for p in permuted.patches] == \
        [(p.factor, p.x0_nm, p.y0_nm, p.x1_nm, p.y1_nm) for p in rotated.patches]
    assert permuted.patches[0].reason == rotated.patches[0].reason


def test_quarter_turn_rotations_keep_exact_alignment() -> None:
    from decimal import Decimal
    footprint = _ball_array(13, "0.75")
    for rotation in (0, 90, 180, 270):
        plan = plan_fine_regions(_board((("U1", footprint, 50, 40, Decimal(rotation)),)), BASE)
        assert len(plan.patches) == 1 and plan.patches[0].factor == 2, rotation
        assert "puts nodes on pin centres" in plan.patches[0].reason, rotation


def test_default_plans_ball_arrays_only_and_names_the_dense_packages_it_skips() -> None:
    parts = (("U1", _ball_array(13, "0.75"), 50, 40), ("U2", _qfn32(), 20, 20), ("U3", _qfp32_1mm(), 80, 60))
    board = _board(parts)
    arrays = plan_fine_regions(board, BASE)
    assert FineRegionOptions().dense_packages is False
    assert [p.references for p in arrays.patches] == [("U1",)]
    # The QFN is listed with the factor dense would give it; the 1 mm QFP is no candidate.
    assert arrays.skipped == (("U2", '32 pins at 0.5 mm: would get k=4; dense packages are refined only '
                                     'with dense_packages (fine_regions="dense")'),)
    dense = plan_fine_regions(board, BASE, DENSE)
    assert [(p.references, p.factor) for p in dense.patches] == [(("U1",), 2), (("U2",), 4)]
    assert dense.patches[0] == arrays.patches[0] and dense.skipped == ()
    # A board with only dense packages plans nothing by default.
    qfn_only = plan_fine_regions(_board((("U2", _qfn32(), 20, 20),)), BASE)
    assert qfn_only.patches == () and qfn_only.total_nodes == qfn_only.base_nodes
    with pytest.raises(ValueError, match="dense_packages"):
        FineRegionOptions(dense_packages="yes")


def test_choose_pitch_judges_a_45_degree_array_by_every_ball_not_the_first() -> None:
    # A 0.75 mm array rotated 45 degrees with its first ball on the lattice:
    # (0.75 // 2) % 0.125 == 0 and that ball is a 0.125 mm lattice point, so
    # the first-ball test called it aligned at 0.125 mm. Every other ball sits
    # at an irrational offset: no candidate aligns it and the pitch stays.
    footprint = _ball_array(9, "0.75")
    board = _first_ball_on_lattice(footprint, 45)
    assert choose_pitch(board) == BASE
    # Listing the pads in another order does not change the verdict.
    reversed_pads = replace(footprint, pads=tuple(reversed(footprint.pads)))
    assert choose_pitch(replace(board, footprints={reversed_pads.name: reversed_pads})) == BASE
    # Quarter turns stay exact: 0.125 mm puts nodes on every ball and channel.
    from decimal import Decimal
    for rotation in (0, 90, 180, 270):
        assert choose_pitch(_board((("U1", footprint, 50, 40, Decimal(rotation)),))) == nm_from_mm("0.125")
    # The planner agrees: the rotated array gets the fallback patch.
    assert "fallback" in plan_fine_regions(board, BASE).patches[0].reason
