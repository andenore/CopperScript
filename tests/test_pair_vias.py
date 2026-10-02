from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, FootprintPad,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
    Placement, Point, PolygonRing, PolygonWithHoles, RouteKind, Size, Stackup,
    nm_from_mm, route_global, route_critical_nets, run_physical_drc)
from pcbir.critical import _route_pair, _validate_candidate
from pcbir.pair_search import PairSearchStats, paired_candidates
from pcbir.pair_vias import paired_via_candidates, _signal_drills_outside_smd
from pcbir.physical import Via, ComponentPlacementRule


def board_fixture(returns=False):
    fp = PhysicalFootprint('pair', tuple(FootprintPad(str(i+1),Point.mm(0,y),Size.mm(.4,.4))
        for i,y in enumerate((-.5,.5))),Size.mm(1,2))
    rules = tuple(NetRoutingRule(name,RouteKind.DIFFERENTIAL,
        differential_partner=partner,width_nm=nm_from_mm(.25),pair_gap_nm=nm_from_mm(.2),
        allowed_layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_2),max_vias=2,
        require_return_vias=returns,return_via_net='GND' if returns else None,
        maximum_return_via_distance_nm=nm_from_mm(1.5) if returns else None)
        for name,partner in (('A','B'),('B','A')))
    wall=CopperKeepout('top-wall',(CopperLayer.FRONT,),PolygonWithHoles(PolygonRing((
        Point.mm(19,0),Point.mm(21,0),Point.mm(21,25),Point.mm(19,25)))))
    return PhysicalBoard('PairVias',BoardOutline.rectangle(40,25),{fp.name:fp},
        (Placement('J1',fp.name,Point.mm(5,12)),Placement('J2',fp.name,Point.mm(35,12))),
        (*tuple(PhysicalNet(name,(PadReference('J1',str(i+1)),PadReference('J2',str(i+1))))
                for i,name in enumerate(('A','B'))),PhysicalNet('GND',())),
        stackup=Stackup(copper_layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_1,
                                      CopperLayer.INTERNAL_2,CopperLayer.BACK)),
        net_routing_rules=rules,copper_keepouts=(wall,))


@pytest.mark.parametrize('returns',(False,True))
def test_terminal_vias_cross_top_wall_jointly_and_deterministically(returns):
    board=board_fixture(returns)
    guides=route_global(board)
    rules=board.net_routing_rules
    routes={r.net:r for r in guides.routes}
    stats=PairSearchStats()
    def propose():
        return next(paired_via_candidates(board,*rules,routes['A'],routes['B'],stats=stats))
    candidate=propose()
    assert candidate==propose()
    assert len(candidate.via_pairs)==2
    assert len(candidate.return_vias)==(2 if returns else 0)
    assert all(t.layer in rules[0].allowed_layers for t in (*candidate.first,*candidate.second))
    assert any(t.layer is CopperLayer.INTERNAL_2 for t in candidate.first)
    assert any(t.layer is CopperLayer.FRONT for t in candidate.first)
    assert all(v.from_layer is CopperLayer.FRONT and v.to_layer is CopperLayer.BACK
               for pair in candidate.via_pairs for v in pair)
    proposed,tracks,vias=_route_pair(board,*rules,routes,
        exact_tracks=(candidate.first,candidate.second),exact_via_pairs=candidate.via_pairs,
        exact_return_vias=candidate.return_vias)
    accepted,tracks,vias=_validate_candidate(board,proposed,tracks,vias,[],[])
    assert accepted.connected and accepted.paired_via_transitions==2
    assert accepted.strategy=='joint_pair_via_search'
    assert accepted.via_count==(6 if returns else 4)
    routed=replace(board,tracks=tracks,vias=vias)
    assert not [f for f in run_physical_drc(routed).findings
                if f.severity.value=='error' and f.code!='DRC-ROUTE-INCOMPLETE']
    assert board.tracks==board.vias==()


@pytest.mark.parametrize('failure',('budget','surface_only','all_layer_wall','return_distance'))
def test_paired_via_search_fails_closed(failure):
    board=board_fixture(returns=failure=='return_distance')
    if failure=='budget':
        board=replace(board,net_routing_rules=tuple(replace(r,max_vias=1) for r in board.net_routing_rules))
    elif failure=='surface_only':
        board=replace(board,net_routing_rules=tuple(replace(r,allowed_layers=(CopperLayer.FRONT,))
            for r in board.net_routing_rules))
    elif failure=='all_layer_wall':
        board=replace(board,copper_keepouts=tuple(replace(k,layers=board.stackup.copper_layers)
            for k in board.copper_keepouts))
    else:
        board=replace(board,net_routing_rules=tuple(replace(r,maximum_return_via_distance_nm=nm_from_mm(.1))
            for r in board.net_routing_rules))
    routes={r.net:r for r in route_global(board).routes}
    assert not list(paired_via_candidates(board,*board.net_routing_rules,routes['A'],routes['B'],
                                         maximum_searches=2,maximum_states=100))
    assert board.tracks==board.vias==()


