"""Incremental drill placement uses both faces and actual accepted owner poses."""
from dataclasses import replace
from math import sqrt
import pytest

from pcbir.drc import non_plated_holes, placement_copper_findings, placed_pad_shape
from pcbir.geometry import shape_distance_squared, shapes_clear
from pcbir.physical import (
    BoardOutline, BoardSide, ComponentHoleClearance, ComponentPlacementRule,
    DesignRules, FootprintPad, PadKind, PadShape, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, PadReference, Placement, Point, Size,
)
from pcbir.placement import (transformed_local_point, placement_solution_is_legal, _legal,
                             PlacementPlannerOptions, placement_rejection_reasons)
from pcbir.placement_holes import footprint_holes_clear


def court(width, height):
    w, h = Size.mm(width, height).width_nm // 2, Size.mm(width, height).height_nm // 2
    return tuple(Point(x, y) for x, y in [(-w, -h), (w, -h), (w, h), (-w, h)])


def scene(*, hole_kind=PadKind.NON_PLATED_THROUGH_HOLE, hole_position=None, land_position=None):
    hole = FootprintPad("H" if hole_kind is PadKind.THROUGH_HOLE else "", Point.mm(0, 0),
                        Size.mm("0.6", "0.6") if hole_kind is PadKind.THROUGH_HOLE else Size.mm("0.2", "0.2"),
                        kind=hole_kind, shape=PadShape.CIRCLE, drill=Size.mm("0.2", "0.2"))
    drilled = PhysicalFootprint("drilled", (hole,), Size.mm(1, 1), courtyard=court(1, 1))
    land = PhysicalFootprint("land", (FootprintPad("1", Point.mm(0, 0), Size.mm("0.2", "0.2"),
                                                shape=PadShape.RECTANGLE),), Size.mm("0.2", "0.2"),
                             courtyard=court("0.2", "0.2"))
    a = Placement("HOLDER", drilled.name, hole_position or Point.mm(10, 10))
    b = Placement("PASSIVE", land.name, land_position or Point.mm("10.399999", 10), side=BoardSide.BACK)
    return PhysicalBoard("drill", BoardOutline.rectangle(30, 30), {p.name:p for p in (drilled,land)},
                         (a,b), (), rules=DesignRules(minimum_hole_clearance_nm=200000))


def test_opposite_face_npth_clearance_exact_boundary_and_symmetric_moves():
    board = scene(); owner, pad = board.placements
    assert not footprint_holes_clear(board, pad, {owner.reference:owner})
    assert not footprint_holes_clear(board, owner, {pad.reference:pad})
    assert not placement_solution_is_legal(board,{p.reference:p for p in board.placements})
    clear = replace(pad, position=Point.mm("10.4", 10))
    assert footprint_holes_clear(board, clear, {owner.reference:owner})
    assert footprint_holes_clear(board, owner, {clear.reference:clear})
    assert any(f.code == "DRC-HOLE-CLEARANCE" for f in placement_copper_findings(board))
    fixed = replace(board, placements=(owner,clear))
    assert not any(f.code == "DRC-HOLE-CLEARANCE" for f in placement_copper_findings(fixed))
    assert placement_solution_is_legal(fixed,{p.reference:p for p in fixed.placements})


def test_cap_courtyard_cannot_cover_foreign_hole_even_when_copper_is_clear():
    board = scene(land_position=Point.mm("10.4", 10))
    fp = replace(board.footprints["land"], courtyard=court(1,1))
    board = replace(board, footprints={**board.footprints, "land":fp})
    owner,pad = board.placements
    assert not any(f.code == "DRC-HOLE-CLEARANCE" for f in placement_copper_findings(board))
    assert not footprint_holes_clear(board, pad, {owner.reference:owner})
    assert not footprint_holes_clear(board, owner, {pad.reference:pad})


