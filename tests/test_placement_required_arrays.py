"""Placement reserves every required array site at its actual accepted pose."""
from dataclasses import replace
import pytest
from pcbir import (BoardOutline,CopperLayer,CopperZone,DesignRules,FootprintPad,
    PadReference,PhysicalBoard,PhysicalFootprint,PhysicalNet,Placement,Point,
    PolygonRing,PolygonWithHoles,Size,Stackup,nm_from_mm)
from pcbir.physical import BoardSide,PadViaInPadRule
from pcbir.placement_holes import footprint_holes_clear
from pcbir.pad_via_arrays import materialize_via_in_pad_arrays


def fixture():
    chip=PhysicalFootprint("array",(FootprintPad("EP",Point(0,0),Size.mm(3.1,3.1)),),Size.mm(4,4))
    passive=PhysicalFootprint("passive",(FootprintPad("1",Point(0,0),Size.mm(.2,.2)),),Size.mm(.2,.2))
    ep=PadReference("U1","EP")
    zone=CopperZone("ground","GND",(CopperLayer.INTERNAL_1,),PolygonWithHoles(PolygonRing(tuple(
        Point.mm(x,y) for x,y in ((1,1),(29,1),(29,19),(1,19))))))
    return PhysicalBoard("RequiredArray",BoardOutline.rectangle(30,20),{p.name:p for p in (chip,passive)},
        (Placement("U1","array",Point.mm(15,10)),Placement("C1","passive",Point.mm(16.1,10),side=BoardSide.BACK)),
        (PhysicalNet("GND",(ep,)),PhysicalNet("SIGNAL",(PadReference("C1","1"),))),
        stackup=Stackup((CopperLayer.FRONT,CopperLayer.INTERNAL_1,CopperLayer.INTERNAL_2,
                        CopperLayer.INTERNAL_3,CopperLayer.INTERNAL_4,CopperLayer.BACK)),zones=(zone,),
        rules=DesignRules(minimum_clearance_nm=90000,minimum_hole_clearance_nm=250000),
        via_in_pad_rules=(PadViaInPadRule(ep,"filled-capped",3,3,650000),),
        metadata={"fabrication_profile":"jlcpcb-six-layer"})


@pytest.mark.parametrize("gap_nm,legal",[(449999,False),(450000,True)])
@pytest.mark.parametrize("moving_owner",[False,True])
def test_required_array_drill_clearance_is_checked_symmetrically(gap_nm,legal,moving_owner):
    board=fixture();owner,passive=board.placements
    passive=replace(passive,position=Point(15650000+gap_nm,10000000))
    if moving_owner:
        candidate,held=owner,{passive.reference:passive}
    else:
        candidate,held=passive,{owner.reference:owner}
    assert footprint_holes_clear(board,candidate,held)==legal
    if legal:
        placed=replace(board,placements=(owner,passive))
        assert len(materialize_via_in_pad_arrays(placed).vias)==9


def test_array_reservation_uses_held_owner_pose_and_does_not_invent_optional_vias():
    board=fixture();owner,passive=board.placements
    seed=replace(owner,position=Point.mm(22,10))
    board=replace(board,placements=(seed,passive))
    blocked=replace(passive,position=Point.mm(15.65,10))
    assert footprint_holes_clear(board,blocked,{seed.reference:seed})
    assert not footprint_holes_clear(board,blocked,{owner.reference:owner})
    optional=replace(board,via_in_pad_rules=(PadViaInPadRule(PadReference("U1","EP")),))
    assert footprint_holes_clear(optional,blocked,{owner.reference:owner})


def test_array_contact_with_a_foreign_same_net_land_still_requires_clearance():
    board=fixture();owner,passive=board.placements
    passive=replace(passive,position=Point.mm(15.65,10))
    board=replace(board,nets=(replace(board.nets[0],pads=(*board.nets[0].pads,PadReference("C1","1"))),))
    assert not footprint_holes_clear(board,passive,{owner.reference:owner})


def test_filled_array_does_not_create_a_body_only_courtyard_exclusion():
    board=fixture();owner,passive=board.placements
    fp=replace(board.footprints["passive"],body_size=Size.mm(4,4))
    board=replace(board,footprints={**board.footprints,"passive":fp})
    assert footprint_holes_clear(board,passive,{owner.reference:owner})
    assert len(materialize_via_in_pad_arrays(board).vias)==9


def test_array_geometry_uses_only_supplied_accepted_owners():
    from pcbir.pad_via_arrays import via_in_pad_array_vias
    board=fixture();owner,passive=board.placements
    moved=replace(owner,position=Point.mm(20,10))
    original=via_in_pad_array_vias(board)
    actual=via_in_pad_array_vias(board,placements={owner.reference:moved})
    assert len(actual)==len(original)==9
    assert all(a.position.x_nm-b.position.x_nm==5000000 for a,b in zip(actual,original))
    assert via_in_pad_array_vias(board,placements={passive.reference:passive})==()
    assert via_in_pad_array_vias(board)==original


def test_generic_placement_moves_a_support_pad_off_required_array_sites():
    from pcbir.placement import generate_placement_candidates,PlacementPlannerOptions,placement_solution_is_legal
    board=fixture();owner,passive=board.placements
    passive=replace(passive,position=Point.mm(15.65,10))
    source=replace(board,placements=(owner,passive))
    options=PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,refinement_passes=0,
                                    grid_step_nm=250000,fixed_references=frozenset({"U1"}))
    selected=generate_placement_candidates(source,options)[0]
    poses={p.reference:p for p in selected.placements}
    assert poses["U1"]==owner and poses["C1"]!=passive
    assert placement_solution_is_legal(source,poses,options)
    assert len(materialize_via_in_pad_arrays(replace(source,placements=selected.placements)).vias)==9
    assert source.placements==(owner,passive) and not source.vias
