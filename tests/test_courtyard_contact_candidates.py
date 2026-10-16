from dataclasses import replace
from decimal import Decimal
import sys
import pytest
from pcbir import BoardOutline,PhysicalBoard,PhysicalFootprint,Placement,Point,Size
from pcbir.physical import BoardSide,ComponentPlacementRule,PlacementRegion,PlacementTarget,RigidPlacementCluster,RigidPlacementMember
from pcbir.clusters import footprint_geometry_digest
from pcbir.placement import PlacementPlannerOptions,_candidate_positions,_allowed_orientations,_legal,_courtyard_contact_candidates,_pack_local_components,placement_solution_is_legal


def scene(prefix="arbitrary",dx=0,dy=0,rotated=False):
    fp=PhysicalFootprint("land",(),Size.mm(1,.5),courtyard=tuple(Point.mm(x,y) for x,y in
        ((-.91,-.46),(.91,-.46),(.91,.46),(-.91,.46))))
    def point(x,y):
        q=Point.mm(x,y)
        return Point((q.y_nm if rotated else q.x_nm)+dx,(q.x_nm if rotated else q.y_nm)+dy)
    angle=90 if rotated else 0
    a=Placement(prefix+"-left",fp.name,point("11.1","10.6"),angle,BoardSide.BACK)
    c=replace(a,reference=prefix+"-right",position=point("15.74","10.6"))
    target=replace(a,reference=prefix+"-pending",position=point(13,"10.6"))
    region=PlacementRegion("channel",BoardOutline.rectangle(1.2 if rotated else 6,
        6 if rotated else 1.2,origin=point(10,10)),BoardSide.BACK)
    b=PhysicalBoard("ContactChannel",BoardOutline.rectangle(30,30),{fp.name:fp},(a,c,target),(),regions=(region,),
        placement_rules=(ComponentPlacementRule(target.reference,region="channel",side=BoardSide.BACK),
            *(ComponentPlacementRule(q.reference,fixed_position=q.position,fixed_rotation_degrees=angle,side=BoardSide.BACK) for q in (a,c))))
    return b,target,{q.reference:q for q in (a,c)}


@pytest.mark.parametrize("prefix,dx,dy,rotated",(("renamed",0,0,False),("other",370000,210000,False),("turned",0,0,True)))
def test_actual_geometry_pitch_solves_channel_missed_by_grid_and_point_one_offsets(prefix,dx,dy,rotated):
    b,target,base=scene(prefix,dx,dy,rotated)
    o=PlacementPlannerOptions()
    ordinary=tuple(replace(target,position=q,rotation_degrees=a) for q in _candidate_positions(b,target,o) for a in _allowed_orientations(b,target.reference))
    fine=tuple(replace(p,position=Point(p.position.x_nm+x,p.position.y_nm+y)) for p in ordinary
        for x in (-100000,0,100000) for y in (-100000,0,100000))
    assert not any(_legal(q,base,b,o) for q in (*ordinary,*fine))
    before=dict(base)
    result=_pack_local_components(b,[target.reference],base,{q.reference:q for q in b.placements},o,allow_general=True)
    assert result is not None and placement_solution_is_legal(b,result,o)
    assert base==before and all(result[r]==q for r,q in before.items())
    selected=result[target.reference]
    assert (selected.position.y_nm if rotated else selected.position.x_nm)==13_420_000+(dy if rotated else dx)
    assert selected.side is BoardSide.BACK


def test_nonrectangular_domain_is_gated_by_actual_polygon_not_bounding_box():
    b,target,base=scene()
    triangle=BoardOutline((Point.mm(10,10),Point.mm(16,10),Point.mm(10,11.2)))
    b=replace(b,regions=(PlacementRegion("channel",triangle,BoardSide.BACK),))
    o=PlacementPlannerOptions()
    proposals=_courtyard_contact_candidates(b,target,tuple(base.values()),o,excluded=(),limit=64)
    assert proposals and not any(_legal(q,base,b,o) for q in proposals)
    assert _pack_local_components(b,[target.reference],base,{q.reference:q for q in b.placements},o,allow_general=True) is None


def test_wrong_side_or_unknown_footprint_proposal_anchors_have_no_authority():
    b,target,base=scene()
    o=PlacementPlannerOptions()
    bad=tuple(replace(q,side=BoardSide.FRONT) for q in base.values())
    unknown=tuple(replace(q,footprint="missing") for q in base.values())
    absent=tuple(replace(q,reference="absent-source-reference") for q in base.values())
    empty=_courtyard_contact_candidates(b,target,(),o,excluded=(),limit=64)
    assert _courtyard_contact_candidates(b,target,bad,o,excluded=(),limit=64)==empty
    assert _courtyard_contact_candidates(b,target,unknown,o,excluded=(),limit=64)==empty
    assert _courtyard_contact_candidates(b,target,absent,o,excluded=(),limit=64)==empty


