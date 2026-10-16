"""Real original-edge retention across disposable critical/plane probes."""
from dataclasses import replace
import pytest
from pcbir.decoupling import route_decouplers
from pcbir.fanout import FanoutOptions,FanoutResult
from pcbir.physical import PhysicalNet,NetRoutingRule,RouteKind,TrackSegment,Point,CopperLayer
from pcbir.priority_bypass import priority_bypass_reservations
from pcbir.package_access import _full_ordinary_pattern
from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.decoupling_paths import validate_crosslayer_witness
from test_decoupling_crosslayer_paths import fixture

def scene(renamed=False,priority=40):
    b,l=fixture(renamed=renamed)
    b=replace(b,nets=(*b.nets,PhysicalNet('CLOCK',())),net_routing_rules=(
        *tuple(replace(r,priority=priority) if r.net==l.net else r for r in b.net_routing_rules),
        NetRoutingRule('CLOCK',kind=RouteKind.CLOCK,priority=30,width_nm=150000)))
    r=route_decouplers(b)
    assert not r.pending
    original=FanoutResult(r.board,{},(),len(r.created_tracks),len(r.created_vias),
        created_tracks=r.created_tracks,created_vias=r.created_vias,
        decoupling_tracks=r.created_tracks,decoupling_vias=r.created_vias,
        decoupling_witnesses=r.admitted_path_witnesses)
    obstacle=TrackSegment('CLOCK',Point.mm(20,15),Point.mm(22,15),150000,CopperLayer.FRONT)
    return b,l,original,obstacle

@pytest.mark.parametrize('renamed',[False,True])
def test_full_probe_retains_only_exact_high_priority_bypass_edges(renamed):
    b,l,original,obstacle=scene(renamed)
    held=priority_bypass_reservations(b,original);assert held is not None
    assert verified_bypass_reservations(b,held)[2] is None
    frame=replace(held.board,tracks=(*held.board.tracks,obstacle),metadata={'critical_routing':'partial'})
    candidate,reason=_full_ordinary_pattern(b,frame,FanoutOptions(),retained=held)
    assert candidate is not None,reason
    assert obstacle not in candidate.board.tracks
    assert all(any(t is old for t in candidate.created_tracks) for old in held.created_tracks)
    assert all(any(v is old for v in candidate.created_vias) for old in held.created_vias)
    assert verified_bypass_reservations(b,candidate)[2] is None
    assert all(validate_crosslayer_witness(candidate.board,w)=='' for w in held.decoupling_witnesses)
    assert original.board.tracks==held.board.tracks

def test_equal_or_lower_priority_has_no_special_reservation():
    for priority in (0,20,30):
        source,l,original,_=scene(priority=priority)
        assert priority_bypass_reservations(source,original) is None

def test_stale_source_and_missing_retained_occurrence_reject():
    b,l,original,obstacle=scene();held=priority_bypass_reservations(b,original)
    changed=replace(b,placements=(replace(b.placements[0],position=Point.mm(7,8)),b.placements[1]))
    assert priority_bypass_reservations(changed,original) is None
    frame=replace(held.board,tracks=held.board.tracks[1:])
    before=held.board
    candidate,reason=_full_ordinary_pattern(b,frame,FanoutOptions(),retained=held)
    assert candidate is None and reason=='lost_priority_bypass_ownership'
    assert held.board is before

def test_unowned_disposable_return_is_still_removed_and_rejected():
    b,l,original,obstacle=scene()
    candidate,reason=_full_ordinary_pattern(b,original.board,FanoutOptions())
    assert candidate is None and reason in {'stale_bypass_witness','lost_bypass_connection'}
