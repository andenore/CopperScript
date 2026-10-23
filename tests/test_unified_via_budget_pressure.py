"""A legal via-bounded route must be able to contest old congestion costs."""
from dataclasses import replace
import pytest
import numpy as np
from pcbir import CopperLayer, Point, nm_from_mm, run_physical_drc
from pcbir.physical import CopperKeepout, PolygonWithHoles, PolygonRing, NetRoutingRule, RouteKind
from pcbir.unified_router import _Router, UnifiedRouterOptions
from pcbir.route_summary import connection_summary
from test_unified_router import _board


def fixture(back_wall=False):
    board=_board({'S':((2,10),(18,10))})
    def wall(name,layer,x):
        polygon=PolygonWithHoles(PolygonRing(tuple(Point.mm(a,b) for a,b in
            ((x-.5,0),(x+.5,0),(x+.5,26),(x-.5,26)))))
        return CopperKeepout(name,(layer,),polygon)
    keepouts=(wall('front-a',CopperLayer.FRONT,10),wall('front-b',CopperLayer.FRONT,16))
    if back_wall:keepouts=(*keepouts,wall('real-back-wall',CopperLayer.BACK,13))
    return replace(board,copper_keepouts=keepouts,net_routing_rules=(NetRoutingRule('S',RouteKind.GENERAL,
        width_nm=nm_from_mm('0.2'),allowed_layers=(CopperLayer.FRONT,CopperLayer.BACK),max_vias=2),))


def router(board):
    return _Router(board,UnifiedRouterOptions(maximum_iterations=3,commit_rounds=1),list(board.nets),None)


@pytest.mark.parametrize('native',(False,True))
@pytest.mark.parametrize('cost_field',('history','occ'))
def test_via_budget_can_reclaim_a_legal_corridor_from_soft_costs(monkeypatch,native,cost_field):
    if not native:
        import pcbir.native
        monkeypatch.setattr(pcbir.native,'astar',lambda:None)
    board=fixture();control=router(board)
    assert control._route_net_budgeted(control.specs[0],.6).connected
    proposed=router(board);g=proposed.grid
    layer=g.layers.index(CopperLayer.BACK)
    pressure=getattr(proposed.occupancy,cost_field)
    corridor=set()
    for x in range(g.nx):
        if nm_from_mm('12.5')<=g.x0+x*g.pitch<=nm_from_mm('13.5'):
            for y in range(g.ny):
                node=g.index(layer,x,y);pressure[node]=5000;corridor.add(node)
    original=np.array(pressure,copy=True)
    route=proposed._route_net_budgeted(proposed.specs[0],.6)
    assert route.connected,route.reason
    assert len(route.via_xy)<=2
    # The retry changes only this proposal's costs, never global history.
    assert np.array_equal(original,np.array(pressure))
    assert route.claims & corridor
    assert not proposed._ignore_soft_costs
    assert proposed.specs[0].via_factor==4.0
    accepted,rejected,complete=proposed.commit({'S':route})
    assert complete=={'S'} and not rejected
    tracks,vias=accepted['S'];result=replace(board,tracks=tuple(tracks),vias=tuple(vias))
    report=connection_summary(result,run_physical_drc(result))
    assert report['signal']['opens']==0 and report['drc']['errors']==0
    assert len(vias)<=2


def test_via_budget_retry_never_ignores_real_source_obstacles():
    proposed=router(fixture(back_wall=True))
    route=proposed._route_net_budgeted(proposed.specs[0],.6)
    assert not route.connected
    accepted,_,complete=proposed.commit({'S':route})
    assert 'S' not in complete
    assert sum(len(vias) for _,vias in accepted.values())<=2