def test_fixed_and_rigid_targets_do_not_gain_contact_motion():
    b,target,base=scene()
    o=PlacementPlannerOptions()
    fixed_rule=replace(b.placement_rules[0],fixed_position=target.position,fixed_rotation_degrees=0)
    fixed=replace(b,placement_rules=(fixed_rule,*b.placement_rules[1:]))
    assert _courtyard_contact_candidates(fixed,target,tuple(base.values()),o,excluded=(),limit=64)==()
    # Audited rigid membership is checked independently of current geometry.
    front=replace(b,placements=tuple(replace(q,side=BoardSide.FRONT) for q in b.placements),
        placement_rules=tuple(replace(r,side=BoardSide.FRONT) for r in b.placement_rules),
        regions=(replace(b.regions[0],side=BoardSide.FRONT),))
    members=tuple(RigidPlacementMember(q.reference,q.footprint,footprint_geometry_digest(front.footprints[q.footprint]),Point(0,0))
                  for q in (front.placements[0],front.placements[-1]))
    cluster=RigidPlacementCluster("audited",PlacementTarget(target.reference),members,"synthetic geometry")
    rigid=replace(front,rigid_clusters=(cluster,))
    assert _courtyard_contact_candidates(rigid,front.placements[-1],front.placements[:2],o,excluded=(),limit=64)==()


@pytest.mark.parametrize("limit",(0,1,3,8))
def test_contact_candidate_storage_and_raw_count_are_bounded(limit):
    b,target,base=scene()
    probes=_courtyard_contact_candidates(b,target,tuple(base.values()),PlacementPlannerOptions(),excluded=(),limit=limit)
    assert len(probes)<=limit and len(set(probes))==len(probes)


def test_initial_member_uses_only_original_raw_allowance():
    import pcbir.placement as module
    b,target,base=scene()
    o=PlacementPlannerOptions(legalization_candidates=1)
    captured=[]
    def local(frame,event,arg):
        if event=="return":captured.append(dict(frame.f_locals.get("region_probe_remaining",{})))
        return local
    def trace(frame,event,arg):
        if event=="call" and frame.f_code.co_filename==module.__file__ and frame.f_code.co_name=="_pack_local_components":
            frame.f_trace_lines=False;return local
    sys.settrace(trace)
    try:result=module._pack_local_components(b,[target.reference],base,{q.reference:q for q in b.placements},o,allow_general=True)
    finally:sys.settrace(None)
    assert result is not None
    assert all(0<=v<=o.legalization_candidates*64 for d in captured for v in d.values())


def test_added_sites_keep_original_legal_ordinary_prefix_verbatim():
    import pcbir.placement as module
    b,target,base=scene()
    b=replace(b,regions=(PlacementRegion("channel",BoardOutline.rectangle(6,6,origin=Point.mm(10,10)),BoardSide.BACK),))
    o=PlacementPlannerOptions()
    ordinary=tuple(replace(target,position=q,rotation_degrees=a) for q in _candidate_positions(b,target,o) for a in _allowed_orientations(b,target.reference))
    expected=tuple(q for q in ordinary if _legal(q,{},b,o))
    captured=[]
    def local(frame,event,arg):
        if event=="return":captured.append(frame.f_locals.get("domains",{}))
        return local
    def trace(frame,event,arg):
        if event=="call" and frame.f_code.co_filename==module.__file__ and frame.f_code.co_name=="_pack_local_components":
            frame.f_trace_lines=False;return local
    sys.settrace(trace)
    try:result=module._pack_local_components(b,[target.reference],{}, {q.reference:q for q in b.placements},o,allow_general=True)
    finally:sys.settrace(None)
    assert result is not None and captured[0][target.reference][:len(expected)]==expected


def test_contacts_receive_only_four_remaining_raw_slots_after_sixty_ordinary_probes(monkeypatch):
    import pcbir.placement as module
    b,target,base=scene()
    b=replace(b,placement_rules=(replace(b.placement_rules[0],allowed_orientations=(0,)),*b.placement_rules[1:]))
    o=PlacementPlannerOptions(legalization_candidates=1)
    # Duplicate ordinary proposals still consume raw probe allowance.
    monkeypatch.setattr(module,"_candidate_positions",lambda *a,**k:(target.position,)*60)
    real=module._courtyard_contact_candidates
    captured=[]
    def observe(*a,**k):
        result=real(*a,**k);captured.append((k["limit"],len(result)));return result
    monkeypatch.setattr(module,"_courtyard_contact_candidates",observe)
    before=dict(base)
    result=module._pack_local_components(b,[target.reference],base,{q.reference:q for q in b.placements},o,allow_general=True)
    assert captured==[(4,4)] and result is None and base==before


def test_invalid_or_immutable_removed_held_proposals_are_not_anchor_authority(monkeypatch):
    import pcbir.placement as module
    b,target,base=scene()
    good=replace(target,position=Point.mm("13.42","10.6"))
    bad=(replace(good,reference="absent"),replace(good,footprint="absent"),
         replace(good,side=BoardSide.FRONT),replace(good,rotation_degrees=Decimal(45)),
         *base.values())
    captured=[]
    real=module._courtyard_contact_candidates
    def observe(board,target,anchors,options,**kwargs):
        captured.append(anchors);return real(board,target,anchors,options,**kwargs)
    monkeypatch.setattr(module,"_courtyard_contact_candidates",observe)
    result=module._pack_local_components(b,[target.reference],base,{q.reference:q for q in b.placements},
        PlacementPlannerOptions(),allow_general=True,_contact_anchors=(*bad,good))
    assert result is not None
    expected=(*base.values(),target,good)
    assert captured[0]==expected
    assert all(result[r]==q for r,q in base.items())


def test_nonregion_domain_has_no_contact_candidate_scope():
    b,target,base=scene()
    b=replace(b,placement_rules=(replace(b.placement_rules[0],region=None),*b.placement_rules[1:]))
    assert _courtyard_contact_candidates(b,target,tuple(base.values()),PlacementPlannerOptions(),excluded=(),limit=64)==()
