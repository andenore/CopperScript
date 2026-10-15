"""Source geometry, anchored requests and shared probe-slot ownership."""
from dataclasses import replace
from itertools import islice
from types import SimpleNamespace

import pytest

import pcbir.package_access as access
import pcbir.package_placement_repair as repair
from pcbir import (BoardOutline, ComponentPlacementRule, FootprintPad, PadReference,
                   PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
                   PlacementPlannerOptions, Point, Size, GlobalRouterOptions)
from pcbir.physical import BoardSide, DecouplingLink, PlacementRegion
from pcbir.fanout import FanoutOptions, FanoutResult
from pcbir.placement import placement_solution_is_legal
from pcbir.routing import _placement_fingerprint, GlobalRoutingStatus
from pcbir.critical import CriticalRoutingStatus


def scene(renamed=False):
    pkg, cap, neighbor = ('DEVICE_41','SUPPORT_99','SUPPORT_12') if renamed else ('PKG','Z_CAP','A_NEIGHBOR')
    package = PhysicalFootprint('two-pin', (
        FootprintPad('1', Point.mm(-1,0), Size.mm('.3','.3')),
        FootprintPad('2', Point(0,0), Size.mm('.3','.3'))), Size.mm(2,2))
    capacitor = PhysicalFootprint('two-land', (
        FootprintPad('1',Point.mm('-.25',0),Size.mm('.3','.3')),
        FootprintPad('2',Point.mm('.25',0),Size.mm('.3','.3'))), Size.mm(1,'.5'))
    terminal = PhysicalFootprint('terminal',(FootprintPad('1',Point(0,0),Size.mm('.3','.3')),),Size.mm('.5','.5'))
    positions=(Placement(pkg,package.name,Point.mm(10,10)),
        Placement('END',terminal.name,Point.mm(17,10)),
        Placement(cap,capacitor.name,Point.mm(10,10),side=BoardSide.BACK),
        Placement(neighbor,capacitor.name,Point.mm('11.75',10),side=BoardSide.BACK))
    regions=tuple(PlacementRegion(name,BoardOutline(tuple(Point.mm(x,y) for x,y in points)),side=BoardSide.BACK)
        for name,points in [('cap',((8,8),(12,8),(12,12),(8,12))),('neighbor',((11,8),(14,8),(14,12),(11,12)))])
    board=PhysicalBoard('coupled',BoardOutline.rectangle(20,20),
        {p.name:p for p in (package,capacitor,terminal)},positions,
        (PhysicalNet('RAIL',(PadReference(pkg,'1'),PadReference(cap,'1'),PadReference(neighbor,'1'))),
         PhysicalNet('GND',(PadReference(cap,'2'),PadReference(neighbor,'2'))),
         PhysicalNet('IO',(PadReference(pkg,'2'),PadReference('END','1')))),regions=regions,
        placement_rules=(ComponentPlacementRule(pkg,fixed_position=positions[0].position,allowed_orientations=(0,)),
            ComponentPlacementRule('END',fixed_position=positions[1].position,allowed_orientations=(0,)),
            ComponentPlacementRule(cap,region='cap',side=BoardSide.BACK,allowed_orientations=(0,)),
            ComponentPlacementRule(neighbor,region='neighbor',side=BoardSide.BACK,allowed_orientations=(0,))),
        decoupling_links=tuple(DecouplingLink(PadReference(ref,'1'),PadReference(pkg,'1'),PadReference(ref,'2'),'RAIL','GND') for ref in (cap,neighbor)))
    baseline=SimpleNamespace(source=board,pending_pads=(PadReference(pkg,'2'),),failed_critical_nets=frozenset(),pattern_trials=())
    options=PlacementPlannerOptions(grid_step_nm=250000,candidate_count=1)
    original={p.reference:p for p in board.placements}
    requested=replace(original[cap],position=Point.mm('10.5',10))
    assert placement_solution_is_legal(board,original,options)
    assert not placement_solution_is_legal(board,{**original,cap:requested},options)
    return baseline,options,cap,neighbor,requested


