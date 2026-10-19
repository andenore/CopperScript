"""Package patterns preserve source rules and solve coupled contact geometry."""
from dataclasses import replace

import pytest

from pcbir.drc import PhysicalDrcPolicy, physical_board_digest, run_physical_drc
from pcbir.fanout import _legal_choices
from pcbir.package_patterns import (PackageContactIntent, PackagePatternOptions,
                                    circular_array, propose_package_patterns)
from pcbir.physical import (BoardSide, FootprintPad, NetRoutingRule, PadReference,
    PadShape, PhysicalFootprint, PhysicalNet, Placement, Point, Size, CopperLayer,
    TrackSegment, Via, PadViaInPadRule, ComponentPlacementRule)
from pcbir.placement import transformed_local_point
from pcbir.routing_clearance import RoutingClearanceIndex
from test_interstitial_fanout import fixture


def scene(rotation=0, side=BoardSide.FRONT, translated=False, renamed=False):
    board,pose,_,_ = fixture()
    pads = tuple(FootprintPad(str(y*5+x+1), Point((x-2)*750000,(y-2)*750000),
        Size(250000,250000),shape=PadShape.CIRCLE) for y in range(5) for x in range(5))
    ref = 'Renamed' if renamed else 'U'
    pose = replace(pose, reference=ref, rotation_degrees=rotation,side=side,
                   position=Point.mm(6,6) if translated else pose.position)
    board = replace(board,footprints={'array':PhysicalFootprint('array',pads,Size.mm(4,4))},
        placements=(pose,),nets=tuple(PhysicalNet(('Rail' if renamed else 'N')+p.number,
            (PadReference(ref,p.number),)) for p in pads),
        rules=replace(board.rules,default_track_width_nm=100000),
        placement_rules=(ComponentPlacementRule(ref,allowed_orientations=(0,45,90,180,270)),))
    intents = tuple(PackageContactIntent(PadReference(ref,n)) for n in ('7','8','9','12','13','14','17','18','19'))
    return board,pose,intents


def materialize(board,candidate):
    return replace(board,tracks=(*board.tracks,*candidate.tracks),vias=(*board.vias,*candidate.new_vias))


