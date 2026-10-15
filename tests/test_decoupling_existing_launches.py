"""Exact held source paths and one shared bounded missing-tail allowance."""
from dataclasses import replace
import pytest
import pcbir.decoupling_paths as paths
from pcbir.fanout import FanoutOptions
from pcbir.physical import (CopperLayer,FootprintPad,InternalPadGroup,PadReference,Point,Size,TrackSegment,Via)
from pcbir.routing_clearance import RoutingClearanceIndex
from test_decoupling_crosslayer_paths import fixture

def scene(renamed=False):
    source,link=fixture(renamed=renamed)
    def chain(net,points):
        return tuple(TrackSegment(net,Point.mm(*a),Point.mm(*b),150000,CopperLayer.FRONT)
                     for a,b in zip(points,points[1:]))
    tracks=(*chain(link.net,((9,8),(9.5,8),(9.5,7.5),(11,7.5))),
            *chain(link.return_net,((9,10),(9.5,10),(9.5,10.5),(11,10.5))))
    vias=tuple(Via(n,Point.mm(11,y),450000,200000,CopperLayer.FRONT,CopperLayer.BACK)
               for n,y in ((link.net,7.5),(link.return_net,10.5)))
    return replace(source,tracks=tracks,vias=vias,net_routing_rules=tuple(replace(r,max_vias=1) for r in source.net_routing_rules)),link

@pytest.mark.parametrize('renamed',(False,True))
def test_actual_multileg_sources_reuse_both_launches_without_new_source_copper(monkeypatch,renamed):
    source,link=scene(renamed)
    def no_fresh(*a,**k):raise AssertionError('one verified reuse must spend the sole launch slot first')
    monkeypatch.setattr(paths,'_legal_choices',no_fresh)
    result=paths.propose_crosslayer_decoupling(source,link,maximum_launch_candidates=1)
    assert result.witness is not None,result.reason
    assert result.created_tracks and not result.created_vias
    assert all(t.layer is CopperLayer.BACK for t in result.created_tracks)
    assert result.board.tracks[:len(source.tracks)]==source.tracks and result.board.vias==source.vias
    assert all(a is b for a,b in zip(result.board.tracks,source.tracks))
    assert paths.validate_crosslayer_witness(result.board,result.witness)==''
    for path in (result.witness.supply_path,result.witness.return_path):
        assert len([t for t in path.tracks if t.layer is CopperLayer.FRONT])==3
    # A second observation adopts nothing and preserves original owner edges.
    again=paths.propose_crosslayer_decoupling(result.board,link,protected_witnesses=(result.witness,))
    assert again.witness and again.board is result.board and not again.created_tracks and not again.created_vias

@pytest.mark.parametrize('fault',('missing_source_leg','wrong_terminal','wrong_net','wrong_span','illegal_via','narrow_source','outside_radius'))
def test_only_exact_valid_original_source_launch_can_be_reused(monkeypatch,fault):
    source,link=scene()
    if fault=='missing_source_leg':source=replace(source,tracks=source.tracks[1:])
    elif fault=='wrong_terminal':
        chip=source.footprints['chip'];extra=FootprintPad('OTHER',Point.mm(1,-1),Size.mm(.4,.4))
        chip=replace(chip,pads=(*chip.pads,extra),internal_pad_groups=(InternalPadGroup(('S','OTHER')),))
        first=replace(source.tracks[0],start=Point.mm(9,7))
        source=replace(source,footprints={**source.footprints,'chip':chip},tracks=(first,*source.tracks[1:]),
            nets=tuple(replace(n,pads=(*n.pads,PadReference(link.target.component,'OTHER'))) if n.name==link.net else n for n in source.nets))
    elif fault=='wrong_net':source=replace(source,vias=(replace(source.vias[0],net=link.return_net),source.vias[1]))
    elif fault=='wrong_span':source=replace(source,vias=(replace(source.vias[0],to_layer=CopperLayer.INTERNAL_2),source.vias[1]))
    elif fault=='illegal_via':source=replace(source,vias=(replace(source.vias[0],position=Point.mm(9,8)),source.vias[1]))
    elif fault=='narrow_source':source=replace(source,tracks=(replace(source.tracks[0],width_nm=100000),*source.tracks[1:]))
    options=FanoutOptions(maximum_radius_nm=1000000) if fault=='outside_radius' else FanoutOptions()
    monkeypatch.setattr(paths,'_candidates',lambda *a,**k:iter(()));monkeypatch.setattr(paths,'_two_leg_candidates',lambda *a,**k:iter(()))
    assert not tuple(paths._connections(source,link.target,link.capacitor,link.net,options,1,64))

def test_site_bound_counts_existing_candidates_before_geometry(monkeypatch):
    source,link=scene()
    # A nearer but disconnected same-net via consumes the sole examined slot.
    decoy=replace(source.vias[0],position=Point.mm(11.5,9))
    source=replace(source,vias=(decoy,*source.vias),net_routing_rules=tuple(replace(r,max_vias=2) if r.net==link.net else r for r in source.net_routing_rules))
    monkeypatch.setattr(paths,'_candidates',lambda *a,**k:iter(()));monkeypatch.setattr(paths,'_two_leg_candidates',lambda *a,**k:iter(()))
    options=FanoutOptions()
    assert not tuple(paths._connections(source,link.target,link.capacitor,link.net,options,1,1))
    assert tuple(paths._connections(source,link.target,link.capacitor,link.net,options,1,2))