def state():return repair.CoupledPlacementState(available_slots=4,next_probe_index=1)


@pytest.mark.parametrize('renamed',(False,True))
def test_source_projection_prepass_reaches_coupled_owner_before_legal_prefix(renamed):
    baseline,options,cap,neighbor,requested=scene(renamed)
    before=baseline.source
    normal=next(access.package_placement_trials(baseline,options,500000))
    assert normal[0]==neighbor
    token=state()
    owner,trial=next(access.package_placement_trials(baseline,options,500000,_coupled_state=token))
    assert owner==cap and baseline.source==before
    poses={p.reference:p for p in trial.placements};old={p.reference:p for p in before.placements}
    assert poses[cap]==requested and poses[neighbor]!=old[neighbor]
    assert all(poses[ref]==old[ref] for ref in old if ref not in {cap,neighbor})
    assert placement_solution_is_legal(before,poses,options)
    assert replace(trial,placements=before.placements)==before
    attempt=token.attempts[0]
    assert attempt.claimed and attempt.borrowed_probe_index==1 and attempt.outcome=='geometry_candidate'
    assert attempt.source_shape_pairs<=2*28*28 and attempt.source_shape_predicates<=4*28*28
    assert attempt.evidence and all(e.capacitor_side=='back' and e.failed_side=='front' for e in attempt.evidence)
    assert any(e.copper_projection_overlap for e in attempt.evidence)
    work=dict(attempt.pack_work)
    assert work['search_operations']<=172032 and work['heuristic_checks']<=57344
    assert all(n<=6144 for n in work['domain_probes'].values())
    assert all(n<=384 for n in work['retained_domains'].values())


@pytest.mark.parametrize('defect',('undo_requested','mutate_value','move_fixed'))
def test_adversarial_pack_cannot_undo_request_or_mutate_source(monkeypatch,defect):
    baseline,options,cap,neighbor,requested=scene()
    original={p.reference:p for p in baseline.source.placements}
    def corrupt(board,refs,base,targets,settings,**kwargs):
        out={**base,neighbor:replace(original[neighbor],position=Point.mm('12.5',10))}
        if defect=='undo_requested':out[cap]=original[cap]
        if defect=='mutate_value':out[neighbor]=replace(out[neighbor],value='different')
        if defect=='move_fixed':out['END']=replace(out['END'],position=Point.mm(17,12))
        return out
    monkeypatch.setattr(repair,'_pack_local_components',corrupt)
    token=state();result=token.prepass(baseline,[cap,neighbor],{cap:[requested]},options)
    assert result is None and token.failed_slots==1 and token.claimed
    assert token.attempts[0].outcome=='complete_source_gate_failed'
    assert {p.reference:p for p in baseline.source.placements}==original


def test_source_invalid_region_request_is_not_fixed_by_displacing_neighbors(monkeypatch):
    baseline,options,cap,neighbor,requested=scene()
    bad=replace(requested,position=Point.mm(17,15))
    monkeypatch.setattr(repair,'_pack_local_components',lambda *a,**k:pytest.fail('invalid source request must not pack'))
    token=state()
    assert token.prepass(baseline,[cap,neighbor],{cap:[bad]},options) is None
    assert token.claimed and token.failed_slots==1
    assert token.attempts[0].outcome=='no_suitable_blocked_request' and token.attempts[0].requested_candidates_considered==1
    assert not token.attempts[0].pack_work


def test_protected_neighbor_cannot_be_released(monkeypatch):
    baseline,options,cap,neighbor,requested=scene()
    options=replace(options,fixed_references={neighbor})
    monkeypatch.setattr(repair,'_pack_local_components',lambda *a,**k:pytest.fail('session lock must remain held'))
    token=state()
    assert token.prepass(baseline,[cap,neighbor],{cap:[requested]},options) is None
    assert token.failed_slots==1 and token.attempts[0].pack_work==()


