"""Patterns own whole groups; routing/return witnesses decide placement gains."""
from dataclasses import replace
from collections import Counter
import pytest

from pcbir.physical import (BoardSide,ComponentPlacementRule,PadReference,Placement,Point,Size)
from pcbir.placement import PlacementPlannerOptions,placement_solution_is_legal
from pcbir.support_patterns import (SupportPatternOptions,support_pattern_candidates,
                                    improve_support_placement,_paths)
from test_decoupling_crosslayer_paths import fixture as local_fixture


def scene(renamed=False):
    board,link=local_fixture(renamed=renamed)
    name=link.capacitor.component+'other'
    other=replace(link,capacitor=PadReference(name,'1'),return_pad=PadReference(name,'2'))
    board=replace(board,footprints={n:replace(fp,body_size=Size.mm(6,6) if n=='chip' else Size.mm(2,2))
                                    for n,fp in board.footprints.items()},
        placements=(*board.placements,Placement(name,'cap',Point.mm(16,13),side=BoardSide.BACK)),
        decoupling_links=(link,other),nets=tuple(replace(n,pads=(*n.pads,
            other.capacitor if n.name==link.net else other.return_pad)) for n in board.nets),
        net_routing_rules=tuple(replace(r,max_vias=6,max_length_nm=70_000_000) for r in board.net_routing_rules))
    board=replace(board,placement_rules=tuple(ComponentPlacementRule(p.reference,side=p.side,
        fixed_position=p.position if p.reference==link.target.component else None)
        for p in board.placements))
    return board,board.decoupling_links


def test_candidate_families_are_finite_source_bound_and_keep_electrical_roles():
    board,links=scene();options=SupportPatternOptions(patterns_per_group=20)
    work={'poses':0}
    result=support_pattern_candidates(board,links,PlacementPlannerOptions(),options=options,work=work)
    assert {'paired','staggered','pocket','split','land_rim'}<={p.family for p in result}
    assert len(result)<=20 and work['poses']<=options.maximum_pose_probes
    for pattern in result:
        assert pattern.links==links and pattern.source_digest
        poses={p.reference:p for p in board.placements};poses.update((p.reference,p) for p in pattern.poses)
        assert placement_solution_is_legal(board,poses)
        assert all(p.side is BoardSide.BACK for p in pattern.poses)
        if pattern.family=='source_frame':assert 'rotation' in dict(pattern.parameters)
    assert result==support_pattern_candidates(board,links,PlacementPlannerOptions(),options=options)


def test_land_rim_is_opposite_face_and_can_use_space_inside_package_courtyard():
    from pcbir.geometry import bounds
    from pcbir.placement import _placement_polygon
    board,links=scene()
    board=replace(board,footprints={**board.footprints,
        'chip':replace(board.footprints['chip'],body_size=Size.mm(12,12))})
    result=support_pattern_candidates(board,links,PlacementPlannerOptions(),
        options=SupportPatternOptions(patterns_per_group=40))
    rim=[p for p in result if p.family=='land_rim']
    assert rim and all(q.side is BoardSide.BACK for p in rim for q in p.poses)
    target=next(p for p in board.placements if p.reference==links[0].target.component)
    target_box=bounds(_placement_polygon(board,target))
    assert any(all(target_box.min_x<=q.position.x_nm<=target_box.max_x and
                   target_box.min_y<=q.position.y_nm<=target_box.max_y for q in p.poses) for p in rim)
    assert all(placement_solution_is_legal(board,{**{q.reference:q for q in board.placements},
        **{q.reference:q for q in p.poses}}) for p in rim)


def test_land_rim_includes_every_physical_land_of_repeated_pad_numbers():
    from pcbir.geometry import bounds
    from pcbir.placement import _placement_polygon
    board,links=scene();chip=board.footprints['chip']
    board=replace(board,footprints={**board.footprints,'chip':replace(chip,pads=(
        *chip.pads,replace(chip.pads[0],position=Point.mm(3,0))))})
    result=support_pattern_candidates(board,links,PlacementPlannerOptions(),
        options=SupportPatternOptions(patterns_per_group=100))
    rows=[p for p in result if p.family=='land_rim' and dict(p.parameters)['normal']==(1,0)]
    assert rows
    assert all(bounds(_placement_polygon(board,q)).min_x>=11_200_000 for p in rows for q in p.poses)