def test_distant_drill_pairs_skip_narrow_geometry(monkeypatch):
    import pcbir.placement_holes as engine
    board = scene(hole_position=Point.mm(20,20))
    owner, pad = board.placements
    def unexpected(*args):
        pytest.fail("distant envelopes must not enter drill/courtyard checks")
    monkeypatch.setattr(engine, "_drills_clear_courtyard", unexpected)
    assert footprint_holes_clear(board, pad, {owner.reference:owner})
    assert footprint_holes_clear(board, owner, {pad.reference:pad})


@pytest.mark.parametrize('angle',(0,90,180,270))
def test_opposite_face_annulus_obeys_copper_spacing_even_when_drill_is_clear(angle):
    board=scene(hole_kind=PadKind.THROUGH_HOLE,land_position=Point.mm('10.5',10))
    owner,pad=board.placements
    frame=replace(owner,rotation_degrees=angle)
    pad=replace(pad,position=transformed_local_point(frame,Point.mm('.5',0)),rotation_degrees=angle)
    board=replace(board,placements=(frame,pad))
    # Drill edge-to-land clearance is0.3 mm; annulus edge-to-land is0.1 mm.
    assert not any(f.code=='DRC-HOLE-CLEARANCE' for f in placement_copper_findings(board))
    assert any(f.code=='DRC-CLEARANCE' for f in placement_copper_findings(board))
    assert not footprint_holes_clear(board,pad,{frame.reference:frame})
    assert not footprint_holes_clear(board,frame,{pad.reference:pad})
    joined=replace(board,nets=(PhysicalNet('JOIN',(PadReference(frame.reference,'H'),PadReference(pad.reference,'1'))),))
    assert footprint_holes_clear(joined,pad,{frame.reference:frame})
    # Reusing the identical geometry with a new net partition cannot reuse
    # the preceding same-net exception.
    assert not footprint_holes_clear(board,pad,{frame.reference:frame})


def test_envelope_includes_pad_outside_small_courtyard():
    board = scene(land_position=Point.mm(15,10))
    fp = board.footprints["land"]
    fp = replace(fp, pads=(replace(fp.pads[0], position=Point.mm(5,0)),))
    board = replace(board, footprints={**board.footprints, fp.name:fp})
    owner, pad = board.placements
    assert not footprint_holes_clear(board, pad, {owner.reference:owner})
    assert not footprint_holes_clear(board, owner, {pad.reference:pad})


@pytest.mark.parametrize("kind", [PadKind.NON_PLATED_THROUGH_HOLE, PadKind.THROUGH_HOLE])
def test_mutable_hole_owner_uses_accepted_held_pose_not_unaccepted_board_seed(kind):
    board = scene(hole_kind=kind, hole_position=Point.mm(20,20))
    seed,pad = board.placements
    held = replace(seed, position=Point.mm(10,10))
    assert footprint_holes_clear(board,pad,{seed.reference:seed})
    assert not footprint_holes_clear(board,pad,{held.reference:held})
    fresh = replace(held,position=Point.mm(20,20))
    assert footprint_holes_clear(board,pad,{fresh.reference:fresh})
    assert board.placements[0] == seed
    assert not _legal(pad,{held.reference:held},board,PlacementPlannerOptions())
    assert _legal(pad,{seed.reference:seed},board,PlacementPlannerOptions())


def test_pth_own_annulus_is_exempt_but_foreign_pad_and_court_are_not():
    board = scene(hole_kind=PadKind.THROUGH_HOLE)
    owner,pad=board.placements
    assert footprint_holes_clear(board,owner,{})
    assert footprint_holes_clear(board,owner,{owner.reference:owner})
    assert not footprint_holes_clear(board,pad,{owner.reference:owner})
    assert not footprint_holes_clear(board,owner,{pad.reference:pad})
    single=replace(board,placements=(owner,))
    assert placement_solution_is_legal(single,{owner.reference:owner})