def fake_probe(baseline,reference,trial,*args):
    index=args[-1]
    return access.PackagePlacementProbe(index,reference,_placement_fingerprint(trial),(),(),(),(),0,(),(index,))


def test_failed_claim_spends_one_of_four_slots_and_never_resets_later_round(monkeypatch):
    baseline,options,cap,neighbor,requested=scene()
    calls=[]
    monkeypatch.setattr(repair,'_pack_local_components',lambda *a,**k:calls.append(1))
    monkeypatch.setattr(access,'_placement_access_probe',fake_probe)
    token=state();records=[];settings=access.PackageAccessOptions(maximum_placement_probes=4)
    generated=access._ranked_placement_trials(baseline,FanoutOptions(),settings,options,None,set(),records,None,_coupled_state=token)
    list(islice(generated,3))
    assert len(calls)==1 and token.failed_slots==1 and [p.index for p in records]==[2,3,4]
    assert token.attempts[0].borrowed_probe_index==1
    later=access._ranked_placement_trials(baseline,FanoutOptions(),settings,options,None,set(),records,None,_coupled_state=token)
    next(later)
    assert len(calls)==1 and len(records)==3 and len(token.attempts)==1


def test_success_reserves_same_probe_slot_and_zero_slot_has_no_pack(monkeypatch):
    baseline,options,*_=scene();token=state();records=[]
    monkeypatch.setattr(access,'_placement_access_probe',fake_probe)
    settings=access.PackageAccessOptions(maximum_placement_probes=4)
    generated=access._ranked_placement_trials(baseline,FanoutOptions(),settings,options,None,set(),records,None,_coupled_state=token)
    list(islice(generated,4))
    assert len(records)==4 and [p.index for p in records]==[1,2,3,4] and token.failed_slots==0
    assert token.attempts[0].borrowed_probe_index==records[0].index==1
    empty=state();empty.available_slots=0
    assert empty.prepass(baseline,[],{},options) is None and not empty.considered and not empty.claimed


def test_full_preflight_rejection_rolls_back_entire_incumbent(monkeypatch):
    simple,options,*_=scene();board=simple.source
    guides=SimpleNamespace(placement_fingerprint=_placement_fingerprint(board),status=GlobalRoutingStatus.SUCCESS)
    fan=FanoutResult(board,{},tuple(simple.pending_pads),0,0)
    critical=SimpleNamespace(board=board,status=CriticalRoutingStatus.FAILED)
    baseline=access.PackageAccessResult(board,guides,fan,critical,None,frozenset(),0)
    monkeypatch.setattr(access,'_placement_access_probe',fake_probe)
    monkeypatch.setattr(access,'route_global',lambda *a,**k:guides)
    def reject(trial,*args,**kwargs):
        return replace(baseline,source=trial,hard_findings=1)
    monkeypatch.setattr(access,'preflight_package_access',reject)
    result=access.improve_package_access(baseline,FanoutOptions(),placement_options=options,
        options=access.PackageAccessOptions(maximum_trials=1,maximum_placement_probes=4))
    assert result.source is baseline.source and result.fanout is baseline.fanout and result.critical is baseline.critical
    assert result.global_route is baseline.global_route and result.accepted_moves==0
    assert result.trials[0].outcome=='hard_drc'
    assert result.coupled_placement_attempts[0].outcome=='geometry_candidate'


