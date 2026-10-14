from dataclasses import dataclass, replace
from types import SimpleNamespace

import pytest

from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.fanout import FanoutResult
from pcbir.physical import (ComponentPlacementRule, CopperLayer, DecouplingLink,
    NetRoutingRule, PadReference, PhysicalNet, Placement, Point, RouteKind,
    TrackSegment, Via)
from test_routing_quality import bypass_board


def fixture(renamed=False):
    board=bypass_board()
    power,ground=('SUPPLY_ALPHA','RETURN_BETA') if renamed else ('P','G')
    board=replace(board,
        placements=(board.placements[0],replace(board.placements[1],position=Point.mm(11,8))),
        nets=tuple(replace(n,name=power if n.name=='P' else ground) for n in board.nets),
        net_routing_rules=(NetRoutingRule(power,RouteKind.GENERAL,width_nm=250000),),
        placement_rules=(ComponentPlacementRule('U',fixed_position=Point.mm(8,8)),
                         ComponentPlacementRule('C',fixed_position=Point.mm(11,8))),
        decoupling_links=tuple(replace(link,net=power,return_net=ground) for link in board.decoupling_links))
    feed=TrackSegment(power,Point.mm(9,8),Point.mm(10.5,8),250000,CopperLayer.FRONT)
    result=reserve(board,(feed,),feeds=(feed,))
    return board,result,power,ground


def reserve(source,tracks=(),vias=(),feeds=()):
    return FanoutResult(replace(source,tracks=(*source.tracks,*tracks),vias=(*source.vias,*vias)),
        {},(),len(tracks),len(vias),created_tracks=tracks,created_vias=vias,decoupling_tracks=feeds)


@pytest.mark.parametrize('renamed',(False,True))
def test_fresh_witness_admission_never_routes_or_mutates_inputs(monkeypatch,renamed):
    source,reserved,_,_=fixture(renamed)
    before=reserved.board
    def forbidden(*args,**kwargs):raise AssertionError('admission must not generate copper')
    monkeypatch.setattr('pcbir.decoupling.route_decouplers',forbidden)
    monkeypatch.setattr('pcbir.fanout.route_fanout',forbidden)
    monkeypatch.setattr('pcbir.surface_path.surface_path_between',forbidden)
    first=verified_bypass_reservations(source,reserved)
    assert first==(reserved.decoupling_tracks,(),None)
    assert verified_bypass_reservations(source,reserved)==first
    assert all(a is b for a,b in zip(first[0],reserved.decoupling_tracks))
    assert reserved.board==before and not source.tracks and not source.vias


@pytest.mark.parametrize('fault',('pose','rules','prefix','count','via_count','undeclared_owner'))
def test_stale_source_counter_and_owner_claims_are_rejected(fault):
    source,reserved,power,ground=fixture()
    if fault=='pose':
        reserved=replace(reserved,board=replace(reserved.board,placements=(
            reserved.board.placements[0],replace(reserved.board.placements[1],position=Point.mm(12,8)))))
    elif fault=='rules':
        reserved=replace(reserved,board=replace(reserved.board,net_routing_rules=()))
    elif fault=='prefix':
        reserved=replace(reserved,board=replace(reserved.board,tracks=()))
    elif fault=='count':reserved=replace(reserved,added_track_count=0)
    elif fault=='via_count':reserved=replace(reserved,added_via_count=1)
    else:reserved=replace(reserved,decoupling_tracks=(replace(reserved.decoupling_tracks[0],width_nm=100000),))
    assert verified_bypass_reservations(source,reserved)[2]


@pytest.mark.parametrize('fault',('floating','extra_stub','return_net','critical','no_association','flag'))
def test_arbitrary_or_forged_semantic_feed_cannot_get_exemption(fault):
    source,reserved,power,ground=fixture()
    feed=reserved.decoupling_tracks[0]
    if fault in ('floating','extra_stub'):
        extra=TrackSegment(power,Point.mm(9,8) if fault=='extra_stub' else Point.mm(20,20),
                           Point.mm(9,7) if fault=='extra_stub' else Point.mm(21,20),250000,CopperLayer.FRONT)
        reserved=reserve(source,(feed,extra),feeds=(feed,extra))
    elif fault=='return_net':
        forged=replace(feed,net=ground)
        reserved=reserve(source,(forged,),feeds=(forged,))
    elif fault=='critical':
        source=replace(source,net_routing_rules=(replace(source.net_routing_rules[0],kind=RouteKind.CRITICAL),))
        reserved=reserve(source,(feed,),feeds=(feed,))
    elif fault=='no_association':
        source=replace(source,decoupling_links=())
        reserved=reserve(source,(feed,),feeds=(feed,))
    else:
        source=replace(source,metadata={**source.metadata,'decoupling_complete':'true'})
        forged=replace(feed,start=Point.mm(20,20),end=Point.mm(21,20))
        reserved=reserve(source,(forged,),feeds=(forged,))
    tracks,vias,reason=verified_bypass_reservations(source,reserved)
    assert tracks==vias==() and reason


