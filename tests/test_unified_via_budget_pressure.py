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
    assert not getattr(proposed,"_ignore_soft_costs",False)
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


@pytest.mark.parametrize("native",(False,True))
def test_via_bounded_nets_still_negotiate_with_each_other_under_history(monkeypatch,native):
    if not native:
        import pcbir.native
        monkeypatch.setattr(pcbir.native,"astar",lambda:None)
    base=_board({"S":((2,10),(18,10)),"T":((2,12),(18,12))})
    one=fixture()
    def channel_wall(name,y0,y1):
        outline=PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in
            ((0,y0),(26,y0),(26,y1),(0,y1)))))
        return CopperKeepout(name,(CopperLayer.BACK,),outline)
    board=replace(base,copper_keepouts=(*one.copper_keepouts,
        channel_wall("back-upper",0,12),channel_wall("back-lower",14,26)),
        net_routing_rules=(one.net_routing_rules[0],replace(one.net_routing_rules[0],net="T")))
    control=_Router(board,UnifiedRouterOptions(maximum_iterations=16,commit_rounds=1),list(board.nets),None)
    plain,plain_conflicts,_=control.negotiate(None)
    assert control.commit(plain,plain_conflicts)[2]=={"S","T"}
    proposed=_Router(board,UnifiedRouterOptions(maximum_iterations=16,commit_rounds=1),list(board.nets),None)
    g=proposed.grid;layer=g.layers.index(CopperLayer.BACK)
    for x in range(g.nx):
        if nm_from_mm("12.5")<=g.x0+x*g.pitch<=nm_from_mm("13.5"):
            for y in range(g.ny):proposed.occupancy.history[g.index(layer,x,y)]=5000
    routes,conflicts,_=proposed.negotiate(None)
    accepted,rejected,complete=proposed.commit(routes,conflicts)
    assert complete=={"S","T"},(conflicts,rejected,{n:r.reason for n,r in routes.items()})
    assert all(len(route.via_xy)<=2 for route in routes.values())
    result=replace(board,tracks=tuple(t for tracks,_ in accepted.values() for t in tracks),
                   vias=tuple(v for _,vias in accepted.values() for v in vias))
    report=connection_summary(result,run_physical_drc(result))
    assert report["signal"]["opens"]==0 and report["drc"]["errors"]==0, [(f.code,f.message) for f in run_physical_drc(result).findings]