def test_last_failed_geometry_slot_cannot_start_a_fifth_probe(monkeypatch):
    baseline,options,*_=scene();token=state();records=[]
    monkeypatch.setattr(repair,'_pack_local_components',lambda *a,**k:None)
    monkeypatch.setattr(access,'_placement_access_probe',lambda *a,**k:pytest.fail('slot spent by failed pack'))
    generated=access._ranked_placement_trials(baseline,FanoutOptions(),
        access.PackageAccessOptions(maximum_placement_probes=1),options,None,set(),records,None,_coupled_state=token)
    _,_,index=next(generated)
    assert index is None and records==[] and token.failed_slots==1
    assert token.attempts[0].borrowed_probe_index==1


def test_source_array_owner_and_relative_neighbor_remain_held(monkeypatch):
    from pcbir.physical import (PadViaInPadRule, RelativePlacementRule, RelativePlacementKind, PlacementTarget,
                                Stackup, CopperLayer, CopperZone, PolygonWithHoles, PolygonRing)
    for kind in ('array','relative'):
        baseline,options,cap,neighbor,requested=scene()
        if kind=='array':
            layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_1,CopperLayer.INTERNAL_2,
                    CopperLayer.INTERNAL_3,CopperLayer.INTERNAL_4,CopperLayer.BACK)
            zone=CopperZone('inner','GND',(CopperLayer.INTERNAL_1,),PolygonWithHoles(PolygonRing(tuple(
                Point.mm(x,y) for x,y in ((1,1),(19,1),(19,19),(1,19))))))
            baseline.source=replace(baseline.source,stackup=Stackup(layers),zones=(zone,),
                metadata={'fabrication_profile':'jlcpcb-six-layer'},
                via_in_pad_rules=(PadViaInPadRule(PadReference(neighbor,'2')),))
        else:
            baseline.source=replace(baseline.source,relative_rules=(RelativePlacementRule(
                RelativePlacementKind.MAX_DISTANCE,(PlacementTarget(neighbor),PlacementTarget('PKG')),distance_nm=20000000),))
        token=state()
        monkeypatch.setattr(repair,'_pack_local_components',lambda *a,**k:pytest.fail('protected owner cannot enter a domain'))
        assert token.prepass(baseline,[cap,neighbor],{cap:[requested]},options) is None
        assert token.attempts[0].outcome=='no_suitable_blocked_request' and not token.attempts[0].pack_work


def test_ambiguous_land_and_missing_pending_identity_decline_without_pack(monkeypatch):
    baseline,options,cap,neighbor,requested=scene()
    footprint=baseline.source.footprints[baseline.source.placements[2].footprint]
    # A source-legal duplicated physical pad number is not an unambiguous two-land cap.
    duplicate=replace(footprint,pads=(*footprint.pads,footprint.pads[0]))
    baseline.source=replace(baseline.source,footprints={**baseline.source.footprints,footprint.name:duplicate})
    monkeypatch.setattr(repair,'_pack_local_components',lambda *a,**k:pytest.fail('unknown identity cannot claim pack'))
    token=state()
    assert token.prepass(baseline,[cap],{cap:[requested]},options) is None
    assert token.considered and not token.claimed and token.attempts[0].outcome=='no_source_projection_hint'
    token.available_slots=4
    assert token.prepass(baseline,[cap],{cap:[requested]},options) is None and len(token.attempts)==1


@pytest.mark.parametrize('renamed',(False,True))
def test_ambiguous_association_is_explicitly_declined_not_overwritten(renamed):
    baseline,options,cap,neighbor,requested=scene(renamed)
    link=next(l for l in baseline.source.decoupling_links if l.capacitor.component==cap)
    raw=(*baseline.source.decoupling_links,link)
    assert cap not in repair._unique_associations(raw)
    assert neighbor in repair._unique_associations(raw)
    # The normal source model also rejects this ambiguity before geometry.
    with pytest.raises(ValueError,match='one explicit protected pin association'):
        replace(baseline.source,decoupling_links=raw)