def test_critical_owner_accepts_via_pair_without_surface_refinement(monkeypatch):
    # The surface engine has its own wall/clearance tests. Isolate the owner
    # integration rather than exhaust three surface maze budgets here.
    monkeypatch.setattr('pcbir.critical.paired_candidates',lambda *a,**k:iter(()))
    board=board_fixture(True)
    result=route_critical_nets(board,route_global(board))
    pair=result.nets[0]
    assert pair.connected and pair.strategy=='joint_pair_via_search'
    assert pair.paired_via_transitions==2 and pair.return_via_count==2
    assert pair.pair_refinement_attempts==0


@pytest.mark.parametrize('distance,legal',((0,False),(.35,False),(.5,True)))
def test_signal_drills_cannot_overlap_their_own_smd_land(distance,legal):
    board=board_fixture()
    pair=tuple(Via(net,Point.mm(5+distance,y),nm_from_mm(.8),nm_from_mm(.4))
               for net,y in (('A',11.5),('B',12.5)))
    assert _signal_drills_outside_smd(board,pair) is legal


def test_diagonal_terminal_rows_keep_matched_transition_geometry():
    board=board_fixture()
    board=replace(board,placements=tuple(replace(p,rotation_degrees=45) for p in board.placements),
        placement_rules=tuple(ComponentPlacementRule(p.reference,allowed_orientations=(45,))
                              for p in board.placements))
    routes={r.net:r for r in route_global(board).routes}
    candidate=next(paired_via_candidates(board,*board.net_routing_rules,routes['A'],routes['B']))
    proposed,tracks,vias=_route_pair(board,*board.net_routing_rules,routes,
        exact_tracks=(candidate.first,candidate.second),exact_via_pairs=candidate.via_pairs)
    accepted,_,_=_validate_candidate(board,proposed,tracks,vias,[],[])
    assert accepted.connected and accepted.paired_via_transitions==2


def test_surface_alternative_uses_actual_pad_layer_not_tentative_guide_access():
    board=replace(board_fixture(),copper_keepouts=())
    routes={r.net:replace(r,accesses=tuple(replace(a,layer=CopperLayer.INTERNAL_2)
                                         for a in r.accesses)) for r in route_global(board).routes}
    candidate=next(paired_candidates(board,*board.net_routing_rules,routes['A'],routes['B']))
    assert all(t.layer is CopperLayer.FRONT for t in (*candidate.first,*candidate.second))
    proposed,tracks,vias=_route_pair(board,*board.net_routing_rules,routes,
                                   exact_tracks=(candidate.first,candidate.second))
    accepted,_,_=_validate_candidate(board,proposed,tracks,vias,[],[])
    assert accepted.connected and accepted.via_count==0


@pytest.mark.parametrize('fault',('missing_return','asymmetric_span','floating_track','clearance','budget'))
def test_critical_owner_rejects_corrupted_paired_transition_atomically(fault):
    board=board_fixture(True)
    routes={r.net:r for r in route_global(board).routes}
    candidate=next(paired_via_candidates(board,*board.net_routing_rules,routes['A'],routes['B']))
    pairs=candidate.via_pairs
    returns=candidate.return_vias
    tracks=(candidate.first,candidate.second)
    if fault=='missing_return':
        returns=()
    elif fault=='asymmetric_span':
        pairs=((pairs[0][0],replace(pairs[0][1],to_layer=CopperLayer.INTERNAL_2)),pairs[1])
    elif fault=='floating_track':
        tracks=(tuple(t for t in tracks[0] if t.layer is not CopperLayer.INTERNAL_2),tracks[1])
    elif fault=='clearance':
        pairs=((pairs[0][0],replace(pairs[0][1],position=pairs[0][0].position)),pairs[1])
    else:
        board=replace(board,net_routing_rules=tuple(replace(r,max_vias=0) for r in board.net_routing_rules))
    proposed,created,vias=_route_pair(board,*board.net_routing_rules,routes,exact_tracks=tracks,
                                    exact_via_pairs=pairs,exact_return_vias=returns)
    rejected,created,vias=_validate_candidate(board,proposed,created,vias,[],[])
    assert not rejected.connected and rejected.candidate_rejected
    assert created==vias==()