def test_bank_offsets_find_clear_interval_and_keep_relative_poses():
    from pcbir.support_patterns import _bank_offsets
    from pcbir.physical import PlacementRegion,BoardOutline
    from pcbir.placement import _legal
    board,links=scene();caps=[p for p in board.placements if p.reference!=links[0].target.component]
    row=tuple(replace(p,position=Point.mm(x,10)) for p,x in zip(caps,(10,12.5)))
    held=tuple(replace(caps[0],reference=f'block{i}',position=Point.mm(x,y))
               for i,(x,y) in enumerate(((10,10),(12.5,10),(11.25,6.5))))
    board=replace(board,placements=(*board.placements,*held),
        regions=(PlacementRegion('bank',BoardOutline.rectangle('4.7',8,origin=Point.mm('8.9',6)),BoardSide.BACK),),
        placement_rules=tuple(replace(r,region='bank') if r.reference in {p.reference for p in row} else r
                              for r in board.placement_rules))
    offsets,_=_bank_offsets(board,row,{p.reference:p for p in held},PlacementPlannerOptions(),SupportPatternOptions())
    assert (0,2_500_000) in offsets
    for dx,dy in offsets:
        moved=tuple(replace(p,position=Point(p.position.x_nm+dx,p.position.y_nm+dy)) for p in row)
        placed={p.reference:p for p in held}
        for p in moved:
            assert _legal(p,placed,board,PlacementPlannerOptions())
            placed[p.reference]=p
        assert moved[1].position.x_nm-moved[0].position.x_nm==2_500_000
        assert moved[1].position.y_nm==moved[0].position.y_nm


def test_opposite_face_bank_pitch_uses_capacitor_pair_gap():
    board,links=scene()
    board=replace(board,placement_rules=tuple(replace(r,additional_courtyard_gap_nm=
        2_000_000 if r.reference==links[0].target.component else 200_000) for r in board.placement_rules))
    patterns=support_pattern_candidates(board,links,PlacementPlannerOptions(),options=SupportPatternOptions(patterns_per_group=100))
    rows=[p for p in patterns if p.family=='pocket' and dict(p.parameters).get('normal')==(0,1)]
    assert rows
    assert all(abs(p.poses[1].position.x_nm-p.poses[0].position.x_nm)==2_200_000 for p in rows)


@pytest.mark.parametrize('budget',(1,5,32,128,512))
def test_offset_and_source_frame_families_share_raw_pose_bound(budget):
    board,links=scene();work={'poses':0}
    result=support_pattern_candidates(board,links,PlacementPlannerOptions(),
        options=SupportPatternOptions(maximum_pose_probes=budget,patterns_per_group=30),work=work)
    assert work['poses']<=budget
    old={p.reference:p for p in board.placements}
    for p in result:
        assert placement_solution_is_legal(board,{**old,**{q.reference:q for q in p.poses}})
        if p.family=='source_frame':
            a,z=p.poses;oa,oz=old[a.reference],old[z.reference]
            assert (a.position.x_nm-z.position.x_nm)**2+(a.position.y_nm-z.position.y_nm)**2==(
                oa.position.x_nm-oz.position.x_nm)**2+(oa.position.y_nm-oz.position.y_nm)**2
    fixed=replace(board,placement_rules=tuple(replace(r,fixed_position=old[r.reference].position) for r in board.placement_rules))
    assert support_pattern_candidates(fixed,links,PlacementPlannerOptions())==()
    exhausted={'poses':budget}
    assert support_pattern_candidates(board,links,PlacementPlannerOptions(),options=SupportPatternOptions(maximum_pose_probes=budget),work=exhausted)==()
    assert exhausted=={'poses':budget}


@pytest.mark.parametrize('renamed',(False,True))
def test_joint_bank_improves_complete_supply_and_return_geometry(renamed):
    board,links=scene(renamed);poses={p.reference:p for p in board.placements}
    options=SupportPatternOptions(maximum_complete_checks=5)
    result=improve_support_placement(board,poses,PlacementPlannerOptions(),options=options)
    assert result.status=='geometry_improved',result.report()
    assert set(result.verified)==set(links)
    assert len(_paths(result.proposal,links,options.maximum_path_states))==2
    assert result.board.tracks==result.board.vias==()
    assert result.board.placements!=board.placements
    assert all(p==poses[p.reference] for p in result.board.placements if p.reference not in {l.capacitor.component for l in links})
    assert placement_solution_is_legal(board,{p.reference:p for p in result.board.placements})
    assert result.complete_checks<=5 and result.routing_passes<=result.complete_checks+2
    assert result.pose_probes<=options.maximum_pose_probes
    assert result.search_states<=options.maximum_search_states and result.pair_checks<=options.maximum_pair_checks
    assert not result.report()['native_acceptance'] and board.tracks==board.vias==()


