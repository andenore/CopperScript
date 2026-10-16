from dataclasses import replace
import pytest
from pcbir import BoardOutline,PhysicalBoard,PhysicalFootprint,Placement,Point,Size
from pcbir.physical import BoardSide,ComponentPlacementRule,PlacementRegion
from pcbir.placement import PlacementPlannerOptions,placement_solution_is_legal


def alias(prefix):
    fp=PhysicalFootprint("land",(),Size.mm(".7",".7"))
    poses=tuple(Placement(prefix+suffix,fp.name,Point.mm("11.5",11),side=BoardSide.BACK)
                for suffix in ("-a","-b"))
    region=PlacementRegion("strip",BoardOutline.rectangle(2,.9,origin=Point.mm("10.5","10.5")),BoardSide.BACK)
    b=PhysicalBoard("NamedAlias",BoardOutline.rectangle(25,25),{fp.name:fp},poses,(),regions=(region,),
        placement_rules=tuple(ComponentPlacementRule(p.reference,region="strip",side=BoardSide.BACK,
                                                    allowed_orientations=(0,)) for p in poses))
    options=PlacementPlannerOptions(legalization_candidates=2)
    return b,options,{p.reference:p for p in poses}


def observed_pack(monkeypatch,mutation=None):
    import pcbir.placement as module
    real=module._pack_local_components
    # Test residual continuation independently of initial contact-site discovery.
    monkeypatch.setattr(module,"_courtyard_contact_candidates",lambda *a,**k: ())
    events=[]
    def observe(*args,**kwargs):
        state=kwargs.get("_region_state")
        event=None
        if state is not None:
            if mutation:mutation(state)
            event={"before_search":state["search_remaining"],"before_heuristic":state["heuristic_remaining"],
                   "before_domain":dict(state["domain_remaining"])}
        result=real(*args,**kwargs)
        if event is not None:
            event.update(after_search=state["search_remaining"],after_heuristic=state["heuristic_remaining"],
                         after_domain=dict(state["domain_remaining"]))
            events.append(event)
        return result
    monkeypatch.setattr(module,"_pack_local_components",observe)
    return observe,events


@pytest.mark.parametrize("prefix",("renamed","arbitrary"))
def test_named_region_alias_continues_with_only_original_residual_resources(prefix,monkeypatch):
    b,o,targets=alias(prefix)
    pack,events=observed_pack(monkeypatch)
    result=pack(b,list(targets),{},targets,o,allow_general=True)
    assert result is not None and placement_solution_is_legal(b,result,o)
    assert any(p.position.x_nm%1_000_000 for p in result.values())
    assert all(p.side is BoardSide.BACK and p.rotation_degrees==0 for p in result.values())
    assert len(events)==1
    event=events[0]
    assert 0<=event["after_search"]<=event["before_search"]<o.legalization_candidates*o.exact_repair_candidates*64
    assert 0<=event["after_heuristic"]<=event["before_heuristic"]
    assert all(0<=event["after_domain"][r]<=event["before_domain"][r]==o.legalization_candidates*64-2 for r in targets)


def test_zero_search_residual_does_not_continue(monkeypatch):
    b,o,targets=alias("zero")
    pack,events=observed_pack(monkeypatch,lambda s:s.update(search_remaining=0))
    assert pack(b,list(targets),{},targets,o,allow_general=True) is None
    assert events[0]["after_domain"]==events[0]["before_domain"]


@pytest.mark.parametrize("quota",(0,1))
def test_member_domain_probe_residual_is_not_reset(quota,monkeypatch):
    b,o,targets=alias("domain")
    def restrict(state):state["domain_remaining"]={r:quota for r in state["domain_remaining"]}
    pack,events=observed_pack(monkeypatch,restrict)
    assert pack(b,list(targets),{},targets,o,allow_general=True) is None
    assert all(events[0]["before_domain"][r]-events[0]["after_domain"][r]<=quota for r in targets)


def test_zero_heuristic_residual_can_use_remaining_search_without_reset(monkeypatch):
    b,o,targets=alias("heuristic")
    pack,events=observed_pack(monkeypatch,lambda s:s.update(heuristic_remaining=0))
    result=pack(b,list(targets),{},targets,o,allow_general=True)
    assert result is not None and placement_solution_is_legal(b,result,o)
    assert events[0]["before_heuristic"]==events[0]["after_heuristic"]==0


@pytest.mark.parametrize("changed",("scene","contact_tuple"))
def test_stale_scene_or_held_base_cannot_reuse_cached_domains(changed,monkeypatch):
    b,o,targets=alias("stale")
    def change(state):
        if changed=="scene":state["board"]=replace(b,name="other")
        else:state["contact_anchors"]=(next(iter(targets.values())),)
    pack,events=observed_pack(monkeypatch,change)
    assert pack(b,list(targets),{},targets,o,allow_general=True) is None
    assert events[0]["after_domain"]==events[0]["before_domain"]


def test_fixed_held_geometry_is_preserved_through_continuation(monkeypatch):
    b,o,targets=alias("fixed-base")
    fixed=Placement("fixed-land","land",Point.mm(20,20),side=BoardSide.BACK)
    b=replace(b,placements=(*b.placements,fixed),placement_rules=(*b.placement_rules,
        ComponentPlacementRule(fixed.reference,fixed_position=fixed.position,fixed_rotation_degrees=0,side=BoardSide.BACK)))
    pack,events=observed_pack(monkeypatch)
    all_targets={**targets,fixed.reference:fixed}
    result=pack(b,list(targets),{fixed.reference:fixed},all_targets,o,allow_general=True)
    assert result is not None and result[fixed.reference]==fixed
    assert placement_solution_is_legal(b,result,o)


def test_existing_pad_max_distance_fine_dispatch_keeps_its_legacy_path(monkeypatch):
    import pcbir.placement as module
    from pcbir import FootprintPad
    from pcbir.physical import PlacementTarget,RelativePlacementKind,RelativePlacementRule
    b,o,targets=alias("legacy")
    fp=replace(b.footprints["land"],pads=(FootprintPad("1",Point(0,0),Size.mm(".2",".2")),))
    refs=list(targets)
    rule=RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
        tuple(PlacementTarget(r,"1") for r in refs),distance_nm=1_000_000_000)
    b=replace(b,footprints={fp.name:fp},relative_rules=(rule,))
    # Isolate fine dispatch from the independent relative-candidate generator.
    monkeypatch.setattr(module,"_relative_candidates",lambda *a,**k: ())
    real=module._pack_local_components
    calls=[]
    def observe(*a,**k):
        calls.append((k.get("fine",False),k.get("_region_state")))
        return real(*a,**k)
    monkeypatch.setattr(module,"_pack_local_components",observe)
    result=observe(b,refs,{},targets,o,allow_general=True)
    assert result is not None and placement_solution_is_legal(b,result,o)
    assert calls==[(False,None),(True,None)]