def test_neighbor_probe_allowance_is_shared_across_requested_poses(monkeypatch):
    baseline,options,cap,neighbor,requested=scene()
    board=baseline.source;third='SECOND_NEIGHBOR'
    original={p.reference:p for p in board.placements}
    added=replace(original[neighbor],reference=third,position=Point.mm('11.75','10.5'))
    baseline.source=replace(board,placements=tuple(replace(p,position=Point.mm('11.75','9.5'))
        if p.reference==neighbor else p for p in board.placements)+(added,),
        nets=tuple(replace(n,pads=(*n.pads,PadReference(third,'1' if n.name=='RAIL' else '2')))
                   if n.name in {'RAIL','GND'} else n for n in board.nets),
        placement_rules=(*board.placement_rules,replace(next(r for r in board.placement_rules if r.reference==neighbor),reference=third)),
        decoupling_links=(*board.decoupling_links,DecouplingLink(PadReference(third,'1'),
            next(l.target for l in board.decoupling_links),PadReference(third,'2'),'RAIL','GND')))
    options=replace(options,exact_repair_limit=2,exact_repair_candidates=2)
    assert placement_solution_is_legal(baseline.source,{p.reference:p for p in baseline.source.placements},options)
    # First pose collides with BOTH neighbors: group3 exceeds original limit2.
    # A reset would examine the second pose and try its single blocker.
    second=replace(requested,position=Point.mm('10.5','10.5'))
    monkeypatch.setattr(repair,'_pack_local_components',lambda *a,**k:pytest.fail('no residual neighbor probes'))
    token=state()
    assert token.prepass(baseline,[cap,neighbor,third],{cap:[requested,second]},options) is None
    attempt=token.attempts[0]
    assert attempt.outcome=='neighbor_probe_exhausted' and attempt.neighbor_probes==attempt.neighbor_probe_limit==2
    assert attempt.requested_candidates_considered==2 and not attempt.pack_work


def test_actual_pack_stats_keep_small_original_numeric_limits():
    baseline,options,cap,neighbor,requested=scene()
    options=replace(options,legalization_candidates=2,exact_repair_candidates=2)
    token=state();token.prepass(baseline,[cap,neighbor],{cap:[requested]},options)
    work=dict(token.attempts[0].pack_work)
    assert work['limits']=={'raw_per_member':128,'retained_per_member':8,'search':256,'heuristic':85}
    assert work['search_operations']<=256 and work['heuristic_checks']<=85
    assert all(n<=128 for n in work['domain_probes'].values())
    assert all(n<=8 for n in work['retained_domains'].values())


def test_real_empty_neighbor_domain_records_exhaustion_and_preserves_source():
    baseline,options,cap,neighbor,requested=scene()
    tight=PlacementRegion('neighbor',BoardOutline(tuple(Point.mm(x,y) for x,y in
        (('11.25','9.75'),('12.25','9.75'),('12.25','10.25'),('11.25','10.25')))),side=BoardSide.BACK)
    baseline.source=replace(baseline.source,regions=(baseline.source.regions[0],tight))
    original=baseline.source
    assert placement_solution_is_legal(original,{p.reference:p for p in original.placements},options)
    token=state()
    assert token.prepass(baseline,[cap,neighbor],{cap:[requested]},options) is None
    work=dict(token.attempts[0].pack_work)
    assert token.failed_slots==1 and token.attempts[0].outcome=='bounded_pack_failed'
    assert work['outcome']=='empty_domain' and work['empty_domain']==neighbor
    assert work['search_operations']==0 and work['domain_probes'][neighbor]<=6144
    assert baseline.source is original