@pytest.mark.parametrize('maze_enabled',(False,True))
def test_one_four_call_allowance_survives_supply_variants_and_return_targets(monkeypatch,maze_enabled):
    source,link=scene()
    chip=source.footprints['chip'];chip=replace(chip,pads=(*chip.pads,FootprintPad('G2',Point.mm(1,3),Size.mm(.4,.4)),FootprintPad('G3',Point.mm(1,4),Size.mm(.4,.4))))
    more=tuple(TrackSegment(link.return_net,Point.mm(*a),Point.mm(*b),150000,CopperLayer.FRONT)
               for a,b in (((9,11),(9.5,11)),((9.5,11),(9.5,10.5)),((9,12),(9.5,12)),((9.5,12),(9.5,11))))
    source=replace(source,footprints={**source.footprints,'chip':chip},tracks=(*source.tracks,*more),
        nets=tuple(replace(n,pads=(*n.pads,PadReference(link.target.component,'G2'),PadReference(link.target.component,'G3'))) if n.name==link.return_net else n for n in source.nets))
    cheap=paths.checked_access_paths;connect=paths._connections;calls=[];contexts=[]
    def tails(board,index,net,*a,**k):return () if net==link.return_net else cheap(board,index,net,*a,**k)
    def local(*a,**k):calls.append((a,k));return None
    def observed(*a,**k):contexts.append(k['work']);return connect(*a,**k)
    monkeypatch.setattr(paths,'checked_access_paths',tails);monkeypatch.setattr(paths,'local_access_path',local);monkeypatch.setattr(paths,'_connections',observed)
    result=paths.propose_crosslayer_decoupling(source,link,fanout_options=FanoutOptions(maze_escapes=maze_enabled),maximum_launch_candidates=1)
    assert result.board is source and not result.witness and not result.created_tracks and not result.created_vias
    assert len(contexts)>5 and len({id(x) for x in contexts})==1
    assert len(calls)==(4 if maze_enabled else 0)
    assert all(k=={'step_nm':100000,'detour_nm':1000000,'maximum_states':4000} for _,k in calls)
    assert all(a[2]==link.return_net and a[5] is CopperLayer.BACK for a,_ in calls)

def test_unverified_or_new_launch_never_gets_an_added_local_maze(monkeypatch):
    source,link=fixture()
    monkeypatch.setattr(paths,'checked_access_paths',lambda *a,**k:())
    def forbidden(*a,**k):raise AssertionError('new launch must not get a new maze family')
    monkeypatch.setattr(paths,'local_access_path',forbidden)
    assert not tuple(paths._connections(source,link.target,link.capacitor,link.net,FanoutOptions(maze_escapes=True),1,64))


def test_actual_local_tails_are_checked_and_complete_without_new_vias(monkeypatch):
    source,link=scene()
    monkeypatch.setattr(paths,'checked_access_paths',lambda *a,**k:())
    actual=paths.local_access_path;calls=[]
    def local(*a,**k):
        calls.append((a,k));return actual(*a,**k)
    monkeypatch.setattr(paths,'local_access_path',local)
    result=paths.propose_crosslayer_decoupling(source,link,fanout_options=FanoutOptions(maze_escapes=True),maximum_launch_candidates=1)
    assert result.witness is not None,result.reason
    assert len(calls)==2 and all(a[5] is CopperLayer.BACK for a,_ in calls)
    assert result.created_tracks and not result.created_vias
    assert all(t.layer is CopperLayer.BACK for t in result.created_tracks)
    assert paths.validate_crosslayer_witness(result.board,result.witness)==''
    assert result.board.tracks[:len(source.tracks)]==source.tracks and result.board.vias==source.vias


def test_new_tails_preserve_fresh_other_owner_witness_and_every_held_occurrence():
    source,link=scene()
    other,other_link=fixture(renamed=True)
    other=replace(other,placements=tuple(replace(p,position=Point(p.position.x_nm,p.position.y_nm+9000000)) for p in other.placements))
    full=paths.propose_crosslayer_decoupling(other,other_link)
    assert full.witness is not None,full.reason
    combined=replace(source,placements=(*source.placements,*other.placements),nets=(*source.nets,*other.nets),
        tracks=(*source.tracks,*full.board.tracks),vias=(*source.vias,*full.board.vias),
        net_routing_rules=(*source.net_routing_rules,*other.net_routing_rules),decoupling_links=(link,other_link))
    protected=paths.verify_crosslayer_decoupling(combined,other_link).witness
    assert protected is not None
    result=paths.propose_crosslayer_decoupling(combined,link,protected_witnesses=(protected,),maximum_launch_candidates=1)
    assert result.witness is not None,result.reason
    assert result.board.tracks[:len(combined.tracks)]==combined.tracks and result.board.vias==combined.vias
    assert all(a is b for a,b in zip(result.board.tracks,combined.tracks))
    assert paths.validate_crosslayer_witness(result.board,protected)==''
    assert paths.validate_crosslayer_witness(result.board,result.witness)==''
    assert not any(t in combined.tracks for t in result.created_tracks) and not result.created_vias
