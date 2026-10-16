from dataclasses import replace
import pytest
from pcbir import compile_source, plan_placement, PlacementPlannerOptions, Point
from pcbir.physical import BoardSide
from pcbir.physicalize import prototype_physicalize, PrototypePhysicalOptions


def source(extra='', *, order=False):
    region='constraint placement_region(R1, R2) { name = "bottom"; x = 2 mm; y = 2 mm; width = 16 mm; height = 16 mm; side = back; }'
    return compile_source('board BackRegion { use library "tiny"; component R1: RESISTOR { footprint = "0402"; } component R2: RESISTOR { footprint = "0402"; }'
        + (extra+region if order else region+extra) + '}')


def test_region_side_is_an_initial_and_refinement_domain_for_all_members():
    board=prototype_physicalize(source(),PrototypePhysicalOptions(board_width_mm=24,board_height_mm=22,margin_mm=3))
    assert all(p.side is BoardSide.BACK for p in board.placements)
    assert all(r.side is BoardSide.BACK and r.region=='bottom' for r in board.placement_rules)
    planned=plan_placement(board,PlacementPlannerOptions(candidate_count=1,analytical_iterations=2,refinement_passes=1))
    assert all(p.side is BoardSide.BACK for p in planned.board.placements)
    assert planned.report.metrics.constraint_penalty_nm==0


@pytest.mark.parametrize('order',(False,True))
def test_region_cannot_override_a_contradictory_fixed_source_side(order):
    fixed='constraint fixed_placement(R1) { x = 8 mm; y = 8 mm; side = front; }'
    with pytest.raises(ValueError,match='conflicting side'):
        prototype_physicalize(source(fixed,order=order))


def test_region_keeps_a_consistent_fixed_pose_and_rotation():
    fixed='constraint fixed_placement(R1) { x = 8 mm; y = 8 mm; rotation = 90; side = back; }'
    board=prototype_physicalize(source(fixed))
    pose=next(p for p in board.placements if p.reference=='R1')
    assert pose.position==Point.mm(8,8) and pose.side is BoardSide.BACK and pose.rotation_degrees==90
