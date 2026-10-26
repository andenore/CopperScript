"""Required return vias must exist when the unified critical search runs."""
from dataclasses import replace
import pytest
from pcbir import CopperLayer, Point, nm_from_mm
from pcbir.physical import CopperZone, CopperKeepout, PolygonWithHoles, PolygonRing, NetRoutingRule, RouteKind
from pcbir.flow import run_routing_pipeline, critical_return_plane_options
from pcbir.plane import PlaneStitchOptions
from pcbir.placement import PlacementPlannerOptions
from pcbir.routeflow import PlacementRoutingFeedbackOptions
from pcbir.unified_router import UnifiedRouterOptions
from pcbir.return_paths import covering_return_vias
from test_unified_router import _board


def fixture():
    board = _board({'DATA':((3,10),(17,10)), 'GND':((3,12),(17,12))}, layers=4)
    def rectangle(x0,y0,x1,y1):
        return PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((x0,y0),(x1,y0),(x1,y1),(x0,y1)))))
    rule = NetRoutingRule('DATA',RouteKind.CRITICAL,width_nm=200000,
        allowed_layers=(CopperLayer.FRONT,CopperLayer.BACK),max_vias=2,max_length_nm=nm_from_mm(20),
        require_return_vias=True,return_via_net='GND',maximum_return_via_distance_nm=nm_from_mm('2.2'))
    return replace(board,net_routing_rules=(rule,),
        zones=(CopperZone('reference','GND',(CopperLayer.INTERNAL_1,CopperLayer.INTERNAL_2),rectangle(1,1,25,25)),),
        copper_keepouts=(CopperKeepout('front-wall',(CopperLayer.FRONT,),rectangle(12.5,1,13.5,25)),))


@pytest.mark.parametrize('early', (False,True))
def test_required_returns_are_prepared_before_critical_routing(early, tmp_path):
    from pcbir.workbench import save_state, load_state
    from pcbir.critical import replay_critical
    board=fixture()
    snapshots = {}
    result=run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,refinement_passes=0,
            fixed_references=tuple(p.reference for p in board.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        plane_stitch_options=PlaneStitchOptions() if early else None,
        unified_options=UnifiedRouterOptions(maximum_iterations=4,time_budget_s=5),
        on_stage=lambda name, state: snapshots.update({name: state}))
    critical=next(n for n in result.critical.nets if n.nets==('DATA',))
    # Without early return contacts DATA detours past its 20 mm limit: it
    # still connects, with a length warning rather than an open.
    assert critical.connected and bool(critical.warnings) == (not early)
    saved = tmp_path / 'critical-inputs.state.json'
    save_state(saved, snapshots['critical-inputs'])
    restored = load_state(saved)
    assert (restored['plane_stitch_options'] is not None) == early
    replay = replay_critical(saved, unified_options=UnifiedRouterOptions(maximum_iterations=4,time_budget_s=5))
    replayed = next(n for n in replay.nets if n.nets==('DATA',))
    assert replayed.connected and bool(replayed.warnings) == (not early)
    if early:
        signal=[v for v in result.critical.board.vias if v.net=='DATA']
        assert len(signal)==2
        assert all(covering_return_vias(result.critical.board,board.net_routing_rules[0],v) for v in signal)
        assert result.critical.board.copper_keepouts == board.copper_keepouts


def test_return_preparation_keeps_out_of_the_pair_launch_corridors():
    from pcbir.plane import stitch_zone_pads
    from pcbir.critical import route_critical_nets
    from pcbir import route_global
    board=fixture()
    gnd=[p for p in board.nets if p.name=='GND'][0].pads
    def rectangle(x0,y0,x1,y1):
        return PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((x0,y0),(x1,y0),(x1,y1),(x0,y1)))))
    # A corridor west of the first GND pad (at 6,15), where the free stitch
    # puts its via (5.5,14.5), on every layer: the stitch may not use it.
    corridor=CopperKeepout('port-halo-0',tuple(board.stackup.copper_layers),rectangle(4,13,5.8,17),
                           block_tracks=True,block_vias=True,block_zones=False)
    plain=critical_return_plane_options(board,PlaneStitchOptions())
    bound=critical_return_plane_options(board,PlaneStitchOptions(),(corridor,))
    assert plain.keepouts==() and bound.keepouts==(corridor,) and bound.only_pads==plain.only_pads
    inside=lambda v: 4e6<=v.position.x_nm<=5.8e6 and 13e6<=v.position.y_nm<=17e6
    near=lambda v: abs(v.position.x_nm-6e6)<=1.5e6 and abs(v.position.y_nm-15e6)<=1.5e6
    free=stitch_zone_pads(board,plain)
    assert any(inside(v) for v in free.board.vias if v.net=='GND')
    kept=stitch_zone_pads(replace(board,copper_keepouts=(*board.copper_keepouts,*bound.keepouts)),bound)
    assert not any(inside(v) for v in kept.board.vias if v.net=='GND')
    assert any(near(v) and not inside(v) for v in kept.board.vias if v.net=='GND')
    # Through the critical stage the keepouts bind the stitch only: the
    # routed board carries none of them.
    routed=route_critical_nets(board,route_global(board),plane_stitch_options=bound)
    assert not any(inside(v) for v in routed.board.vias if v.net=='GND')
    assert routed.board.copper_keepouts==board.copper_keepouts
    assert next(n for n in routed.nets if n.nets==('DATA',)).connected


def test_return_preparation_respects_requested_pads_and_declared_planes():
    board=fixture()
    assert critical_return_plane_options(board,None) is None
    assert critical_return_plane_options(board,PlaneStitchOptions(only_pads=frozenset())) is None
    assert critical_return_plane_options(replace(board,zones=()),PlaneStitchOptions()) is None
    ground=next(n for n in board.nets if n.name=='GND')
    requested=PlaneStitchOptions(only_pads=frozenset({ground.pads[0]}),maximum_radius_nm=1000000)
    chosen=critical_return_plane_options(board,requested)
    assert chosen.only_pads==requested.only_pads and chosen.maximum_radius_nm==1000000