def test_scoped_own_npth_rule_and_mask_never_waive_foreign_clearance():
    board=scene(); owner,pad=board.placements
    board=replace(board,component_hole_clearances=(ComponentHoleClearance(owner.reference,100000,"own pads only"),))
    assert not footprint_holes_clear(board,pad,{owner.reference:owner})
    fp=replace(board.footprints[pad.footprint],pads=tuple(replace(p,has_solder_mask=False) for p in board.footprints[pad.footprint].pads))
    no_mask=replace(board,footprints={**board.footprints,fp.name:fp})
    assert not footprint_holes_clear(no_mask,pad,{owner.reference:owner})


def test_actual_renamed_oval_usb_peg_and_0603_back_pad_163um_conflict():
    # Exact relevant locked KiCad dimensions; cropped fixture, not a complete
    # connector or consumer electrical/assembly qualification. Source .2mm rule.
    # USB_C_Receptacle_Amphenol_12401610E4-2A: local(+3.6,-4.36), drill.95×.65.
    # C_0603_1608Metric: localpad±.775, copper.9×.95, roundrect ratio.25.
    peg=FootprintPad("",Point.mm("3.6","-4.36"),Size.mm(".95",".65"),
                     kind=PadKind.NON_PLATED_THROUGH_HOLE,shape=PadShape.OVAL,drill=Size.mm(".95",".65"))
    connector=PhysicalFootprint("cropped-mechanics",(peg,),Size.mm(10.78,11.6),
                                courtyard=tuple(Point.mm(x,y) for x,y in [(-5.39,-5.87),(5.39,-5.87),(5.39,5.73),(-5.39,5.73)]))
    cap=PhysicalFootprint("0603-land",tuple(FootprintPad(str(i),Point.mm(x,0),Size.mm(".9",".95"))
                                          for i,x in [(1,"-.775"),(2,".775")]),Size.mm(1.6,.8),
                          courtyard=court(2.96,1.46))
    a=Placement("MOUNT",connector.name,Point.mm(114.77,12.5),90,BoardSide.FRONT)
    b=Placement("DECOUPLER",cap.name,Point.mm(110,10),0,BoardSide.BACK)
    board=PhysicalBoard("actual-drill-dimensions",BoardOutline.rectangle(150,30),
                        {p.name:p for p in (connector,cap)},(a,b),(),
                        rules=DesignRules(minimum_hole_clearance_nm=200000),
                        placement_rules=(ComponentPlacementRule(a.reference,fixed_position=a.position,
                                                                fixed_rotation_degrees=90,side=a.side),))
    hole=non_plated_holes(board)[0][1]
    assert transformed_local_point(a,peg.position)==Point.mm(110.41,8.9)
    land=cap.pads[0]; position=transformed_local_point(b,land.position)
    assert position==Point.mm(110.775,10)
    copper=placed_pad_shape(position,land,b)
    measured=sqrt(shape_distance_squared(hole,copper))-(hole.radius_nm+copper.radius_nm)
    assert 163000 < measured < 165000 and not shapes_clear(hole,copper,200000)
    assert not footprint_holes_clear(board,b,{a.reference:a})
    assert not footprint_holes_clear(board,a,{b.reference:b})
    assert not placement_solution_is_legal(board,{p.reference:p for p in board.placements})
    assert not _legal(b,{a.reference:a},board,PlacementPlannerOptions())
    assert not _legal(a,{b.reference:b},board,PlacementPlannerOptions())
    assert any("footprint drill" in reason for reason in placement_rejection_reasons(board,{p.reference:p for p in board.placements}))
    assert any(f.code=="DRC-HOLE-CLEARANCE" for f in placement_copper_findings(board))
    assert board.placements==(a,b) and board.tracks==board.vias==()
    clear=replace(b,position=Point.mm(110,11.5))
    corrected=replace(board,placements=(a,clear))
    assert placement_solution_is_legal(corrected,{p.reference:p for p in corrected.placements})
    assert not any(f.code=="DRC-HOLE-CLEARANCE" for f in placement_copper_findings(corrected))


