from dataclasses import replace
import pytest
from pcbir import BoardOutline, PhysicalBoard, PhysicalFootprint, Placement, Point, Size
from pcbir.physical import BoardSide, ComponentPlacementRule, PlacementRegion
from pcbir.placement import PlacementPlannerOptions, _exact_repair, _relative_candidates, placement_solution_is_legal


def rectangle(x, y, w, h):
    return BoardOutline.rectangle(w, h, origin=Point.mm(x, y))


def scene(prefix):
    small = PhysicalFootprint("land", (), Size.mm(1.5, 1.5))
    blocker = PhysicalFootprint("held-land", (), Size.mm(2, 2))
    target = Placement(prefix + "-trapped", small.name, Point.mm(13, 13), side=BoardSide.BACK)
    early = Placement(prefix + "-early", blocker.name, Point.mm(13, 13), side=BoardSide.BACK)
    tails = tuple(Placement(prefix + f"-tail-{i}", small.name, Point.mm(30 + i * 3, 30), side=BoardSide.BACK) for i in range(6))
    board = PhysicalBoard("RegionRepair", BoardOutline.rectangle(60, 50),
        {small.name: small, blocker.name: blocker}, (target, early, *tails), (),
        regions=(PlacementRegion("tight", rectangle(12,12,2,2), BoardSide.BACK),
                 PlacementRegion("flexible", rectangle(10,10,8,8), BoardSide.BACK)),
        placement_rules=(ComponentPlacementRule(target.reference, region="tight", side=BoardSide.BACK),
                         ComponentPlacementRule(early.reference, region="flexible", side=BoardSide.BACK)))
    held={p.reference:p for p in (early,*tails)}
    return board,target,early,tails,held


@pytest.mark.parametrize("prefix", ("renamed-a", "other"))
def test_region_only_repair_moves_actual_early_blocker_instead_of_unrelated_tail(prefix):
    board,target,early,tails,held=scene(prefix)
    options=PlacementPlannerOptions(exact_repair_limit=2)
    targets={p.reference:p for p in board.placements}
    assert _relative_candidates(board,target.reference,target,held,options)==()
    source_held=dict(held)
    result=_exact_repair(board,target.reference,held,list(held),targets,{},options,0)
    assert result is not None
    assert result[early.reference].position != early.position
    assert all(result[p.reference]==p for p in tails)
    assert placement_solution_is_legal(board,result,options)
    assert held==source_held


def test_fixed_actual_blocker_is_never_relaxed_or_moved():
    board,target,early,tails,held=scene("fixed")
    rule=replace(board.placement_rules[1],fixed_position=early.position,fixed_rotation_degrees=0)
    board=replace(board,placement_rules=(board.placement_rules[0],rule))
    options=PlacementPlannerOptions(exact_repair_limit=2)
    before=dict(held)
    result=_exact_repair(board,target.reference,held,list(held),{p.reference:p for p in board.placements},{},options,0)
    assert result is None and held==before


def test_supplemental_raw_blocker_probes_respect_existing_limit(monkeypatch):
    import pcbir.placement as implementation
    board,target,early,tails,held=scene("bounded")
    options=PlacementPlannerOptions(exact_repair_limit=2,exact_repair_candidates=3)
    real=implementation._legal
    checks=[]
    def observe(candidate,placed,board,options,**kwargs):
        if not placed: checks.append(candidate)
        return real(candidate,placed,board,options,**kwargs)
    monkeypatch.setattr(implementation,"_legal",observe)
    monkeypatch.setattr(implementation,"_pack_local_components",lambda *a,**k: None)
    implementation._exact_repair(board,target.reference,held,list(held),{p.reference:p for p in board.placements},{},options,0)
    assert len(checks)==options.exact_repair_candidates


def test_nonrectangular_region_and_effective_rotation_side_remain_legal_authority():
    board,target,early,tails,held=scene("polygon")
    small=PhysicalFootprint("triangle-land",(),Size.mm(.4,.4))
    target=replace(target,footprint=small.name,position=Point.mm(12.5,12.5))
    early=replace(early,position=target.position)
    triangle=BoardOutline((Point.mm(12,12),Point.mm(14,12),Point.mm(12,14)))
    rule=replace(board.placement_rules[0],allowed_orientations=(0,))
    board=replace(board,footprints={**board.footprints,small.name:small},
        placements=(target,early,*tails),placement_rules=(rule,board.placement_rules[1]),
        regions=(PlacementRegion("tight",triangle,BoardSide.BACK),board.regions[1]))
    held={p.reference:p for p in (early,*tails)}
    options=PlacementPlannerOptions(exact_repair_limit=2)
    result=_exact_repair(board,target.reference,held,list(held),{p.reference:p for p in board.placements},{},options,0)
    assert result is not None and placement_solution_is_legal(board,result,options)
    chosen=result[target.reference]
    assert chosen.side is BoardSide.BACK and chosen.rotation_degrees==0
    # A bounding rectangle alone would accept the opposite triangular corner.
    assert chosen.position.x_nm+chosen.position.y_nm+400_000 <= 26_000_000
    assert all(result[p.reference]==p for p in tails)
