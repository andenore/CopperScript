"""Matched crossover branches preserve every duplicated contact atomically."""
from dataclasses import replace
import pytest
from pcbir.physical import (BoardOutline,DesignRules,FootprintPad,PadReference,
    PhysicalBoard,PhysicalFootprint,PhysicalNet,Placement,Point,Size,CopperLayer,
    NetRoutingRule,RouteKind)
from pcbir.pair_terminals import prepare_pair_terminals
from pcbir.drc import explicit_copper_connectivity,run_physical_drc,PhysicalDrcPolicy

def fixture():
    lands=[FootprintPad('A',Point.mm(0,-.25),Size.mm(.7,.3)),
           FootprintPad('B',Point.mm(0,.25),Size.mm(.7,.3)),
           FootprintPad('C',Point.mm(1.7,.5),Size.mm(.7,.3)),
           FootprintPad('D',Point.mm(1.7,0),Size.mm(.7,.3))]
    for i in range(-6,7):
        for column,y in ((0,i*.5+.25),(1.7,i*.5)):
            if (column==0 and y in (-.25,.25)) or (column==1.7 and y in (0,.5)):
                continue
            lands.append(FootprintPad('unused'+str(len(lands)),Point.mm(str(column),str(y)),Size.mm(.7,.3)))
    chip=PhysicalFootprint('chip',(FootprintPad('1',Point.mm(0,-.5),Size.mm(.4,.4)),
        FootprintPad('2',Point.mm(0,.5),Size.mm(.4,.4))),Size.mm(1,2))
    connector=PhysicalFootprint('contacts',tuple(replace(p,position=Point(p.position.x_nm-5000000,p.position.y_nm)) for p in lands),Size.mm(10,8))
    rules=tuple(NetRoutingRule(name,RouteKind.DIFFERENTIAL,differential_partner=partner,
        width_nm=250000,pair_gap_nm=150000,allowed_layers=(CopperLayer.FRONT,CopperLayer.BACK),
        max_vias=6,breakout_length_nm=2000000,breakout_width_nm=120000,
        breakout_gap_nm=100000,breakout_clearance_nm=100000,
        tuning_amplitude_limit_nm=1000000,max_skew_nm=500000)
        for name,partner in (('N','P'),('P','N')))
    return PhysicalBoard('crossed contacts',BoardOutline.rectangle(40,25),
        {'chip':chip,'contacts':connector},(Placement('U','chip',Point.mm(5,12)),
        Placement('J','contacts',Point.mm(30,12))),
        (PhysicalNet('N',(PadReference('U','1'),PadReference('J','A'),PadReference('J','C'))),
         PhysicalNet('P',(PadReference('U','2'),PadReference('J','B'),PadReference('J','D')))),
        rules=DesignRules(minimum_clearance_nm=100000,minimum_track_width_nm=100000,
            default_via_size_nm=450000,default_via_drill_nm=200000,minimum_hole_clearance_nm=200000),
        net_routing_rules=rules)

def test_local_matched_crossover_joins_all_four_real_lands_without_source_mutation():
    board=fixture()
    routes,a,b,pairs,returns=prepare_pair_terminals(board,*board.net_routing_rules,{})
    assert routes and len(pairs)==2
    assert all(len(g.accesses)==2 and not g.connected for g in routes.values())
    local=replace(board,tracks=(*a,*b),vias=(*[v for pair in pairs for v in pair],*returns))
    graph=explicit_copper_connectivity(local)
    assert graph.net_connected(PhysicalNet('N',(PadReference('J','A'),PadReference('J','C'))))
    assert graph.net_connected(PhysicalNet('P',(PadReference('J','B'),PadReference('J','D'))))
    assert not [f for f in run_physical_drc(local,policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings
        if f.severity.value=='error' and f.code!='DRC-OPEN-NET']
    assert board.tracks==board.vias==()

def test_rejected_main_pair_keeps_all_local_crossover_copper_disposable():
    from pcbir import route_global,route_critical_nets
    board=fixture()
    # Main routing's strict via budget includes the local branch portals.
    board=replace(board,net_routing_rules=tuple(replace(r,max_vias=0) for r in board.net_routing_rules))
    guides=route_global(board)
    result=route_critical_nets(board,guides,pair_state_limit=1)
    assert not any(n.connected for n in result.nets)
    assert result.board.tracks==board.tracks and result.board.vias==board.vias


@pytest.mark.parametrize('remote', (False, True))
def test_crossover_with_support_keeps_every_contact_and_rejects_remote_branch(remote):
    board=fixture()
    support=PhysicalFootprint('support',(
        FootprintPad('1',Point.mm(0,-.25),Size.mm(.4,.3)),
        FootprintPad('2',Point.mm(0,.25),Size.mm(.4,.3))),Size.mm(1,1))
    board=replace(board,footprints={**board.footprints,'support':support},
        placements=(*board.placements,Placement('ESD','support',Point.mm(18 if remote else 23,12))),
        nets=tuple(replace(n,pads=(*n.pads,PadReference('ESD',str(i)))) for i,n in enumerate(board.nets,1)),
        net_routing_rules=tuple(replace(r,breakout_length_nm=4500000) for r in board.net_routing_rules))
    routes,a,b,pairs,returns=prepare_pair_terminals(board,*board.net_routing_rules,{})
    if remote:
        assert not a and not b and not pairs
    else:
        assert routes and len(pairs)==2
        assert all(len(g.accesses)==2 for g in routes.values())
        local=replace(board,tracks=(*a,*b),vias=(*[v for pair in pairs for v in pair],*returns))
        graph=explicit_copper_connectivity(local)
        for net in board.nets:
            assert graph.net_connected(replace(net,pads=tuple(p for p in net.pads if p.component!='U')))
        assert not [f for f in run_physical_drc(local,policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings
            if f.severity.value=='error' and f.code!='DRC-OPEN-NET']
    assert board.tracks==board.vias==()


def test_complete_pair_accepts_matched_branch_and_preserves_all_source_contacts():
    from pcbir import route_global,route_critical_nets
    board=fixture()
    result=route_critical_nets(board,route_global(board))
    assert all(n.connected for n in result.nets),result.nets
    graph=explicit_copper_connectivity(result.board)
    assert all(graph.net_connected(n) for n in board.nets)
    assert not [f for f in run_physical_drc(result.board,policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings if f.severity.value=='error']
    assert board.tracks==board.vias==()