def test_fixed_bank_is_diagnosed_without_routing_or_pose_trials():
    board,links=scene();poses={p.reference:p for p in board.placements}
    result=improve_support_placement(board,poses,PlacementPlannerOptions(fixed_references=frozenset(poses)))
    assert result.status=='fixed_or_unsupported' and result.board==board
    assert set(result.protected_members)=={l.capacitor.component for l in links}
    assert result.complete_checks==result.routing_passes==result.pose_probes==0


@pytest.mark.parametrize('renamed',(False,True))
def test_same_rail_at_distinct_package_pads_keeps_local_banks(renamed):
    from pcbir.support_patterns import _groups,_protected_members
    board,links=scene(renamed)
    second=replace(links[1],target=PadReference(links[0].target.component,'S2'))
    chip=board.footprints['chip']
    board=replace(board,footprints={**board.footprints,'chip':replace(chip,pads=(
        *chip.pads,replace(chip.pads[0],number='S2',position=Point.mm(-2,-1))))},
        decoupling_links=(links[0],second),nets=tuple(replace(n,pads=(*n.pads,second.target))
            if n.name==second.net else n for n in board.nets))
    poses={p.reference:p for p in board.placements};placement=PlacementPlannerOptions()
    groups=_groups(board,frozenset(board.decoupling_links),
        _protected_members(board,poses,placement),SupportPatternOptions())
    assert groups==((links[0],),(second,))
    assert all(support_pattern_candidates(board,g,placement) for g in groups)
    # One physical capacitor associated with both balls cannot become two
    # independent placement owners, even when its rail name is identical.
    shared=replace(second,capacitor=links[0].capacitor,return_pad=links[0].return_pad)
    with pytest.raises(ValueError,match='one explicit protected pin association'):
        replace(board,decoupling_links=(links[0],shared))


def test_missing_return_or_forbidden_transition_cannot_win_by_compactness():
    board,links=scene()
    board=replace(board,net_routing_rules=tuple(replace(r,max_vias=0) if r.net==links[0].return_net else r
                                               for r in board.net_routing_rules))
    poses={p.reference:p for p in board.placements}
    result=improve_support_placement(board,poses,PlacementPlannerOptions(),
        options=SupportPatternOptions(maximum_complete_checks=2))
    assert result.board==board and not result.verified and result.proposal is None
    assert all(t['status']!='verified_geometry' for t in result.trials)


def test_final_regeneration_failure_rolls_back_the_whole_group(monkeypatch):
    import pcbir.support_patterns as patterns
    board,links=scene();poses={p.reference:p for p in board.placements}
    original=patterns._route;calls=[]
    def route(current,*a,**k):
        result=original(current,*a,**k);calls.append(current.placements)
        # Five candidate checks plus baseline and separate final regeneration.
        return replace(result,vias=()) if len(calls)==7 else result
    monkeypatch.setattr(patterns,'_route',route)
    result=patterns.improve_support_placement(board,poses,PlacementPlannerOptions(),
        options=SupportPatternOptions(maximum_complete_checks=5))
    assert len(calls)==7 and result.status=='final_regeneration_rejected',result.report()
    assert result.board==board and result.proposal is None


def test_native_bank_relocation_keeps_both_nets_connected_without_danglers(tmp_path):
    from pathlib import Path
    import shutil
    from pcbir.plane_verify import verify_filled_planes
    from pcbir.decoupling import route_decouplers
    cli=shutil.which('kicad-cli')
    if not cli:pytest.skip('KiCad CLI unavailable')
    board,links=scene();poses={p.reference:p for p in board.placements}
    result=improve_support_placement(board,poses,PlacementPlannerOptions())
    assert result.status=='geometry_improved'
    before=route_decouplers(board).board
    evidence=[]
    for name,current in (('before',before),('after',result.proposal)):
        e=verify_filled_planes(current,kicad_cli=Path(cli),artifact_directory=tmp_path/name)
        assert e.matches(current) and e.filled_board_digest and e.report_digest
        assert e.unconnected_count==e.other_violation_count==e.island_count==0,e.to_json()
        evidence.append(e)
    # These are intentionally changed poses, so source-inventory checks for
    # frozen power-preparation replacement are not bypassed or reused here.
    assert before.nets==result.proposal.nets and before.placements!=result.proposal.placements


