"""One late completion pass preserves ordinary owners and exact source IDs."""
from dataclasses import asdict,replace
from types import MappingProxyType
import json
import pytest
import pcbir.decoupling as decoupling
import pcbir.decoupling_paths as paths
from pcbir.fanout import FanoutOptions,FanoutResult,FanoutPinAnalysis,route_fanout
from pcbir.fanout_bypass_completion import complete_late_bypasses
from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.pin_escape import RoutingAccess
from pcbir.physical import FootprintPad,PadReference,Point,Size,TrackSegment,CopperLayer
from test_decoupling_crosslayer_paths import fixture
from test_decoupling_existing_launches import scene


def incumbent_scene():
    board,link=scene()
    source=replace(board,tracks=(),vias=())
    ports={link.target:board.vias[0].position,PadReference(link.target.component,'G'):board.vias[1].position}
    pending=((link,'early source launch unavailable'),)
    incumbent=FanoutResult(board,MappingProxyType(ports),(link.capacitor,link.return_pad),len(board.tracks),len(board.vias),
        created_tracks=board.tracks,created_vias=board.vias,decoupling_pending=((link.capacitor,pending[0][1]),),decoupling_pending_links=pending)
    return source,link,incumbent,pending


@pytest.mark.parametrize('renamed',(False,True))
def test_normal_fanout_early_failure_then_real_ordinary_launches_then_late_success(monkeypatch,renamed):
    source,link=fixture(renamed=renamed)
    chip=source.footprints['chip'];chip=replace(chip,pads=(*chip.pads,FootprintPad('NC',Point.mm(-1,0),Size.mm(.4,.4))))
    source=replace(source,footprints={**source.footprints,'chip':chip},net_routing_rules=tuple(replace(r,max_vias=1) for r in source.net_routing_rules))
    # Isolate the early family's unavailable cheap source proposals. Ordinary
    # fanout uses its untouched generator, actual clearance and materialization.
    monkeypatch.setattr(paths,'_legal_choices',lambda *a,**k:())
    original=decoupling.route_decouplers;calls=[]
    def observe(board,**kwargs):
        answer=original(board,**kwargs);calls.append((board,kwargs,answer));return answer
    monkeypatch.setattr(decoupling,'route_decouplers',observe)
    result=route_fanout(source,FanoutOptions(minimum_component_pads=3,maximum_neighbor_distance_nm=3000000,joint_escapes=False))
    assert len(calls)==2 and calls[0][2].pending and not calls[0][2].created_tracks
    assert calls[1][1]['only_links']==frozenset({link})
    assert calls[1][0].tracks and calls[1][0].vias  # actual accepted ordinary frame
    assert result.late_bypass.passes==1 and result.late_bypass.status=='completed'
    assert result.late_bypass.completed_links==(link,) and not result.decoupling_pending_links and not result.decoupling_pending
    assert result.decoupling_witnesses and paths.verify_crosslayer_decoupling(result.board,link).witness
    assert not result.decoupling_vias and result.decoupling_tracks
    assert all(t.layer is CopperLayer.BACK for t in result.decoupling_tracks)
    ordinary=calls[1][0];assert result.board.tracks[:len(ordinary.tracks)]==ordinary.tracks and result.board.vias==ordinary.vias
    assert all(t not in result.decoupling_tracks for t in ordinary.tracks)
    assert result.created_tracks==result.board.tracks and result.created_vias==result.board.vias
    assert result.added_track_count==len(result.created_tracks) and result.added_via_count==len(result.created_vias)
    assert verified_bypass_reservations(source,result)[2] is None
    assert json.loads(json.dumps(asdict(result.late_bypass)))['passes']==1


def test_real_completion_preserves_surface_handoff_and_ordinary_occurrence_owners():
    source,link,incumbent,pending=incumbent_scene()
    path=incumbent.board.tracks[:3]
    surface=RoutingAccess(path[-1].end,CopperLayer.FRONT,path[0].start,path)
    incumbent=replace(incumbent,accesses=MappingProxyType({p:a for p,a in incumbent.accesses.items() if p!=link.target}),
        surface_accesses=MappingProxyType({link.target:surface}))
    result=complete_late_bypasses(incumbent,pending,FanoutOptions())
    assert result.late_bypass.status=='completed'
    assert result.surface_accesses is incumbent.surface_accesses and result.surface_accesses[link.target] is surface
    assert link.target not in result.accesses
    assert all(a is b for a,b in zip(result.board.tracks,incumbent.board.tracks))
    assert not result.decoupling_vias and all(t not in incumbent.board.tracks for t in result.decoupling_tracks)
    assert verified_bypass_reservations(source,result)[2] is None