def test_oval_drill_geometry_matches_drc_after_rotation_and_back_mirroring():
    board=scene();a,b=board.placements
    fp=board.footprints[a.footprint]
    pad=replace(fp.pads[0],position=Point.mm(1,0),size=Size.mm(".8",".3"),drill=Size.mm(".8",".3"),shape=PadShape.OVAL)
    fp=replace(fp,pads=(pad,))
    a=replace(a,rotation_degrees=90,side=BoardSide.BACK)
    board=replace(board,footprints={**board.footprints,fp.name:fp},placements=(a,b))
    from pcbir.placement_holes import _geometry
    assert tuple(hole.shape for hole in _geometry(board,a).holes)==(non_plated_holes(board)[0][1],)


def test_new_footprint_snapshot_changes_geometry_cache_and_keeps_previous_snapshot():
    board=scene();a,b=board.placements
    assert not footprint_holes_clear(board,b,{a.reference:a})
    fp=board.footprints[a.footprint]
    moved=replace(fp,pads=(replace(fp.pads[0],position=Point.mm(5,0)),))
    fresh=replace(board,footprints={**board.footprints,fp.name:moved})
    assert footprint_holes_clear(fresh,b,{a.reference:a})
    assert not footprint_holes_clear(board,b,{a.reference:a})


def test_own_npth_pad_rule_matches_drc_without_relaxing_other_components():
    board=scene();a,b=board.placements
    fp=board.footprints[a.footprint]
    fp=replace(fp,pads=(*fp.pads,FootprintPad("1",Point.mm(".35",0),Size.mm(".2",".2"),shape=PadShape.RECTANGLE)))
    board=replace(board,footprints={**board.footprints,fp.name:fp},placements=(a,))
    assert not footprint_holes_clear(board,a,{})
    assert not placement_solution_is_legal(board,{a.reference:a})
    assert any(f.code=="DRC-HOLE-CLEARANCE" for f in placement_copper_findings(board))
    scoped=replace(board,component_hole_clearances=(ComponentHoleClearance(a.reference,150000,"own source land geometry"),))
    assert footprint_holes_clear(scoped,a,{})
    assert placement_solution_is_legal(scoped,{a.reference:a})
    assert not any(f.code=="DRC-HOLE-CLEARANCE" for f in placement_copper_findings(scoped))
    strict=replace(scoped,component_hole_clearances=(ComponentHoleClearance(a.reference,150001,"stricter source geometry"),))
    assert not footprint_holes_clear(strict,a,{})


def test_concave_courtyard_notch_has_no_phantom_body_but_material_is_protected():
    board=scene();a,b=board.placements
    shape=tuple(Point.mm(x,y) for x,y in [(-1,-1),(1,-1),(1,0),(0,0),(0,1),(-1,1)])
    fp=replace(board.footprints[b.footprint],pads=(),courtyard=shape)
    b=replace(b,position=Point.mm(10,10))
    a=replace(a,position=Point.mm("9.5","10.5"))
    board=replace(board,footprints={**board.footprints,fp.name:fp},placements=(a,b))
    assert footprint_holes_clear(board,b,{a.reference:a})
    blocked=replace(a,position=Point.mm("10.5","10.5"))
    assert not footprint_holes_clear(board,b,{blocked.reference:blocked})


def test_mismatched_held_identity_is_not_a_self_exemption():
    board=scene();a,b=board.placements
    assert not footprint_holes_clear(board,b,{b.reference:a})


def test_same_net_label_never_permits_foreign_pth_body_or_land_penetration():
    board=scene(hole_kind=PadKind.THROUGH_HOLE);a,b=board.placements
    shared=PhysicalNet("shared",(PadReference(a.reference,"H"),PadReference(b.reference,"1")))
    board=replace(board,nets=(shared,))
    assert not footprint_holes_clear(board,b,{a.reference:a})
    assert not footprint_holes_clear(board,a,{b.reference:b})
