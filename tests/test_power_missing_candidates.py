"""A zero-candidate domain is eligible for bounded same-source regrowth."""
from dataclasses import replace
import pytest
from pcbir import CopperLayer,PhysicalNet,PhysicalPowerDomain,PadReference,Point
from pcbir.power_geometry import PowerAreaOptions,plan_power_areas,apply_power_area_plan
from test_power_geometry import board


def fixture():
    b=board();pads=(PadReference('S2','1'),PadReference('L2','1'))
    b=replace(b,placements=(*b.placements,
        replace(b.placements[0],reference='S2',position=Point.mm(3,8)),
        replace(b.placements[1],reference='L2',position=Point.mm(15,8))),
        nets=(*b.nets,PhysicalNet('OTHER',pads)),
        power_domains=(*b.power_domains,PhysicalPowerDomain('OTHER',pads,pads[:1])))
    return b,PowerAreaOptions(candidates_per_domain=1,allowed_layers=(CopperLayer.FRONT,),maximum_conflict_trials=2)


@pytest.mark.parametrize('recover',[True,False])
def test_missing_initial_domain_can_regrow_but_never_adopts_a_partial_prefix(monkeypatch,recover):
    import pcbir.power_geometry as geometry
    b,options=fixture();original=geometry._tree;calls=[]
    def missing(source,obligation,*args,**kwargs):
        held=any(z.net=='RAIL' and z.id.startswith('power-area:') for z in source.zones)
        if obligation.net=='OTHER':
            calls.append(held)
            if not held or not recover:return None,'no_tree_within_budget'
        return original(source,obligation,*args,**kwargs)
    monkeypatch.setattr(geometry,'_tree',missing)
    plan=plan_power_areas(b,options)
    assert calls==[False,True]
    assert plan.conflict_contexts and len(plan.conflict_trials)==1
    assert plan.source_board is b and plan.allocation_states<=options.allocation_states
    routed,status=apply_power_area_plan(b,plan)
    if recover:
        assert len(plan.selected)==2 and status=='contact_routed_native_pending'
        assert all(c.allocation.snapshot==plan.snapshot for c in plan.candidates)
        assert len(routed.tracks)==sum(len(c.tracks) for c in plan.selected)
    else:
        assert not plan.selected and routed is b
    assert b.tracks==b.vias==b.zones==()


def test_unknown_source_never_retries_as_a_known_domain(monkeypatch):
    b,options=fixture()
    b=replace(b,power_domains=(b.power_domains[0],replace(b.power_domains[1],sources=())))
    plan=plan_power_areas(b,options)
    assert not plan.selected and not plan.conflict_trials
    assert apply_power_area_plan(b,plan)[0] is b


def test_missing_domains_share_trials_and_equivalent_obstacles_are_not_repeated(monkeypatch):
    import pcbir.power_geometry as geometry
    b,options=fixture();pads=(PadReference('S3','1'),PadReference('L3','1'))
    b=replace(b,placements=(*b.placements,
        replace(b.placements[0],reference='S3',position=Point.mm(3,2)),
        replace(b.placements[1],reference='L3',position=Point.mm(15,2))),
        nets=(*b.nets,PhysicalNet('THIRD',pads)),
        power_domains=(*b.power_domains,PhysicalPowerDomain('THIRD',pads,pads[:1])))
    original=geometry._tree
    def missing(source,obligation,*args,**kwargs):
        if obligation.net!='RAIL':return None,'no_tree_within_budget'
        # Two proposal identities deliberately have the same copper obstacle.
        kwargs.update(reverse=False, detour=0)
        return original(source,obligation,*args,**kwargs)
    monkeypatch.setattr(geometry,'_tree',missing)
    plan=plan_power_areas(b,replace(options,candidates_per_domain=2,maximum_conflict_trials=8))
    assert [net for net,_,_ in plan.conflict_trials]==['OTHER','THIRD']
    assert not plan.selected and apply_power_area_plan(b,plan)[0] is b


def test_zero_addition_candidates_are_not_regeneration_obstacles(monkeypatch):
    import pcbir.power_geometry as geometry
    from pcbir import TrackSegment
    b,options=fixture()
    b=replace(b,tracks=(TrackSegment('RAIL',Point.mm(3,5),Point.mm(15,5),250000,CopperLayer.FRONT),))
    original=geometry._tree
    def missing(source,obligation,*args,**kwargs):
        if obligation.net=='OTHER':return None,'no_tree_within_budget'
        return original(source,obligation,*args,**kwargs)
    monkeypatch.setattr(geometry,'_tree',missing)
    plan=plan_power_areas(b,options)
    assert len(plan.candidates)==1 and not plan.candidates[0].tracks
    assert not plan.conflict_trials and apply_power_area_plan(b,plan)[0] is b


@pytest.mark.parametrize('unrelated_prefix', [False, True])
def test_unavoidable_conflict_tries_both_repair_directions_within_two_slots(monkeypatch, unrelated_prefix):
    import pcbir.power_geometry as geometry
    b,options=fixture()
    b=replace(b,placements=tuple(replace(p,position=Point.mm(8,4)) if p.reference=='S2' else
        replace(p,position=Point.mm(12,6)) if p.reference=='L2' else p for p in b.placements))
    if unrelated_prefix:
        pads=(PadReference('S3','1'),PadReference('L3','1'))
        b=replace(b,placements=(*b.placements,
            replace(b.placements[0],reference='S3',position=Point.mm(3,2)),
            replace(b.placements[1],reference='L3',position=Point.mm(15,2))),
            nets=(*b.nets,PhysicalNet('AAA',pads)),
            power_domains=(*b.power_domains,PhysicalPowerDomain('AAA',pads,pads[:1])))
    original=geometry._tree
    def reject_regrowth(source,obligation,*args,**kwargs):
        if any(z.id.startswith('power-area:') and z.net!=obligation.net for z in source.zones):
            return None,'no_tree_within_budget'
        kwargs.update(reverse=False,detour=0)
        return original(source,obligation,*args,**kwargs)
    monkeypatch.setattr(geometry,'_tree',reject_regrowth)
    plan=plan_power_areas(b,replace(options,candidates_per_domain=2,maximum_conflict_trials=2))
    assert len(plan.conflict_trials)==2
    assert {net for net,_,_ in plan.conflict_trials}=={'RAIL','OTHER'}
    assert plan.allocation_states<=options.allocation_states and not plan.selected
    assert apply_power_area_plan(b,plan)[0] is b
