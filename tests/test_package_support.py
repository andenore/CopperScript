from dataclasses import replace
import pytest

from pcbir.package_support import plan_package_support,PackageSupportOptions
from pcbir.package_patterns import PackageContactIntent,PackagePatternOptions,propose_package_patterns
from pcbir.physical import (PhysicalFootprint,FootprintPad,Size,Point,Placement,BoardSide,
    PadReference,DecouplingLink,ComponentPlacementRule,PhysicalNet,TrackSegment,CopperLayer)
from pcbir.placement import PlacementPlannerOptions,placement_solution_is_legal,transformed_local_point
from pcbir.drc import physical_board_digest
from pcbir.routing_clearance import RoutingClearanceIndex
from test_package_patterns import scene,errors


def fixture(rotation=0,renamed=False):
    b,p,unused=scene(rotation=rotation,renamed=renamed)
    target=PadReference(p.reference,'13');ret=PadReference(p.reference,'12')
    member={x:n.name for n in b.nets for x in n.pads}
    ref='DifferentCap' if renamed else 'C'
    cap=PhysicalFootprint('cap',(FootprintPad('1',Point(-400000,0),Size(400000,400000)),
        FootprintPad('2',Point(400000,0),Size(400000,400000))),Size(1300000,650000))
    # A different-side land obstructs the package contact. Source pose itself
    # remains a valid component placement before routing.
    pos=transformed_local_point(p,Point(647489,247489))
    cp=Placement(ref,'cap',pos,rotation_degrees=rotation,side=BoardSide.BACK)
    link=DecouplingLink(PadReference(ref,'1'),target,PadReference(ref,'2'),member[target],member[ret])
    b=replace(b,footprints={**b.footprints,'cap':cap},placements=(*b.placements,cp),
        decoupling_links=(link,),nets=tuple(replace(n,pads=(*n.pads,link.capacitor if n.name==link.net else link.return_pad))
            if n.name in (link.net,link.return_net) else n for n in b.nets),
        placement_rules=(*b.placement_rules,ComponentPlacementRule(ref,side=BoardSide.BACK)))
    intents=(PackageContactIntent(target,'supply'),PackageContactIntent(ret,'return'))
    return b,p,intents,link


@pytest.mark.parametrize('rotation',(0,90,180,270))
def test_via_first_placement_restores_all_lands_and_pin_roles(rotation):
    b,p,intents,link=fixture(rotation,renamed=rotation!=0)
    before=physical_board_digest(b)
    result=plan_package_support(b,p.reference,intents,PlacementPlannerOptions(),
        pattern_options=PackagePatternOptions(families=('south_east',)))
    assert result.status=='complete_geometry_proposal',result.report()
    assert result.moved==(link.capacitor.component,)
    assert physical_board_digest(b)==before
    assert result.board.nets==b.nets and result.board.decoupling_links==b.decoupling_links
    assert result.board.placement_rules==b.placement_rules and result.board.tracks==b.tracks
    assert not result.pattern.deferred_components
    assert result.pattern.source_digest==physical_board_digest(result.board)
    assert placement_solution_is_legal(b,{p.reference:p for p in result.board.placements})
    assert not errors(result.proposal)
    assert result.pattern in propose_package_patterns(result.board,p.reference,intents,
        PackagePatternOptions(families=('south_east',))).candidates


def test_fixed_capacitor_is_not_released_to_make_room():
    b,p,intents,link=fixture();cp=b.placements[-1]
    b=replace(b,placement_rules=(*b.placement_rules[:-1],replace(b.placement_rules[-1],fixed_position=cp.position)))
    result=plan_package_support(b,p.reference,intents,PlacementPlannerOptions(),
        pattern_options=PackagePatternOptions(families=('south_east',)))
    # A different legal offset is permitted, but never a moved fixed capacitor.
    assert not result.moved and result.board.placements==b.placements


def test_rejection_rolls_back_entire_trial_and_never_reports_acceptance():
    b,p,intents,link=fixture();seen=[]
    result=plan_package_support(b,p.reference,intents,PlacementPlannerOptions(),
        options=PackageSupportOptions(maximum_complete_checks=2),
        accept=lambda board,pattern:seen.append((board,pattern)) or False)
    assert len(seen)==2
    assert result.board is b and result.pattern is None and result.proposal is None
    assert dict(result.work)['complete_checks']==2
    assert not result.report()['native_accepted']


def test_existing_partial_copper_contact_protects_capacitor():
    b,p,intents,link=fixture();cp=b.placements[-1]
    from pcbir.placement import transformed_pad_position
    start=transformed_pad_position(b,cp,'1')
    b=replace(b,tracks=(TrackSegment(link.net,start,Point(start.x_nm,start.y_nm+100000),100000,CopperLayer.BACK),))
    result=plan_package_support(b,p.reference,intents,PlacementPlannerOptions())
    assert not result.moved and result.board.placements==b.placements
    assert result.board.tracks==b.tracks


