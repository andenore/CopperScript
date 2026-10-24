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


def test_complete_pair_accepts_matched_branch_and_preserves_all_source_contacts():
    from pcbir import route_global,route_critical_nets
    board=fixture()
    result=route_critical_nets(board,route_global(board))
    assert all(n.connected for n in result.nets),result.nets
    graph=explicit_copper_connectivity(result.board)
    assert all(graph.net_connected(n) for n in board.nets)
    assert not [f for f in run_physical_drc(result.board,policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings if f.severity.value=='error']
    assert board.tracks==board.vias==()


def support_fixture():
    from test_critical_routing import _pair_board
    board = _pair_board()
    one = PhysicalFootprint("support", (FootprintPad("1", Point(0, 0), Size.mm("0.4", "0.4")),), Size.mm(1, 1))
    refs = {"USB_DP": "SUP_P", "USB_DM": "SUP_N"}
    return replace(board, footprints={**board.footprints, "support": one},
        placements=(*board.placements, Placement("SUP_P", "support", Point.mm(33, "10.5")),
                    Placement("SUP_N", "support", Point.mm(33, "13.5"))),
        nets=tuple(replace(n, pads=(*n.pads, PadReference(refs[n.name], "1"))) for n in board.nets),
        net_routing_rules=tuple(replace(r, breakout_length_nm=4000000, breakout_width_nm=200000) for r in board.net_routing_rules))


def test_separate_nearby_support_contacts_keep_two_real_main_endpoints():
    from pcbir import route_global, route_critical_nets
    board = support_fixture()
    guides = route_global(board)
    # Failed coarse access must not grant connectivity or lose any real pad.
    guides = replace(guides, routes=tuple(replace(g, connected=False, segments=(), vias=()) for g in guides.routes))
    result = route_critical_nets(board, guides, pair_state_limit=6000)
    assert result.nets[0].connected, result.nets
    assert result.nets[0].local_preparation.status == "joint_support_contacts"
    assert not result.nets[0].local_preparation.pending_contacts
    assert 0 < result.nets[0].local_preparation.support_trials <= 16
    assert result.nets[0].local_preparation.support_expanded_states <= 1200
    assert all(not g.connected for g in guides.routes)
    graph = explicit_copper_connectivity(result.board)
    assert all(graph.net_connected(n) for n in board.nets)
    assert board.tracks == board.vias == ()
    assert not [f for f in run_physical_drc(result.board,
        policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings if f.severity.value == "error"]


@pytest.mark.parametrize("mode", ("remote", "ambiguous", "no-contract"))
def test_support_contacts_require_one_source_declared_local_endpoint(mode):
    board = support_fixture()
    if mode == "remote":
        board = replace(board, placements=tuple(replace(p, position=Point.mm(20, 4))
            if p.reference == "SUP_P" else p for p in board.placements))
    elif mode == "ambiguous":
        board = replace(board, net_routing_rules=tuple(replace(r, breakout_length_nm=40000000)
            for r in board.net_routing_rules))
    else:
        board = replace(board, net_routing_rules=tuple(replace(r, breakout_length_nm=None, breakout_width_nm=None)
            for r in board.net_routing_rules))
    from pcbir.pair_terminals import PairTerminalPreparationStats
    report = PairTerminalPreparationStats()
    _, a, b, pairs, returns = prepare_pair_terminals(board, *board.net_routing_rules, {}, report=report)
    assert not (a or b or pairs or returns)
    assert report.status == "unsupported"
    assert board.tracks == board.vias == ()
