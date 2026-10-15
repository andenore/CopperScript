"""Finite terminal domains must expose already checked alternative holes."""
from collections import defaultdict
from dataclasses import replace
import pytest
from pcbir import (BoardOutline, BoardSide, CopperLayer, FootprintPad,
    GlobalRouterOptions, NetRoutingRule, PadReference, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, RouteKind, Size,
    Stackup, TrackSegment, Via)
from pcbir import routing

F,I,J,B=(CopperLayer.FRONT,CopperLayer.INTERNAL_1,CopperLayer.INTERNAL_2,CopperLayer.BACK)

def scene(renamed=False):
    net='ANOTHER' if renamed else 'SIGNAL'
    rootpad=PadReference('A_IN','1');target=PadReference('Z_OUT','1')
    fp=PhysicalFootprint('land',(FootprintPad('1',Point(0,0),Size.mm('.3','.3')),),Size.mm('.5','.5'))
    layers=(F,I,J,B)
    board=PhysicalBoard('diverse-launches',BoardOutline.rectangle(6,4),{fp.name:fp},
        (Placement('A_IN',fp.name,Point.mm(1,2),side=BoardSide.BACK),Placement('Z_OUT',fp.name,Point.mm(2,2))),
        (PhysicalNet(net,(rootpad,target)),),stackup=Stackup(layers),
        net_routing_rules=(NetRoutingRule(net,RouteKind.GENERAL,allowed_layers=layers),))
    ns={(l,x):routing.GridNode(l,x,0) for l in range(4) for x in range(3)}
    resources={};sites={}
    for l in range(1,4):
        for x in range(2):
            a,b=ns[l,x],ns[l,x+1]
            resources[routing._edge_key(a,b)]=routing._Resource(f'run-{l}-{x}',a,b,10,layers[l])
    for x in (1,2):
        identifier=f'via:0:3:{x}:0';sites[identifier]=(Point.mm(x+1,2),)
        for l in range(4):
            for k in range(l+1,4):
                a,b=ns[l,x],ns[k,x]
                resources[routing._edge_key(a,b)]=routing._Resource(identifier,a,b,1,None)
    graph=routing._Graph(layers,(1_000_000,2_000_000,3_000_000),(2_000_000,),frozenset(ns.values()),resources,sites,
        {layer:0 for layer in layers},{layer:'horizontal' for layer in layers})
    root=routing.PinAccess(rootpad,Point.mm(1,2),ns[3,0],Point.mm(1,2),B)
    pool=[]
    for x in (1,2):
        via=Via(net,Point.mm(x+1,2),450_000,200_000,F,B)
        for l in range(1,4):
            tracks=() if x==1 else (TrackSegment(net,Point.mm(2,2),Point.mm(3,2),200_000,F),)
            pool.append(routing.PinAccess(target,Point.mm(2,2),ns[l,x],Point.mm(x+1,2),layers[l],tracks,via))
    return board,graph,root,tuple(pool)

def route(board,graph,root,choices,*,max_vias=None):
    rule=replace(board.net_routing_rules[0],max_vias=max_vias)
    options=GlobalRouterOptions(pin_access_candidates=4)
    return routing._route_net(board,graph,board.nets[0].name,board.nets[0].pads,rule,
        {(rule.net,root.pad):(root,),(rule.net,choices[0].pad):choices},1,
        defaultdict(int,{'via:0:3:1:0':1}),defaultdict(int,{'via:0:3:1:0':100}),options.present_penalty,options)

@pytest.mark.parametrize('renamed',[False,True])
def test_retained_alternative_changes_actual_congested_route_without_capacity_change(monkeypatch,renamed):
    board,graph,root,pool=scene(renamed);before=(repr(board),dict(graph.resources),dict(graph.via_sites))
    def forbidden(*a,**k):raise AssertionError('selection cannot generate/check/register geometry')
    monkeypatch.setattr(routing,'RoutingClearanceIndex',forbidden)
    old=route(board,graph,root,pool[:3])
    assert old.connected and old.accesses[-1].via.position==Point.mm(2,2)
    retained=routing._bounded_access_choices(list(pool),4)
    new=route(board,graph,root,retained)
    assert new.connected and new.accesses[-1].via.position==Point.mm(3,2)
    assert len(retained)==4 and set(retained)<=set(pool)
    assert (repr(board),dict(graph.resources),dict(graph.via_sites))==before

def test_alias_reduction_preserves_available_layer_for_single_via_budget():
    board,graph,root,pool=scene()
    retained=routing._bounded_access_choices(list(pool),4)
    assert {a.layer for a in retained}=={I,J,B}
    answer=route(board,graph,root,retained,max_vias=1)
    assert answer.connected and answer.accesses[-1].layer is B
    assert len(answer.vias)==1

@pytest.mark.parametrize('limit',[1,2,3,4,6,8])
def test_all_selected_launches_are_original_exact_objects_and_bound_is_shared(limit):
    _,_,_,pool=scene()
    selected=routing._bounded_access_choices(list(pool),limit)
    assert len(selected)==min(limit,len(pool))
    assert all(any(a is original for original in pool) for a in selected)
    if limit>=2:assert len({a.via.position for a in selected})==2

def test_only_one_physical_launch_keeps_layer_aliases_and_actual_overflow():
    board,graph,root,pool=scene();selected=routing._bounded_access_choices(list(pool[:3]),4)
    answer=route(board,graph,root,selected)
    assert answer.connected and len(selected)==3
    prior=routing.GlobalNetRoute('PRIOR',True,(),(),
        (routing.GlobalViaProposal('PRIOR',Point.mm(2,2),F,B,'via:0:3:1:0'),),0)
    measured=routing._attempt(board,graph,{},(prior,answer),GlobalRouterOptions(),20)
    assert measured.metrics.total_overflow==1

def test_surface_entrances_keep_their_distinct_graph_nodes():
    _,_,_,pool=scene();surface=[replace(a,via=None) for a in pool[:3]]
    selected=routing._bounded_access_choices(surface,4)
    assert selected==tuple(surface)

def test_no_generated_choice_has_no_invented_escape():
    assert routing._bounded_access_choices([],4)==()