@pytest.mark.parametrize('fault',('source_change','reordered_prefix','equal_copy_prefix','promote_ordinary','bad_new_port','obstruct_held_path','budget'))
def test_rejected_late_candidate_rolls_back_every_incumbent_field(monkeypatch,fault):
    source,link,incumbent,pending=incumbent_scene()
    real=decoupling.route_decouplers(incumbent.board,only_links=frozenset({link}))
    assert real.created_tracks
    if fault=='source_change':bad=replace(real,board=replace(real.board,metadata={**real.board.metadata,'changed':'yes'}))
    elif fault=='reordered_prefix':bad=replace(real,board=replace(real.board,tracks=(real.board.tracks[1],real.board.tracks[0],*real.board.tracks[2:])))
    elif fault=='equal_copy_prefix':bad=replace(real,board=replace(real.board,tracks=(replace(real.board.tracks[0]),*real.board.tracks[1:])))
    elif fault=='promote_ordinary':bad=replace(real,created_tracks=(incumbent.board.tracks[0],*real.created_tracks))
    elif fault=='bad_new_port':bad=replace(real,accesses={**real.accesses,link.capacitor:Point.mm(24,19)})
    elif fault=='obstruct_held_path':
        t=TrackSegment(link.return_net,Point.mm(9.5,7),Point.mm(9.5,8.5),150000,CopperLayer.FRONT)
        bad=replace(real,board=replace(real.board,tracks=(*real.board.tracks,t)),created_tracks=(*real.created_tracks,t))
    else:
        incumbent=replace(incumbent,board=replace(incumbent.board,net_routing_rules=tuple(replace(r,max_length_nm=3000000) if r.net==link.net else r for r in incumbent.board.net_routing_rules)))
        bad=replace(real,board=replace(real.board,net_routing_rules=incumbent.board.net_routing_rules))
    calls=[]
    def proposal(*a,**k):calls.append(k);return bad
    monkeypatch.setattr(decoupling,'route_decouplers',proposal)
    result=complete_late_bypasses(incumbent,pending,FanoutOptions())
    assert len(calls)==1 and result.late_bypass.status=='rejected' and result.late_bypass.passes==1
    assert replace(result,late_bypass=incumbent.late_bypass)==incumbent
    assert result.board is incumbent.board and result.created_tracks is incumbent.created_tracks


def test_only_nets_intersection_and_same_face_do_not_start_a_late_pass(monkeypatch):
    _,link,incumbent,pending=incumbent_scene()
    def forbidden(*a,**k):raise AssertionError('no selected pending crosslayer obligations')
    monkeypatch.setattr(decoupling,'route_decouplers',forbidden)
    result=complete_late_bypasses(incumbent,pending,FanoutOptions(),only_nets=frozenset({link.return_net}))
    assert result.late_bypass.passes==0 and result.late_bypass.status=='not_needed'
    assert result.decoupling_pending_links==pending and result.board is incumbent.board
    poses=tuple(replace(p,side=incumbent.board.placements[0].side) if p.reference==link.capacitor.component else p for p in incumbent.board.placements)
    same=replace(incumbent,board=replace(incumbent.board,placements=poses))
    assert complete_late_bypasses(same,pending,FanoutOptions()).late_bypass.passes==0


def test_shared_capacitor_source_is_rejected_and_pending_ids_are_not_guessed():
    source,link,incumbent,pending=incumbent_scene()
    extra=PadReference(link.target.component,'FAR')
    other=replace(link,target=extra)
    board=incumbent.board;chip=board.footprints['chip'];chip=replace(chip,pads=(*chip.pads,FootprintPad('FAR',Point.mm(14,7),Size.mm(.4,.4))))
    with pytest.raises(ValueError,match='one explicit protected pin association'):
        replace(board,footprints={**board.footprints,'chip':chip},nets=tuple(replace(n,pads=(*n.pads,extra)) if n.name==link.net else n for n in board.nets),decoupling_links=(link,other))
    with pytest.raises(ValueError,match='undeclared original obligation'):
        complete_late_bypasses(incumbent,((other,'forged same capacitor different target'),),FanoutOptions())
    assert incumbent.decoupling_pending_links==pending and incumbent.board is board