def test_deferred_index_is_private_and_cannot_poison_full_pad_cache():
    b,p,intents,link=fixture();ref=link.capacitor.component
    a=propose_package_patterns(b,p.reference,intents,PackagePatternOptions(families=('south_east',)))
    deferred=propose_package_patterns(b,p.reference,intents,PackagePatternOptions(families=('south_east',)),
        _deferred_components=frozenset({ref}))
    assert deferred.deferred_components==(ref,)
    assert all(not t['complete_geometry'] for t in deferred.report()['trials'])
    assert all(c.deferred_components==(ref,) for c in deferred.candidates)
    assert propose_package_patterns(b,p.reference,intents,PackagePatternOptions(families=('south_east',)))==a
    with pytest.raises(ValueError):RoutingClearanceIndex(b,_deferred_components=frozenset({'missing'}))


def test_work_limit_returns_original_inventory():
    b,p,intents,link=fixture()
    result=plan_package_support(b,p.reference,intents,PlacementPlannerOptions(),
        options=PackageSupportOptions(maximum_pose_checks=1))
    assert result.status=='pose_budget_exhausted' and result.board is b
    assert dict(result.work)['poses']==1


def test_preferred_offset_survives_fresh_obstacle_frontier_changes():
    b,p,intents,link=fixture()
    result=plan_package_support(b,p.reference,intents,PlacementPlannerOptions())
    assert result.pattern is not None
    fresh=propose_package_patterns(result.board,p.reference,intents,
        PackagePatternOptions(preferred_offset_nm=result.pattern.offset_nm))
    assert result.pattern in fresh.candidates
    assert len(fresh.trials)<=24 and fresh.frontier_queries==0


def test_standard_placement_adapter_keeps_proof_copper_out_of_source():
    from pcbir.package_support import improve_package_support_placement
    b,p,intents,link=fixture()
    poses,reports=improve_package_support_placement(b,{p.reference:p for p in b.placements},
        PlacementPlannerOptions(package_support_trials=2))
    assert reports[0]['status']=='complete_geometry_proposal'
    assert reports[0]['component']==p.reference
    assert poses[link.capacitor.component]!=b.placements[-1]
    assert not b.tracks and not b.vias


@pytest.mark.parametrize('command',('plan-layout','route-global','route-board'))
def test_cli_package_placement_is_explicit_and_bounded(command):
    from pcbir.cli import _parser,_planner_options
    parser=_parser()
    assert _planner_options(parser.parse_args([command,'board.copper'])).package_support_trials==0
    assert _planner_options(parser.parse_args([command,'board.copper','--package-support-trials','2'])).package_support_trials==2
    with pytest.raises(ValueError):_planner_options(parser.parse_args([command,'board.copper','--package-support-trials','-1']))


def test_package_preparation_owns_only_produced_contacts_and_keeps_signal_reservations():
    from pcbir.package_preparation import prepare_package_patterns
    from pcbir.copper_ownership import begin_copper_ownership
    from pcbir.power_geometry import PowerAreaOptions
    b,p,intents,link=fixture()
    # A second terminal makes this general signal an active routing obligation.
    signal=PadReference(p.reference,'7')
    signal_net=next(n for n in b.nets if signal in n.pads)
    b=replace(b,nets=tuple(replace(n,pads=(*n.pads,PadReference(p.reference,'8')))
        if n is signal_net else n for n in b.nets if n.name!=next(n.name for n in b.nets if PadReference(p.reference,'8') in n.pads)))
    from pcbir.package_support import package_contact_intents
    result=plan_package_support(b,p.reference,package_contact_intents(b,p.reference),PlacementPlannerOptions())
    assert result.pattern is not None
    source=result.board;ownership=begin_copper_ownership(source)
    options=PowerAreaOptions(nets=(link.net,),package_patterns_before_contacts=True,
        package_pattern_offset_nm=result.pattern.offset_nm)
    board,owned,settings,reports=prepare_package_patterns(source,ownership,(link.net,),(link,),options)
    assert owned.matches(board)
    assert board.vias and all(v.net in (link.net,link.return_net) for v in board.vias)
    assert settings.reserved_vias and all(v.net==signal_net.name for v in settings.reserved_vias)
    assert all(not o.trial_mutable for o in owned.occurrences)
    assert reports and reports[0]['candidates']
    assert source==result.board and source.vias==b.vias