def test_reserved_signal_path_is_freshly_preserved_after_bank_movement():
    from pcbir.physical import FootprintPad,PhysicalNet,TrackSegment,CopperLayer
    from pcbir.pin_escape import RoutingAccess,verified_routing_access
    from pcbir.routing_clearance import RoutingClearanceIndex
    board,links=scene();fp=board.footprints['chip']
    pad=FootprintPad('SIG',Point.mm(1,-1),Size.mm('.4','.4'))
    terminal=PadReference(links[0].target.component,'SIG')
    board=replace(board,footprints={**board.footprints,'chip':replace(fp,pads=(*fp.pads,pad))},
        nets=(*board.nets,PhysicalNet('SIGNAL',(terminal,))))
    track=TrackSegment('SIGNAL',Point.mm(9,7),Point.mm(12,7),150000,CopperLayer.FRONT)
    port=RoutingAccess(track.end,track.layer,track.start,(track,))
    result=improve_support_placement(board,{p.reference:p for p in board.placements},PlacementPlannerOptions(),
        reserved_tracks=(track,),ports=((terminal,port),))
    assert result.status=='geometry_improved',result.report()
    assert track not in result.proposal.tracks and track in result.signal_scene.tracks
    assert verified_routing_access(result.signal_scene,terminal,'SIGNAL',port,RoutingClearanceIndex(result.signal_scene))
    with pytest.raises(ValueError,match='fresh ordinary'):
        improve_support_placement(board,{p.reference:p for p in board.placements},PlacementPlannerOptions(),
            reserved_tracks=(replace(track,start=Point.mm(10,7)),),ports=((terminal,port),))


def test_two_groups_can_swap_occupied_sites_only_as_one_complete_assignment(monkeypatch):
    import pcbir.support_patterns as patterns
    from pcbir.physical import PhysicalNet
    board,links=scene();first=links[0];second=replace(links[1],target=PadReference('U_other','S'),net='P_other',return_net='R_other')
    left=Point.mm(8,13);right=Point.mm(19,13)
    old={p.reference:p for p in board.placements}
    old[first.capacitor.component]=replace(old[first.capacitor.component],position=right)
    old[second.capacitor.component]=replace(old[second.capacitor.component],position=left)
    anchor=replace(old[first.target.component],reference='U_other',position=Point.mm(19,8))
    board=replace(board,placements=(*old.values(),anchor),decoupling_links=(first,second),
        nets=(PhysicalNet(first.net,(first.target,first.capacitor)),
              PhysicalNet(first.return_net,(PadReference(first.target.component,'G'),first.return_pad)),
              PhysicalNet(second.net,(second.target,second.capacitor)),
              PhysicalNet(second.return_net,(PadReference(second.target.component,'G'),second.return_pad))),
        net_routing_rules=(*board.net_routing_rules,*(replace(r,net=second.net if r.net==first.net else second.return_net)
                                                     for r in board.net_routing_rules)),
        placement_rules=(*board.placement_rules,ComponentPlacementRule('U_other',fixed_position=anchor.position,side=anchor.side)))
    poses={p.reference:p for p in board.placements}
    proposed={first.capacitor.component:replace(poses[first.capacitor.component],position=left),
              second.capacitor.component:replace(poses[second.capacitor.component],position=right)}
    assert not placement_solution_is_legal(board,{**poses,first.capacitor.component:proposed[first.capacitor.component]})
    assert not placement_solution_is_legal(board,{**poses,second.capacitor.component:proposed[second.capacitor.component]})
    assert placement_solution_is_legal(board,{**poses,**proposed})
    def domain(source,group,placement_options,**kwargs):
        return (patterns.SupportPattern(patterns.physical_board_digest(source),'edge',
            (proposed[group[0].capacitor.component],),group),)
    monkeypatch.setattr(patterns,'support_pattern_candidates',domain)
    result=patterns.improve_support_placement(board,poses,PlacementPlannerOptions())
    assert result.status=='geometry_improved',result.report()
    assert set(result.verified)=={first,second}
    assert {p.reference:p for p in result.board.placements}=={**poses,**proposed}


