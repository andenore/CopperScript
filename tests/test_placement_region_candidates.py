from dataclasses import replace
from decimal import Decimal

import pytest
from pcbir import plan_placement, BoardOutline, PhysicalBoard, PhysicalFootprint, Placement, Point, Size
from pcbir.physical import BoardSide, ComponentPlacementRule, PlacementRegion
from pcbir.placement import PlacementPlannerOptions, _candidate_positions, _legal, _best_legal_choice, placement_solution_is_legal


def rectangle(x, y, width, height):
    return BoardOutline(tuple(Point.mm(a, b) for a, b in
        ((x, y), (x + width, y), (x + width, y + height), (x, y + height))))


def scene(name="small", x=66.5, y=7.5, width=4, height=4):
    fp = PhysicalFootprint("small-land", (), Size.mm(1.82, .92))
    target = Placement(name, fp.name, Point.mm(x + width / 2, y + height / 2), side=BoardSide.BACK)
    board = PhysicalBoard("RegionCoverage", BoardOutline.rectangle(100, 40), {fp.name: fp},
        (target,), (), regions=(PlacementRegion("bounded", rectangle(x, y, width, height), BoardSide.BACK),),
        placement_rules=(ComponentPlacementRule(name, region="bounded", side=BoardSide.BACK),))
    return board, target


@pytest.mark.parametrize("name", ("renamed-bypass", "arbitrary-component"))
def test_small_region_preserves_coarse_order_and_finds_legal_off_grid_gap(name):
    board, target = scene(name)
    obstacle = replace(target, reference="held-neighbor", position=Point.mm(68, 9), rotation_degrees=Decimal(90))
    board = replace(board, placements=(target, obstacle), placement_rules=board.placement_rules +
        (ComponentPlacementRule(obstacle.reference, fixed_position=obstacle.position,
            fixed_rotation_degrees=90, side=BoardSide.BACK),))
    options = PlacementPlannerOptions()
    points = _candidate_positions(board, target, options)
    coarse = tuple(sorted((Point.mm(x, y) for x in (68, 69) for y in (9, 10)),
        key=lambda p: (abs(p.x_nm-target.position.x_nm)+abs(p.y_nm-target.position.y_nm), p.y_nm, p.x_nm)))
    assert points[:4] == coarse
    assert len(set(points)) == len(points)
    assert len(points) * 4 <= options.legalization_candidates
    held = {obstacle.reference: obstacle}
    assert not any(_legal(replace(target, position=p, rotation_degrees=a), held, board, options)
        for p in coarse for a in (0, 90, 180, 270))
    chosen = _best_legal_choice(board, target.reference, target, held, {p.reference: p for p in board.placements}, {}, options, 0)
    assert chosen is not None and _legal(chosen, held, board, options)
    assert chosen.position not in coarse
    assert chosen.side is BoardSide.BACK


@pytest.mark.parametrize("width,height,angle", ((3, 1.2, 0), (1.2, 3, 90)))
def test_orientation_specific_boundary_survives_empty_square_envelope(width, height, angle):
    board, target = scene("other-name", x=10, y=10, width=width, height=height)
    options = PlacementPlannerOptions()
    points = _candidate_positions(board, target, options)
    assert points and len(points) * 4 <= options.legalization_candidates
    assert any(_legal(replace(target, position=p, rotation_degrees=angle), {}, board, options) for p in points)
    assert not any(_legal(replace(target, position=p, rotation_degrees=90-angle), {}, board, options) for p in points)
    assert not any(_legal(replace(target, position=p, rotation_degrees=angle, side=BoardSide.FRONT), {}, board, options) for p in points)


def test_impossible_region_remains_empty_and_no_extra_allowance_is_created():
    board, target = scene(width=.8, height=.8)
    assert _candidate_positions(board, target, PlacementPlannerOptions()) == ()
    board, target = scene()
    points = _candidate_positions(board, target, PlacementPlannerOptions(legalization_candidates=16))
    assert len(points) == 4


def test_large_and_unconstrained_domains_keep_existing_lattice_only():
    board, target = scene(x=10, y=10, width=20, height=20)
    points = _candidate_positions(board, target, PlacementPlannerOptions())
    assert len(points) > 96
    assert all(p.x_nm % 1_000_000 == p.y_nm % 1_000_000 == 0 for p in points)
    board = replace(board, placement_rules=())
    assert all(p.x_nm % 1_000_000 == p.y_nm % 1_000_000 == 0
        for p in _candidate_positions(board, target, PlacementPlannerOptions()))


def test_fixed_pose_and_orientation_remain_legal_authority():
    board, target = scene()
    rule = replace(board.placement_rules[0], fixed_position=Point.mm(68, 9), fixed_rotation_degrees=90)
    board = replace(board, placement_rules=(rule,))
    options = PlacementPlannerOptions()
    points = _candidate_positions(board, target, options)
    assert points
    result = plan_placement(board, replace(options, candidate_count=1,
        analytical_iterations=0, refinement_passes=0))
    pose = result.board.placements[0]
    assert pose.position == rule.fixed_position and pose.rotation_degrees == 90
    assert pose.side is BoardSide.BACK
    assert placement_solution_is_legal(board, {pose.reference: pose}, options)