def test_pending_pad_resolution_does_not_invent_a_domain_candidate_index():
    _,link,incumbent,pending=incumbent_scene()
    entry=FanoutPinAnalysis(link.capacitor,0,None,'no ordinary candidate')
    incumbent=replace(incumbent,pin_analysis=(entry,))
    result=complete_late_bypasses(incumbent,pending,FanoutOptions())
    assert result.late_bypass.status=='completed' and link.capacitor not in result.pending_pads
    assert result.pin_analysis[0].selected_candidate_index is None
    assert result.pin_analysis[0].legal_candidate_count==0
    assert result.pin_analysis[0].diagnostic=='resolved by verified late bypass port'


@pytest.mark.parametrize('obstruct',(False,True))
def test_old_semantic_witness_paths_survive_or_late_candidate_alone_rolls_back(monkeypatch,obstruct):
    source,link,incumbent,pending=incumbent_scene()
    other,other_link=fixture(renamed=True)
    other=replace(other,placements=tuple(replace(p,position=Point(p.position.x_nm,p.position.y_nm+9000000)) for p in other.placements))
    full=paths.propose_crosslayer_decoupling(other,other_link)
    assert full.witness is not None
    b=incumbent.board
    b=replace(b,placements=(*b.placements,*other.placements),nets=(*b.nets,*other.nets),
        net_routing_rules=(*b.net_routing_rules,*other.net_routing_rules),decoupling_links=(link,other_link),
        tracks=(*b.tracks,*full.board.tracks),vias=(*b.vias,*full.board.vias))
    protected=paths.verify_crosslayer_decoupling(b,other_link).witness
    assert protected is not None
    incumbent=replace(incumbent,board=b,created_tracks=b.tracks,created_vias=b.vias,
        added_track_count=len(b.tracks),added_via_count=len(b.vias),
        decoupling_tracks=full.board.tracks,decoupling_vias=full.board.vias,decoupling_witnesses=(protected,))
    if obstruct:
        real=decoupling.route_decouplers(b,only_links=frozenset({link}))
        track=protected.supply_path.tracks[0]
        middle=Point((track.start.x_nm+track.end.x_nm)//2,(track.start.y_nm+track.end.y_nm)//2)
        cross=TrackSegment(link.return_net,Point(middle.x_nm,middle.y_nm-500000),Point(middle.x_nm,middle.y_nm+500000),150000,track.layer)
        bad=replace(real,board=replace(real.board,tracks=(*real.board.tracks,cross)),created_tracks=(*real.created_tracks,cross))
        monkeypatch.setattr(decoupling,'route_decouplers',lambda *a,**k:bad)
    result=complete_late_bypasses(incumbent,pending,FanoutOptions())
    if obstruct:
        assert result.late_bypass.status=='rejected'
        assert replace(result,late_bypass=None)==incumbent
    else:
        assert result.late_bypass.status=='completed'
        assert paths.validate_crosslayer_witness(result.board,protected)==''
        assert result.decoupling_tracks[:len(full.board.tracks)]==full.board.tracks
        assert all(a is b for a,b in zip(result.board.tracks,b.tracks))
        original=replace(b,tracks=(),vias=())
        assert verified_bypass_reservations(original,result)[2] is None


def test_no_ordinary_progress_does_not_repeat_the_early_bypass_work(monkeypatch):
    source,link=fixture()
    incumbent=FanoutResult(source,MappingProxyType({}),(),0,0,
        decoupling_pending=((link.capacitor,'no source launch'),),decoupling_pending_links=((link,'no source launch'),))
    def forbidden(*a,**k):raise AssertionError('same copper cannot justify another completion pass')
    monkeypatch.setattr(decoupling,'route_decouplers',forbidden)
    result=complete_late_bypasses(incumbent,incumbent.decoupling_pending_links,FanoutOptions())
    assert result.late_bypass.passes==0 and result.late_bypass.reason=='no_new_ordinary_copper'
    assert result.decoupling_pending_links==incumbent.decoupling_pending_links


def test_compact_report_exposes_late_scope_without_claiming_routing_closure():
    from pcbir.engineering_report import summarize_report,summary_lines
    _,link,incumbent,pending=incumbent_scene()
    result=complete_late_bypasses(incumbent,pending,FanoutOptions())
    report={'schema':'copperscript-route-board/v0.1','status':'partial','routing_complete':False,
            'fanout':{'late_bypass_completion':asdict(result.late_bypass)}}
    summary=summarize_report(report)
    assert summary['routing']['complete'] is False
    late=summary['decoupling']['late_completion']
    assert late['passes']==late['maximum_passes']==late['selected']==late['completed']==1
    assert late['remaining']==0 and late['added_tracks']>0
    assert any('Late bypass pass: completed' in line for line in summary_lines(summary))
