"""Exact paired routing retains real source terminals when coarse access fails."""
from dataclasses import replace
import pytest
from pcbir import (BoardOutline, CopperLayer, FootprintPad, PhysicalFootprint,
    PhysicalBoard, PhysicalNet, PadReference, Placement, Point, Size,
    NetRoutingRule, RouteKind, route_global, route_critical_nets)
from pcbir.physical import DesignRules
from pcbir.drc import explicit_copper_connectivity, run_physical_drc, PhysicalDrcPolicy

def fixture(duplicates=False):
    source=PhysicalFootprint('pair',(FootprintPad('1',Point.mm(0,-.5),Size.mm(.3,.3)),
        FootprintPad('2',Point.mm(0,.5),Size.mm(.3,.3))),Size.mm(1,2))
    one=PhysicalFootprint('one',(FootprintPad('1',Point(0,0),Size.mm(.3,.3)),),Size.mm(1,.6))
    if duplicates:
        target=PhysicalFootprint('contacts',(
            FootprintPad('1',Point.mm(0,-.5),Size.mm(.3,.3)),
            FootprintPad('2',Point.mm(0,.5),Size.mm(.3,.3)),
            FootprintPad('3',Point.mm(1.7,-.25),Size.mm(.3,.3)),
            FootprintPad('4',Point.mm(1.7,.75),Size.mm(.3,.3))),Size.mm(4,4))
        poses=(Placement('U','pair',Point.mm(5,10)),Placement('J','contacts',Point.mm(20,10)))
        footprints={source.name:source,target.name:target}
        nets=(PhysicalNet('A',(PadReference('U','1'),PadReference('J','1'),PadReference('J','3'))),
            PhysicalNet('B',(PadReference('U','2'),PadReference('J','2'),PadReference('J','4'))))
    else:
        poses=(Placement('U','pair',Point.mm(5,10)),Placement('C_A','one',Point.mm(20,9.25)),
            Placement('C_B','one',Point.mm(20,10.75)))
        footprints={source.name:source,one.name:one}
        nets=(PhysicalNet('A',(PadReference('U','1'),PadReference('C_A','1'))),
            PhysicalNet('B',(PadReference('U','2'),PadReference('C_B','1'))))
    rules=tuple(NetRoutingRule(name,RouteKind.DIFFERENTIAL,differential_partner=partner,
        width_nm=200000,pair_gap_nm=200000,allowed_layers=(CopperLayer.FRONT,),
        max_vias=0,max_skew_nm=500000,breakout_length_nm=2000000,
        breakout_width_nm=150000,breakout_gap_nm=150000,breakout_clearance_nm=100000)
        for name,partner in (('A','B'),('B','A')))
    return PhysicalBoard('paired terminals',BoardOutline.rectangle(30,20),footprints,poses,nets,net_routing_rules=rules,rules=DesignRules(minimum_clearance_nm=100000,minimum_track_width_nm=100000,default_track_width_nm=200000))

@pytest.mark.parametrize('duplicates',[False,True])
def test_exact_search_without_coarse_guides_connects_every_real_terminal(duplicates):
    board=fixture(duplicates)
    global_result=route_global(board)
    global_result=replace(global_result,routes=tuple(replace(r,connected=False,accesses=(),segments=(),vias=()) for r in global_result.routes))
    result=route_critical_nets(board,global_result)
    assert all(r.connected for r in result.nets),result.nets
    graph=explicit_copper_connectivity(result.board)
    assert all(graph.net_connected(n) for n in board.nets)
    findings=run_physical_drc(result.board,policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings
    assert not [f for f in findings if f.severity.value=='error'],findings
    assert board.tracks==board.vias==()

def test_tiny_budget_does_not_adopt_partial_connector_ties():
    board=fixture(True)
    global_result=route_global(board)
    global_result=replace(global_result,routes=tuple(replace(r,connected=False,accesses=(),segments=(),vias=()) for r in global_result.routes))
    result=route_critical_nets(board,global_result,pair_state_limit=1)
    assert not any(r.connected for r in result.nets)
    assert result.board.tracks==board.tracks and result.board.vias==board.vias


def test_coarse_pair_stubs_retain_checked_source_lead_in_occurrences():
    from pcbir.routing import PinAccess, GridNode, GlobalNetRoute
    from pcbir.physical import TrackSegment
    from pcbir.critical import _pair_pin_stubs
    board=fixture()
    layer=CopperLayer.FRONT
    first=(TrackSegment('A',Point.mm(5,9.5),Point.mm(6,9.5),200000,layer),
        TrackSegment('A',Point.mm(6,9.5),Point.mm(6.5,9.25),200000,layer))
    second=(TrackSegment('A',Point.mm(20,9.25),Point.mm(18.5,9.25),200000,layer),)
    accesses=(PinAccess(PadReference('U','1'),Point.mm(5,9.5),GridNode(0,0,0),Point.mm(6.5,9.25),layer,first),
        PinAccess(PadReference('C_A','1'),Point.mm(20,9.25),GridNode(0,1,0),Point.mm(18.5,9.25),layer,second))
    guide=GlobalNetRoute('A',True,accesses,(),(),0)
    paths=[(layer,Point.mm(6.5,9.25),Point.mm(18.5,9.25))]
    assert _pair_pin_stubs('A',guide,200000,paths,1,400000,
        board=board,retain_access_tracks=True)==(*first,*second)

def test_cli_exposes_positive_computational_pin_access_step(capsys):
    from pcbir.cli import main
    import pytest
    with pytest.raises(SystemExit) as e:
        main(['route-board','missing.copper','--pin-access-step-mm','0'])
    assert e.value.code==2
    assert 'positive' in capsys.readouterr().err