def test_real_rigid_front_template_remains_exact_during_back_neighbor_pack():
    from pcbir.physical import RigidPlacementCluster,RigidPlacementMember,PlacementTarget
    from pcbir.clusters import footprint_geometry_digest
    baseline,options,cap,neighbor,requested=scene()
    board=baseline.source;original={p.reference:p for p in board.placements}
    members=tuple(RigidPlacementMember(ref,original[ref].footprint,
        footprint_geometry_digest(board.footprints[original[ref].footprint]),local)
        for ref,local in [('PKG',Point(0,0)),('END',Point.mm(7,0))])
    cluster=RigidPlacementCluster('audited',PlacementTarget('PKG'),members,'synthetic exact template',(0,))
    baseline.source=replace(board,rigid_clusters=(cluster,),placement_rules=tuple(
        replace(r,fixed_position=None) if r.reference in {'PKG','END'} else r for r in board.placement_rules))
    assert {'PKG','END'}<=repair._protected(baseline.source,original,options)
    token=state();_,candidate=token.prepass(baseline,[cap,neighbor],{cap:[requested]},options)
    poses={p.reference:p for p in candidate.placements}
    assert poses['PKG']==original['PKG'] and poses['END']==original['END']
    assert placement_solution_is_legal(baseline.source,poses,options)


def test_accepted_round_does_not_renew_token_or_probe_slots(monkeypatch):
    simple,options,*_=scene();board=simple.source
    guides=SimpleNamespace(placement_fingerprint=_placement_fingerprint(board),status=GlobalRoutingStatus.SUCCESS)
    initial_pending=(*simple.pending_pads,PadReference('END','1'))
    baseline=access.PackageAccessResult(board,guides,FanoutResult(board,{},initial_pending,0,0),
        SimpleNamespace(board=board,status=CriticalRoutingStatus.FAILED),None,frozenset(),0)
    calls=[];real=repair._pack_local_components
    def counted(*args,**kwargs):
        calls.append(1);return real(*args,**kwargs)
    monkeypatch.setattr(repair,'_pack_local_components',counted)
    monkeypatch.setattr(access,'_placement_access_probe',fake_probe)
    monkeypatch.setattr(access,'route_global',lambda trial,*a,**k:SimpleNamespace(
        placement_fingerprint=_placement_fingerprint(trial),status=GlobalRoutingStatus.SUCCESS))
    def accepted_once(trial,guide,*args,**kwargs):
        return replace(baseline,source=trial,global_route=guide,
            fanout=FanoutResult(trial,{},(PadReference('END','1'),),0,0),
            critical=SimpleNamespace(board=trial,status=CriticalRoutingStatus.FAILED))
    monkeypatch.setattr(access,'preflight_package_access',accepted_once)
    result=access.improve_package_access(baseline,FanoutOptions(),placement_options=options,
        options=access.PackageAccessOptions(maximum_trials=2,maximum_placement_probes=4))
    assert result.accepted_moves==1 and len(calls)==1
    assert len(result.placement_probes)==4 and [p.index for p in result.placement_probes]==[1,2,3,4]
    assert len(result.coupled_placement_attempts)==1 and result.coupled_placement_attempts[0].borrowed_probe_index==1


def test_unreachable_or_congested_global_retains_success_gate(monkeypatch):
    simple,options,*_=scene();board=simple.source
    guides=SimpleNamespace(placement_fingerprint=_placement_fingerprint(board),status=GlobalRoutingStatus.SUCCESS)
    baseline=access.PackageAccessResult(board,guides,FanoutResult(board,{},tuple(simple.pending_pads),0,0),
        SimpleNamespace(board=board,status=CriticalRoutingStatus.FAILED),None,frozenset(),0)
    monkeypatch.setattr(access,'_placement_access_probe',fake_probe)
    monkeypatch.setattr(access,'route_global',lambda *a,**k:SimpleNamespace(status=GlobalRoutingStatus.OVERFLOW))
    monkeypatch.setattr(access,'preflight_package_access',lambda *a,**k:pytest.fail('global SUCCESS remains required'))
    result=access.improve_package_access(baseline,FanoutOptions(),placement_options=options,
        options=access.PackageAccessOptions(maximum_trials=1,maximum_placement_probes=1))
    assert result.accepted_moves==0 and result.trials[0].outcome=='global_failed' and result.source is board