def test_package_preparation_rejection_rolls_back_ownership_and_options():
    from pcbir.package_preparation import prepare_package_patterns
    from pcbir.copper_ownership import begin_copper_ownership
    from pcbir.power_geometry import PowerAreaOptions
    b,p,intents,link=fixture()
    # An impossible process dimension rejects all interstitial offsets.
    b=replace(b,rules=replace(b.rules,default_via_size_nm=2000000))
    ownership=begin_copper_ownership(b);options=PowerAreaOptions(package_patterns_before_contacts=True)
    after,owned,settings,reports=prepare_package_patterns(b,ownership,(link.net,),(link,),options)
    assert after is b and owned is ownership and settings is options
    assert not reports[0]['candidates']


def test_existing_duplicate_copper_occurrences_survive_placement_and_preparation():
    from pcbir.package_preparation import prepare_package_patterns
    from pcbir.package_support import package_contact_intents
    from pcbir.copper_ownership import begin_copper_ownership
    from pcbir.power_geometry import PowerAreaOptions
    b,p,intents,link=fixture()
    trace=TrackSegment(link.net,Point.mm(1,1),Point.mm(2,1),100000,CopperLayer.FRONT)
    b=replace(b,tracks=(trace,trace))
    result=plan_package_support(b,p.reference,package_contact_intents(b,p.reference),PlacementPlannerOptions())
    assert result.pattern is not None
    assert result.board.tracks==b.tracks and result.proposal.tracks[:2]==b.tracks
    board,owned,_,_=prepare_package_patterns(result.board,begin_copper_ownership(result.board),
        (link.net,),(link,),PowerAreaOptions(package_pattern_offset_nm=result.pattern.offset_nm))
    assert board.tracks[:2]==(trace,trace) and owned.matches(board)
    assert len([o for o in owned.occurrences if o.kind=='track' and o.index<2])==2


def test_contact_snapshot_rejects_changed_or_conflicting_package_reservations():
    from pcbir.power_preparation import ContactPreparationSnapshot
    from pcbir.copper_ownership import begin_copper_ownership,append_copper_ownership
    from pcbir.critical import CriticalRoutingResult,CriticalRoutingStatus
    from pcbir.physical import Via
    b,p,intents,link=fixture()
    critical=CriticalRoutingResult(CriticalRoutingStatus.SUCCESS,b,(),(),(),'','')
    track=TrackSegment('N7',Point.mm(1,1),Point.mm(2,1),100000,CopperLayer.FRONT)
    via=Via('N7',Point.mm(2,1),450000,200000,CopperLayer.FRONT,CopperLayer.BACK)
    snapshot=ContactPreparationSnapshot(b,critical,b,begin_copper_ownership(b),(link.net,),frozenset(),
        package_reserved_tracks=(track,),package_reserved_vias=(via,))
    assert snapshot.matches()
    assert not replace(snapshot,package_reserved_tracks=()).matches()
    assert not replace(snapshot,package_reserved_vias=(replace(via,position=Point.mm(3,1)),)).matches()
    # Even with a freshly computed reservation digest, producer-net overlap is invalid.
    assert not replace(snapshot,package_reserved_vias=(replace(via,net=link.net),),
                       package_reservation_digest='').matches()
    blocker=replace(track,net=link.net)
    changed=replace(b,tracks=(blocker,))
    owned=append_copper_ownership(snapshot.ownership,b,changed,'test-contact')
    assert not replace(snapshot,board=changed,ownership=owned).matches()


def test_early_ordinary_preparation_does_not_duplicate_existing_reserved_drills(monkeypatch):
    from collections import Counter
    from pcbir import plane
    from pcbir.power_preparation import prepare_power_contacts
    from pcbir.critical import CriticalRoutingResult,CriticalRoutingStatus
    from pcbir.power_geometry import PowerAreaOptions
    from pcbir.fanout import FanoutOptions
    from pcbir.physical import Via
    b,p,intents,link=fixture()
    placed=plan_package_support(b,p.reference,intents,PlacementPlannerOptions())
    b=placed.board
    critical=CriticalRoutingResult(CriticalRoutingStatus.SUCCESS,b,(),(),(),'','')
    reserved=Via('N7',Point.mm(1,1),450000,200000,CopperLayer.FRONT,CopperLayer.BACK)
    options=PowerAreaOptions(nets=(link.net,),plane_return_nets=(link.net,),
        package_patterns_before_contacts=True,package_pattern_offset_nm=placed.pattern.offset_nm,
        reserve_ordinary_before_contacts=True,reserved_vias=(reserved,))
    seen=[];original=plane.stitch_zone_pads
    def capture(board,*args,**kwargs):
        seen.append(Counter(board.vias)[reserved])
        return original(board,*args,**kwargs)
    monkeypatch.setattr(plane,'stitch_zone_pads',capture)
    result=prepare_power_contacts(b,critical,options,FanoutOptions(plane_return_nets={link.net}),ordinary_nets=frozenset())
    assert result.status=='contacts_prepared',result.report()
    assert seen and all(count==1 for count in seen)
    assert reserved not in result.board.vias
    assert result.contact_snapshot.matches()
