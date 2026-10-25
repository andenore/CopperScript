"""A later preparation step must not occupy a promised signal-via site."""
from dataclasses import replace
from collections import Counter
import pytest
import pcbir.flow as flow
from pcbir import CopperLayer, Point, nm_from_mm, FootprintPad, Size
from pcbir.physical import CopperZone, PolygonWithHoles, PolygonRing, NetRoutingRule, RouteKind
from pcbir.fanout import FanoutOptions
from pcbir.plane import PlaneStitchOptions
from pcbir.placement import PlacementPlannerOptions
from pcbir.routeflow import PlacementRoutingFeedbackOptions
from pcbir.unified_router import UnifiedRouterOptions
from pcbir.routing_clearance import RoutingClearanceIndex
from test_unified_router import _board


@pytest.mark.parametrize('retain_copper', (False, True))
def test_signal_exit_masks_protect_plane_preparation_but_are_removed_for_negotiation(monkeypatch, retain_copper):
    board=_board({'S':((10,7),(10,17)), 'C':((1,9),(19,9)), 'GND':((3,3),(20,17))},layers=4)
    # Escape planning requires a real neighbouring land, even with a low
    # package threshold; a one-pad footprint has no package access demand.
    fp=next(iter(board.footprints.values()))
    fp=replace(fp,pads=(*fp.pads,FootprintPad('2',Point.mm(-1,0),Size.mm('0.6','0.6'))))
    board=replace(board,footprints={fp.name:fp})
    polygon=PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((1,1),(25,1),(25,25),(1,25)))))
    board=replace(board,zones=(CopperZone('ground','GND',(CopperLayer.INTERNAL_1,),polygon),),
        net_routing_rules=(NetRoutingRule('C',RouteKind.CLOCK,width_nm=200000,
            allowed_layers=(CopperLayer.FRONT,),max_vias=0),))
    actual=flow.stitch_zone_pads
    observed=[]
    def stitch(source,options):
        masks=tuple(k for k in source.copper_keepouts if k.id.startswith('escape-reserve-via-'))
        assert masks, 'plane preparation lost the still-valid signal exit reservations'
        clean=replace(source,copper_keepouts=tuple(k for k in source.copper_keepouts if not k.id.startswith('escape-reserve')))
        unmasked=RoutingClearanceIndex(clean);guarded=RoutingClearanceIndex(source)
        available=[]
        for mask in masks:
            vertices=mask.outline.outer.vertices
            site=Point(sum(p.x_nm for p in vertices)//len(vertices),sum(p.y_nm for p in vertices)//len(vertices))
            args=('GND',site,source.rules.default_via_size_nm,CopperLayer.FRONT,CopperLayer.BACK)
            if unmasked.can_via(*args,drill_nm=source.rules.default_via_drill_nm):
                assert not guarded.can_via(*args,drill_nm=source.rules.default_via_drill_nm)
                available.append(site)
        assert available, 'fixture must expose an otherwise legal competing ground-via site'
        observed.extend(available)
        return actual(source,options)
    monkeypatch.setattr(flow,'stitch_zone_pads',stitch)
    stages = {}
    result=flow.run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,refinement_passes=0,
            fixed_references=tuple(p.reference for p in board.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        fanout_options=FanoutOptions(minimum_component_pads=1),plane_stitch_options=PlaneStitchOptions(),
        unified_options=UnifiedRouterOptions(maximum_iterations=5,time_budget_s=5,pre_route_decouplers=False,
            retain_escape_copper=retain_copper),
        on_stage=lambda name,value: stages.update({name:value}))
    assert observed
    assert not any(k.id.startswith('escape-reserve') for k in result.board.copper_keepouts)
    # No critical failure: the opt-in policy must not freeze unrelated escapes.
    assert 'critical-retry-inputs' not in stages


def test_failed_critical_retry_converts_whole_endangered_escape_group(monkeypatch):
    board=_board({'S':((10,7),(10,17)), 'C':((1,8.5),(19,8.5))}, layers=4)
    fp=next(iter(board.footprints.values()))
    fp=replace(fp,pads=(*fp.pads,FootprintPad('2',Point.mm(-1,0),Size.mm('0.6','0.6'))))
    board=replace(board,footprints={fp.name:fp},
        net_routing_rules=(NetRoutingRule('C',RouteKind.CLOCK,width_nm=200000,
            allowed_layers=(CopperLayer.FRONT,),max_vias=0),))
    actual = flow.route_critical_nets
    attempts = []
    def bounded(source, global_route, **kwargs):
        attempts.append(source)
        if len(attempts) == 1:
            # A bounded first-stage miss, before any critical copper commits.
            from pcbir.critical import CriticalNetResult, CriticalRoutingStatus
            empty = actual(source,global_route,only_nets=frozenset(),**kwargs)
            return replace(empty,status=CriticalRoutingStatus.FAILED,
                nets=(CriticalNetResult(('C',),False,0,0,(),0,('bounded fixture miss',)),))
        return actual(source,global_route,**kwargs)
    monkeypatch.setattr(flow,'route_critical_nets',bounded)
    stages = {}
    events = []
    result=flow.run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,refinement_passes=0,
            fixed_references=tuple(p.reference for p in board.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        fanout_options=FanoutOptions(minimum_component_pads=1),
        unified_options=UnifiedRouterOptions(maximum_iterations=5,time_budget_s=5,
            pre_route_decouplers=False,retain_escape_copper=True),
        on_stage=lambda name,value:stages.update({name:value}),
        on_progress=lambda phase,event,details:events.append((phase,event,details)))
    assert len(attempts) == 2
    held = stages['critical-retry-inputs']['reserved_accesses']
    assert held is not None and len(held.created_vias) == 2
    assert all(n.connected for n in result.critical.nets)
    assert Counter(held.board.tracks) <= Counter(result.board.tracks)
    assert Counter(held.board.vias) <= Counter(result.board.vias)
