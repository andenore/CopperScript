"""Off-grid access copper must participate in negotiated via clearance."""
import pytest
from pcbir import CopperLayer, Point, TrackSegment, nm_from_mm
from pcbir.unified_router import _Router, _NetRoute, UnifiedRouterOptions
from test_unified_router import _board


@pytest.mark.parametrize("native", (False, True))
@pytest.mark.parametrize("sign", (-1,1))
@pytest.mark.parametrize("offset,close", ((.375,True),(.625,False),(.875,False)))
def test_short_off_grid_lead_reports_a_real_via_clearance_conflict(monkeypatch, native, sign, offset, close):
    if not native:
        import pcbir.native
        monkeypatch.setattr(pcbir.native, "astar", lambda: None)
    board = _board({"A": ((2,2),(3,2)), "B": ((18,18),(19,18))})
    router = _Router(board, UnifiedRouterOptions(), list(board.nets), None)
    grid = router.grid
    def node(layer, x, y):
        return grid.index(grid.layers.index(layer),
            round((nm_from_mm(x)-grid.x0)/grid.pitch),
            round((nm_from_mm(y)-grid.y0)/grid.pitch))
    front, back = CopperLayer.FRONT, CopperLayer.BACK
    a = _NetRoute([[node(front,13,13),node(back,13,13)]], [()],
                  set(),set(),set(),set(),True,0.0)
    start, end = Point.mm(13+sign*offset,13.125), Point.mm(13+sign*(offset+.125),13.25)
    lead = TrackSegment("B", start, end,
                        nm_from_mm("0.2"), front)
    b = _NetRoute([[node(front,13+sign*(offset+.125),13.25)]], [(lead,)],
                  set(),set(),set(),set(),True,0.0)
    for route,spec in ((a,router.specs[0]),(b,router.specs[1])):
        router._finish_route(route,spec)
        router.occupancy.add(route,1)
    # The exact geometry oracle already rejects this via/lead pair. Negotiation
    # must also see it, before fixing one route and rejecting the other.
    routes={"A":a,"B":b}
    _,rejected,_=router.commit(routes)
    assert bool(rejected)==close
    conflicts=router._conflicts(routes)
    assert conflicts==({"A","B"} if close else set())
    router._penalise(routes, conflicts)
    xy = node(front,13,13) % grid.plane
    assert (router.occupancy.via_history[xy] > 0)==close


@pytest.mark.parametrize("native",(False,True))
def test_grid_diagonal_via_collision_is_not_hidden_between_raster_nodes(monkeypatch,native):
    if not native:
        import pcbir.native
        monkeypatch.setattr(pcbir.native,"astar",lambda:None)
    board=_board({"A":((2,2),(3,2)),"B":((18,18),(19,18))})
    router=_Router(board,UnifiedRouterOptions(),list(board.nets),None);g=router.grid
    def node(layer,x,y):
        return g.index(g.layers.index(layer),round((nm_from_mm(x)-g.x0)/g.pitch),
                       round((nm_from_mm(y)-g.y0)/g.pitch))
    f,b=CopperLayer.FRONT,CopperLayer.BACK
    a=_NetRoute([[node(f,13,13),node(b,13,13)]],[()],set(),set(),set(),set(),True,0.)
    track=_NetRoute([[node(f,13.5,13.25),node(f,13.25,13.5)]],[()],set(),set(),set(),set(),True,0.)
    for route,spec in ((a,router.specs[0]),(track,router.specs[1])):
        router._finish_route(route,spec);router.occupancy.add(route,1)
    routes={"A":a,"B":track}
    assert router.commit(routes)[1]  # Exact copper oracle sees the real clash.
    assert router._conflicts(routes)=={"A","B"}
    router._penalise(routes,{"A","B"})
    assert router.occupancy.via_history[node(f,13,13)%g.plane]>0


@pytest.mark.parametrize("native",(False,True))
def test_via_proposals_avoid_mutable_tracks_on_unvisited_inner_layers(monkeypatch,native):
    from dataclasses import replace
    from pcbir.physical import BoardSide,NetRoutingRule,RouteKind
    from pcbir.routing_clearance import RoutingClearanceIndex
    if not native:
        import pcbir.native
        monkeypatch.setattr(pcbir.native,"astar",lambda:None)
    board=_board({"A":((2,10),(18,10)),"B":((2,2),(18,2))},layers=4)
    board=replace(board,placements=tuple(replace(p,side=BoardSide.BACK) if p.reference=="A_1" else p
                                         for p in board.placements),
        net_routing_rules=(NetRoutingRule("A",RouteKind.GENERAL,width_nm=200000,
            allowed_layers=(CopperLayer.FRONT,CopperLayer.BACK),max_vias=1),))
    router=_Router(board,UnifiedRouterOptions(),list(board.nets),None);g=router.grid
    specs={s.name:s for s in router.specs}
    baseline=router._route_net_budgeted(specs["A"],100.)
    assert baseline.connected and len(baseline.via_xy)==1
    xy=next(iter(baseline.via_xy));y,x=divmod(xy,g.nx)
    layer=g.layers.index(CopperLayer.INTERNAL_1)
    foreign=_NetRoute([[g.index(layer,x,y-3),g.index(layer,x,y+3)]],[()],
                       set(),set(),set(),set(),True,0.)
    router._finish_route(foreign,specs["B"])
    tracks,_=router.copper(specs["B"],foreign.paths[0],())
    index=RoutingClearanceIndex(board)
    assert all(index.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in tracks)
    for track in tracks:index.add_track(track)
    router.occupancy.add(foreign,1)
    proposed=router._route_net_budgeted(specs["A"],100.)
    assert proposed.connected and len(proposed.via_xy)==1
    for path,leads in zip(proposed.paths,proposed.leads):
        _,vias=router.copper(specs["A"],path,leads)
        assert all(index.can_via(v.net,v.position,v.size_nm,v.from_layer,v.to_layer,v.drill_nm)
                   for v in vias), "Via proposal ignored a mutable track on its internal barrel span"