@pytest.mark.parametrize('fault',('source_claim','duplicate_generated','duplicate_claim'))
def test_source_and_ambiguous_occurrences_never_become_generated_ownership(fault):
    source,reserved,_,_=fixture()
    feed=reserved.decoupling_tracks[0]
    if fault=='source_claim':
        source=replace(source,tracks=(feed,))
        reserved=reserve(source,(feed,),feeds=(feed,))
    elif fault=='duplicate_generated':reserved=reserve(source,(feed,feed),feeds=(feed,))
    else:reserved=reserve(source,(feed,),feeds=(feed,feed))
    assert verified_bypass_reservations(source,reserved)[2]


def test_new_ordinary_copper_cannot_obstruct_verified_source_sibling():
    source,reserved,power,ground=fixture()
    original=reserved.decoupling_tracks[0]
    other=DecouplingLink(PadReference('C2','1'),PadReference('U2','1'),PadReference('C2','2'),'SECOND',ground)
    source=replace(source,tracks=(original,),
        placements=(*source.placements,Placement('U2','chip',Point.mm(8,16)),Placement('C2','cap',Point.mm(11,16))),
        nets=(*tuple(replace(n,pads=(*n.pads,other.return_pad)) if n.name==ground else n for n in source.nets),
              PhysicalNet('SECOND',(other.capacitor,other.target))),
        decoupling_links=(*source.decoupling_links,other))
    new_feed=TrackSegment('SECOND',Point.mm(9,16),Point.mm(10.5,16),250000,CopperLayer.FRONT)
    blocker=TrackSegment(ground,Point.mm(9.75,7.5),Point.mm(9.75,8.5),250000,CopperLayer.FRONT)
    candidate=reserve(source,(new_feed,blocker),feeds=(new_feed,))
    before=candidate.board
    tracks,vias,reason=verified_bypass_reservations(source,candidate)
    assert tracks==vias==() and 'verified source bypass' in reason
    assert candidate.board==before and source.tracks==(original,)


def test_future_ground_via_metadata_does_not_grant_semantic_authority():
    @dataclass(frozen=True,slots=True)
    class FutureFanout(FanoutResult):
        decoupling_vias:tuple=()
    source,reserved,_,ground=fixture()
    via=Via(ground,Point.mm(20,20),600000,300000,CopperLayer.FRONT,CopperLayer.BACK)
    base=reserve(source,reserved.created_tracks,(via,),reserved.decoupling_tracks)
    values={name:getattr(base,name) for name in base.__dataclass_fields__};values['decoupling_vias']=(via,)
    future=FutureFanout(**values)
    tracks,vias,reason=verified_bypass_reservations(source,future)
    assert tracks==vias==() and 'not supported' in reason


def test_empty_semantic_declaration_does_not_infer_an_exemption_from_copper():
    source,reserved,_,_=fixture()
    undeclared=replace(reserved,decoupling_tracks=())
    assert verified_bypass_reservations(source,undeclared)==((),(),None)
    assert verified_bypass_reservations(source,SimpleNamespace(board=reserved.board))[2]


def test_materialized_macro_and_required_array_cannot_be_claimed_or_removed(tmp_path):
    from test_autonomous_plane_access import constrained_contact_board,with_test_owners
    from pcbir.hard_macros import materialize_hard_macros
    from pcbir.pad_via_arrays import require_via_in_pad_arrays
    source=materialize_hard_macros(with_test_owners(constrained_contact_board(),tmp_path,required_array=True))
    require_via_in_pad_arrays(source,"read-only admission fixture")
    assert source.tracks and len(source.vias)==5 and source.hard_macros and source.via_in_pad_rules
    reserved=reserve(source)
    assert verified_bypass_reservations(source,reserved)==((),(),None)
    claimed=replace(reserved,decoupling_tracks=(source.tracks[0],))
    assert 'immutable' in verified_bypass_reservations(source,claimed)[2]
    lost=replace(reserved,board=replace(reserved.board,via_in_pad_rules=()))
    assert verified_bypass_reservations(source,lost)[2]
    assert reserved.board==source and len(source.vias)==5


def test_unclaimed_critical_copper_cannot_hide_beside_a_valid_semantic_feed():
    source,reserved,_,_=fixture()
    source=replace(source,nets=(*source.nets,PhysicalNet("CLOCK",())),
        net_routing_rules=(*source.net_routing_rules,NetRoutingRule("CLOCK",RouteKind.CRITICAL)))
    extra=TrackSegment("CLOCK",Point.mm(20,20),Point.mm(21,20),250000,CopperLayer.FRONT)
    forged=reserve(source,(*reserved.created_tracks,extra),feeds=reserved.decoupling_tracks)
    tracks,vias,reason=verified_bypass_reservations(source,forged)
    assert tracks==vias==() and 'critical-kind' in reason