@pytest.mark.parametrize('angle',(0,45,90,180,270))
def test_rotated_and_mirrored_sources_keep_pad_identity_and_legality(angle):
    from pcbir.placement import transformed_local_point
    board,links=scene();origin=Placement('frame','chip',Point.mm(13,10),rotation_degrees=angle)
    transformed=tuple(replace(p,position=transformed_local_point(origin,Point(
        p.position.x_nm-13_000_000,p.position.y_nm-10_000_000)),rotation_degrees=p.rotation_degrees+angle)
        for p in board.placements)
    from pcbir.physical import BoardOutline
    # Larger material avoids confusing rotation with board-edge restrictions.
    transformed=tuple(replace(p,position=Point(p.position.x_nm+10_000_000,p.position.y_nm+10_000_000)) for p in transformed)
    board=replace(board,outline=BoardOutline.rectangle(45,40),placements=transformed,
        placement_rules=tuple(replace(r,fixed_position=next(p.position for p in transformed if p.reference==r.reference)
             if r.fixed_position else None,allowed_orientations=tuple((a+angle)%360 for a in r.allowed_orientations))
             for r in board.placement_rules))
    candidates=support_pattern_candidates(board,links,PlacementPlannerOptions())
    assert candidates and any(p.family=='pocket' for p in candidates)
    for candidate in candidates:
        all_poses={p.reference:p for p in board.placements};all_poses.update((p.reference,p) for p in candidate.poses)
        assert placement_solution_is_legal(board,all_poses)
        assert candidate.links==links


def test_normal_placement_stage_runs_pattern_search_and_keeps_its_report():
    from pcbir.placement import generate_placement_candidates
    board,links=scene()
    options=PlacementPlannerOptions(analytical_iterations=0,refinement_passes=0,candidate_count=1,
        escape_spacing_passes=0,support_pattern_trials=3)
    candidates=generate_placement_candidates(board,options)
    assert candidates and candidates[0].statistics.support_patterns
    assert candidates[0].statistics.support_patterns['complete_checks']<=3
    assert not candidates[0].statistics.support_patterns['native_acceptance']


def test_stale_pattern_cannot_authorize_component_moves(monkeypatch):
    import pcbir.support_patterns as patterns
    board,links=scene();poses={p.reference:p for p in board.placements}
    original=patterns.support_pattern_candidates
    def stale(*a,**k):
        return tuple(replace(p,source_digest='0'*64) for p in original(*a,**k))
    monkeypatch.setattr(patterns,'support_pattern_candidates',stale)
    result=patterns.improve_support_placement(board,poses,PlacementPlannerOptions())
    assert result.board==board and result.complete_checks==0
    assert result.trials and all(t['status']=='stale_or_incomplete_pattern' for t in result.trials)


def test_scope_keeps_sibling_source_roles_unobserved_and_never_changes_inventory():
    board,links=scene();poses={p.reference:p for p in board.placements}
    result=improve_support_placement(board,poses,PlacementPlannerOptions(),only_links=frozenset(links[:1]))
    assert result.board.decoupling_links==board.decoupling_links and result.board.nets==board.nets
    assert result.scope==links[:1] and result.report()['unobserved_associations']==1
    assert next(p for p in result.board.placements if p.reference==links[1].capacitor.component)==poses[links[1].capacitor.component]
    with pytest.raises(ValueError,match='immutable'):
        improve_support_placement(board,poses,PlacementPlannerOptions(),only_links=set(links))


@pytest.mark.parametrize('command',('plan-layout','route-global','route-board'))
def test_normal_cli_exposes_bounded_opt_in(command):
    from pcbir.cli import _parser,_planner_options
    parser=_parser();a=_planner_options(parser.parse_args([command,'board.copper']))
    b=_planner_options(parser.parse_args([command,'board.copper','--support-pattern-trials','3']))
    assert a.support_pattern_trials==0 and b.support_pattern_trials==3
    assert replace(b,support_pattern_trials=0)==a
    with pytest.raises(ValueError):_planner_options(parser.parse_args([command,'board.copper','--support-pattern-trials','-1']))


@pytest.mark.parametrize('value',(0,-1,True,1.5))
def test_invalid_pattern_work_bounds_are_rejected(value):
    with pytest.raises(ValueError):SupportPatternOptions(maximum_search_states=value)
