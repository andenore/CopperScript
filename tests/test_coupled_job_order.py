"""A flexible single must not consume an equal-priority pair's only layer."""
from dataclasses import replace
import pytest

from pcbir import route_global, route_critical_nets, CopperLayer
from pcbir.physical import (PhysicalFootprint, FootprintPad, Point, Size, Placement,
                            PhysicalNet, PadReference, NetRoutingRule, RouteKind, ComponentPlacementRule)
from pcbir.drc import explicit_copper_connectivity, run_physical_drc, PhysicalDrcPolicy
from pcbir.unified_router import UnifiedRouterOptions
from test_critical_routing import _pair_board


@pytest.mark.parametrize('clock_priority', (100,101))
def test_coupled_exit_precedes_equal_priority_clock_but_never_overrides_explicit_priority(clock_priority):
    board=_pair_board()
    terminal=PhysicalFootprint('one',(FootprintPad('1',Point(0,0),Size.mm(.4,.4)),),Size.mm(.4,.4))
    board=replace(board,footprints={**board.footprints,'one':terminal},
        rules=replace(board.rules,minimum_clearance_nm=100000,minimum_track_width_nm=100000),
        placements=(*board.placements,Placement('OSC','one',Point.mm(20,.6)),
                    Placement('DEST','one',Point.mm(20,24.4))),
        placement_rules=(ComponentPlacementRule('OSC',edge_clearance_nm=300000),
                         ComponentPlacementRule('DEST',edge_clearance_nm=300000)),
        nets=(*board.nets,PhysicalNet('REFERENCE',(PadReference('OSC','1'),PadReference('DEST','1')))),
        net_routing_rules=(*tuple(replace(r,allowed_layers=(CopperLayer.FRONT,),max_vias=0)
                                  for r in board.net_routing_rules),
            NetRoutingRule('REFERENCE',RouteKind.CLOCK,priority=clock_priority,width_nm=150000,
                max_length_nm=30000000,max_vias=2,allowed_layers=(CopperLayer.FRONT,CopperLayer.BACK))))
    result=route_critical_nets(board,route_global(board),pair_state_limit=1000,
        unified_options=UnifiedRouterOptions(time_budget_s=5,maximum_iterations=8))
    pair=next(r for r in result.nets if len(r.nets)==2)
    clock=next(r for r in result.nets if len(r.nets)==1)
    assert clock.connected
    if clock_priority==101:
        assert not pair.connected  # The explicit higher priority takes the only front crossing.
    else:
        assert pair.connected,result.nets
        assert clock.via_count==2
        graph=explicit_copper_connectivity(result.board)
        assert all(graph.net_connected(n) for n in board.nets)
        assert not [f for f in run_physical_drc(result.board,
            policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings if f.severity.value=='error']
    assert board.tracks==board.vias==()