def errors(board):
    return [f for f in run_physical_drc(board,policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings
            if f.severity.value=='error' and f.code!='DRC-OPEN-NET']


@pytest.mark.parametrize('rotation',(0,45,90,180,270))
@pytest.mark.parametrize('side',(BoardSide.FRONT,BoardSide.BACK))
def test_patterns_transform_without_names_or_board_global_coordinates(rotation,side):
    board,pose,intents = scene(rotation,side,translated=True,renamed=True)
    before = physical_board_digest(board)
    result = propose_package_patterns(board,pose.reference,intents,
        PackagePatternOptions(families=('south_east',),frontier_offsets_per_family=4))
    assert result.status=='contact_geometry_proposals',result.report()
    assert result.candidates
    for candidate in result.candidates:
        for contact in candidate.contacts:
            pad = next(p for p in board.footprints[pose.footprint].pads if p.number==contact.intent.pad.pad)
            assert contact.via.position==transformed_local_point(pose,
                Point(pad.position.x_nm+candidate.offset_nm,pad.position.y_nm+candidate.offset_nm))
            assert len(contact.tracks)==1
            assert (contact.via.size_nm,contact.via.drill_nm)==(450000,200000)
        assert not errors(materialize(board,candidate))
    assert physical_board_digest(board)==before and not board.tracks and not board.vias
    assert result.report()['native_accepted'] is False
    assert result.report()['boundary_verified'] is False


def test_coherent_pattern_avoids_locally_legal_collision():
    board,pose,intents = scene()
    intents = tuple(i for i in intents if i.pad.pad in ('12','13'))
    ix=RoutingClearanceIndex(board)
    by={p.number:p for p in board.footprints[pose.footprint].pads}
    same=transformed_local_point(pose,Point(-375000,375000))
    greedy=[_legal_choices(board,ix,'N'+n,transformed_local_point(pose,by[n].position),pose,by[n],(same,))[0]
            for n in ('12','13')]
    assert not ix.candidate_vias_clear(tuple(v for _,v in greedy))
    result=propose_package_patterns(board,'U',intents)
    assert result.candidates and any(t.conflicts for t in result.trials)
    assert all(len({c.via.position for c in p.contacts})==len(intents) for p in result.candidates)
    assert all(not errors(materialize(board,p)) for p in result.candidates)
    assert result==propose_package_patterns(board,'U',reversed(intents))


def test_shared_rail_does_not_hide_coincident_via_or_drill_conflict():
    board,pose,intents=scene()
    intents=tuple(i for i in intents if i.pad.pad in ('12','13'))
    board=replace(board,nets=tuple(n for n in board.nets if n.name not in {'N12','N13'})+
                  (PhysicalNet('SUPPLY',tuple(i.pad for i in intents)),))
    intents=tuple(replace(i,role='supply') for i in intents)
    result=propose_package_patterns(board,'U',intents)
    assert result.candidates
    assert all(len(p.new_vias)==2 for p in result.candidates)
    assert any(t.conflicts for t in result.trials)
    assert all(not errors(materialize(board,p)) for p in result.candidates)


def test_opposite_face_pad_blocks_full_span_and_frontier_finds_common_offset():
    board,pose,intents=scene()
    intents=tuple(i for i in intents if i.pad.pad=='13')
    fp=PhysicalFootprint('back',(FootprintPad('1',Point(0,0),Size(100000,100000)),),Size(200000,200000))
    blocker=Placement('C','back',Point.mm('5.64','5.375'),side=BoardSide.BACK)
    board=replace(board,footprints={**board.footprints,'back':fp},placements=(*board.placements,blocker),
                  nets=tuple(replace(n,pads=(*n.pads,PadReference('C','1'))) if n.name=='N13' else n for n in board.nets))
    result=propose_package_patterns(board,'U',intents,PackagePatternOptions(families=('south_east',)))
    assert result.candidates
    assert next(t for t in result.trials if t.offset_nm==375000).blocked_pads
    assert any(247488<p.offset_nm<375000 for p in result.candidates)
    assert any(s.blockers for _,d in result.diagnostics for s in d.samples)
    assert all(not errors(materialize(board,p)) for p in result.candidates)


@pytest.mark.parametrize('kind',('missing','noncircular','pitch','rotation'))
def test_unsupported_geometry_is_explicit_and_does_not_drop_required_pads(kind):
    board,pose,intents=scene()
    fp=board.footprints['array']
    if kind=='missing':
        fp=replace(fp,pads=fp.pads[:-1])
        board=replace(board,nets=tuple(n for n in board.nets if n.name!='N25'))
    if kind=='noncircular':fp=replace(fp,pads=(replace(fp.pads[0],shape=PadShape.RECTANGLE),*fp.pads[1:]))
    if kind=='pitch':fp=replace(fp,pads=(replace(fp.pads[0],position=Point(-1500001,-1500000)),*fp.pads[1:]))
    if kind=='rotation':board=replace(board,placements=(replace(pose,rotation_degrees=17),))
    board=replace(board,footprints={'array':fp})
    result=propose_package_patterns(board,'U',intents)
    assert result.status==('unsupported_rotation' if kind=='rotation' else 'unsupported_array')
    assert set(result.intents)==set(intents) and not result.candidates and result.site_queries==0


@pytest.mark.parametrize('limit',('maximum_site_queries','maximum_pair_checks','maximum_pair_queries',
                                  'maximum_frontier_queries','maximum_patterns','maximum_pads'))
def test_limits_are_shared_and_failure_keeps_source(limit):
    board,_,intents=scene()
    before=physical_board_digest(board)
    result=propose_package_patterns(board,'U',intents,replace(PackagePatternOptions(),**{limit:1}))
    assert 'budget' in result.status
    measured={'maximum_site_queries':result.site_queries,'maximum_pair_checks':result.pair_checks,
              'maximum_pair_queries':result.pair_queries,'maximum_frontier_queries':result.frontier_queries,
              'maximum_patterns':len(result.trials),'maximum_pads':0}[limit]
    assert measured<=1
    assert physical_board_digest(board)==before


def test_source_via_and_length_limits_are_joint_not_per_pad():
    board,_,intents=scene()
    intents=tuple(i for i in intents if i.pad.pad in ('12','13'))
    board=replace(board,nets=tuple(n for n in board.nets if n.name not in {'N12','N13'})+
                  (PhysicalNet('SUPPLY',tuple(i.pad for i in intents)),),
                  net_routing_rules=(NetRoutingRule('SUPPLY',max_vias=1),))
    result=propose_package_patterns(board,'U',intents)
    assert not result.candidates and any('SUPPLY:via_budget' in t.rule_failures for t in result.trials)
    board=replace(board,net_routing_rules=(NetRoutingRule('SUPPLY',max_length_nm=600000),))
    result=propose_package_patterns(board,'U',intents)
    assert not result.candidates and any('SUPPLY:length_budget' in t.rule_failures for t in result.trials)


def test_local_width_cannot_bypass_fabrication_minimum():
    board,_,intents=scene()
    intents=tuple(i for i in intents if i.pad.pad=='13')
    board=replace(board,net_routing_rules=(NetRoutingRule('N13',width_nm=50000),))
    result=propose_package_patterns(board,'U',intents)
    assert not result.candidates
    assert any('fabrication_minimum_track_width' in t.rule_failures for t in result.trials)


def test_reuse_exact_existing_via_without_duplicating_or_spending_via_budget():
    board,pose,intents=scene()
    intents=tuple(i for i in intents if i.pad.pad=='13')
    via=Via('N13',Point.mm('5.375','5.375'),450000,200000)
    board=replace(board,vias=(via,),net_routing_rules=(NetRoutingRule('N13',max_vias=1),))
    result=propose_package_patterns(board,'U',intents,PackagePatternOptions(families=('south_east',)))
    candidate=next(p for p in result.candidates if p.offset_nm==375000)
    assert candidate.contacts[0].reused_via and candidate.new_vias==()
    assert not errors(materialize(board,candidate))


def test_straight_dogbone_preserves_declared_breakout_width_transition():
    board,_,intents=scene()
    intents=tuple(i for i in intents if i.pad.pad=='13')
    board=replace(board,net_routing_rules=(NetRoutingRule('N13',width_nm=400000,
        breakout_width_nm=100000,breakout_length_nm=200000),),
        tracks=(TrackSegment('N1',Point.mm('5.19','4.8'),Point.mm('5.21','4.8'),100000,CopperLayer.FRONT),))
    result=propose_package_patterns(board,'U',intents,PackagePatternOptions(families=('south_east',)))
    pattern=next(p for p in result.candidates if p.offset_nm==375000)
    assert {t.width_nm for t in pattern.tracks}=={100000,400000}
    assert not errors(materialize(board,pattern))


def test_surface_only_rule_and_unknown_target_layer_cannot_be_overridden():
    board,_,intents=scene()
    intent=next(i for i in intents if i.pad.pad=='13')
    board=replace(board,net_routing_rules=(NetRoutingRule('N13',allowed_layers=(CopperLayer.FRONT,)),))
    result=propose_package_patterns(board,'U',(intent,))
    assert result.status=='unsupported_obligations' and result.unsupported[0][1]=='layer_access_unavailable'
    board=replace(board,net_routing_rules=())
    result=propose_package_patterns(board,'U',(replace(intent,target_layers=(CopperLayer.INTERNAL_1,)),))
    assert result.status=='unsupported_obligations'


def test_required_array_cannot_be_replaced_by_single_dogbone():
    from test_pad_via_arrays import SIX
    from pcbir.physical import CopperZone,PolygonRing,PolygonWithHoles
    board,_,intents=scene()
    fp=board.footprints['array']
    board=replace(board,footprints={'array':replace(fp,pads=tuple(replace(p,size=Size(400000,400000)) for p in fp.pads))},
        nets=tuple(replace(n,name='GND') if intents[0].pad in n.pads else n for n in board.nets),
        stackup=SIX,metadata={'fabrication_profile':'jlcpcb-six-layer'},
        zones=(CopperZone('ground','GND',(CopperLayer.INTERNAL_1,),PolygonWithHoles(PolygonRing((
            Point.mm(1,1),Point.mm(11,1),Point.mm(11,11),Point.mm(1,11))))),),
        via_in_pad_rules=(PadViaInPadRule(intents[0].pad,'filled-capped',rows=1,columns=1),))
    with pytest.raises(ValueError,match='required via-in-pad arrays'):
        propose_package_patterns(board,'U',intents)
    from pcbir.pad_via_arrays import materialize_via_in_pad_arrays
    board=materialize_via_in_pad_arrays(board)
    before=board.vias
    result=propose_package_patterns(board,'U',intents)
    assert result.status=='unsupported_obligations' and not result.candidates
    assert result.unsupported==((intents[0].pad,'protected_macro_or_via_array'),)
    assert board.vias==before


def test_unnetted_or_duplicate_obligation_never_silently_disappears():
    board,_,intents=scene()
    with pytest.raises(ValueError,match='unique'):
        propose_package_patterns(board,'U',(intents[0],intents[0]))
    with pytest.raises(ValueError,match='actual netted land'):
        propose_package_patterns(board,'U',(PackageContactIntent(PadReference('U','missing')),))


def test_specialized_pair_remains_with_its_owner_and_telemetry_is_read_only():
    from pcbir.physical import RouteKind
    board,_,intents=scene()
    one=next(i for i in intents if i.pad.pad=='13')
    pair=replace(board,net_routing_rules=(NetRoutingRule('N13',kind=RouteKind.DIFFERENTIAL,
        differential_partner='N14',pair_gap_nm=100000),NetRoutingRule('N14',kind=RouteKind.DIFFERENTIAL,
        differential_partner='N13',pair_gap_nm=100000)))
    result=propose_package_patterns(pair,'U',(one,))
    assert result.status=='unsupported_obligations' and result.unsupported==((one.pad,'specialized_routing_owner'),)
    events=[]
    observed=propose_package_patterns(board,'U',(one,),on_progress=lambda *e:events.append(e))
    assert observed==propose_package_patterns(board,'U',(one,))
    assert events[0][:2]==('package_patterns','started')
    assert events[-1][:2]==('package_patterns','finished')


def test_macro_owned_pad_never_gets_an_additional_dogbone(tmp_path):
    from hashlib import sha256
    import json
    from pcbir.clusters import footprint_geometry_digest
    from pcbir.hard_macros import bind_hard_macro,materialize_hard_macros
    from test_hard_macros import fixture as macro_fixture
    board,_,intents=scene()
    _,data,_=macro_fixture(tmp_path)
    data.update(members=[dict(reference='chip',footprint='array',
        footprint_digest=footprint_geometry_digest(board.footprints['array']),
        center_nm=[0,0],rotation_degrees='0',edge_clearance_nm=250000)],
        pad_nets=[['chip','13','signal']],keepouts=[],protected_regions=[],
        tracks=[dict(net='signal',width_nm=100000,layer='F.Cu',points=[{'pad':['chip','13']},[250000,250000]])],
        ports=[dict(name='signal',net='signal',layer='F.Cu',point=[250000,250000],pads=[['chip','13']])])
    path=tmp_path/'pattern-macro.json';path.write_text(json.dumps(data),encoding='utf-8')
    source=bind_hard_macro(board,path,expected_sha256=sha256(path.read_bytes()).hexdigest(),
        name='protected',bindings={'chip':'U'},net_bindings={'signal':'N13'})
    assert propose_package_patterns(source,'U',intents).status=='unmaterialized_macro_source'
    held=materialize_hard_macros(source)
    result=propose_package_patterns(held,'U',intents)
    assert result.status=='unsupported_obligations'
    assert (PadReference('U','13'),'protected_macro_or_via_array') in result.unsupported
    assert held.tracks==materialize_hard_macros(held).tracks


def test_complete_pattern_connection_passes_native_drc(tmp_path):
    import json,subprocess
    from pathlib import Path
    from pcbir import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    cli=Path('C:/Program Files/KiCad/10.0/bin/kicad-cli.exe')
    if not cli.is_file():pytest.skip('KiCad CLI unavailable')
    board,_,intents=scene()
    intents=tuple(i for i in intents if i.pad.pad=='13')
    target=PhysicalFootprint('target',(FootprintPad('1',Point(0,0),Size(250000,250000),shape=PadShape.CIRCLE),),Size(400000,400000))
    end=Point.mm('5.375',8)
    board=replace(board,footprints={**board.footprints,'target':target},
        placements=(*board.placements,Placement('J','target',end,side=BoardSide.BACK)),
        nets=tuple(replace(n,pads=(*n.pads,PadReference('J','1'))) if n.name=='N13' else n for n in board.nets))
    result=propose_package_patterns(board,'U',intents,PackagePatternOptions(families=('south_east',)))
    pattern=next(p for p in result.candidates if p.offset_nm==375000)
    candidate=materialize(board,pattern)
    candidate=replace(candidate,tracks=(*candidate.tracks,TrackSegment('N13',pattern.contacts[0].via.position,end,100000,CopperLayer.BACK)))
    assert not errors(candidate)
    output=tmp_path/'complete-pattern.kicad_pcb'
    write_kicad_project(KiCadPcbBackend().generate(candidate),output)
    report=tmp_path/'drc.json'
    completed=subprocess.run([str(cli),'pcb','drc','--format','json','--output',str(report),str(output)],
        capture_output=True,text=True,timeout=60)
    assert completed.returncode==0,completed.stdout+completed.stderr
    native=json.loads(report.read_text(encoding='utf-8'))
    assert not native['unconnected_items']
    assert not [f for f in native['violations'] if f['severity']=='error'],native['violations']
