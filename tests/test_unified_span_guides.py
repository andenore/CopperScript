"""Physical via spans in search and guide costs across repair routers."""
from dataclasses import replace

import pytest

from pcbir import (CopperKeepout, CopperLayer, PhysicalNet, Point, PolygonRing,
                   PolygonWithHoles, Via, nm_from_mm, run_physical_drc)
from pcbir.physical import NetRoutingRule, ViaKind
from pcbir.routing_clearance import RoutingClearanceIndex
import pcbir.unified_router as unified
from test_unified_commit_index import _grid_board, _guides, _four_layer, _technology, THROUGH, BLIND, FRONT, IN1
from test_unified_router import _walled_plane_pad_board

BACK_BLIND = _technology('blind-b2', ViaKind.BLIND, CopperLayer('In2.Cu'), CopperLayer.BACK)


@pytest.fixture(params=['python', 'native'])
def backend(request, monkeypatch):
    import pcbir.native as native
    if request.param == 'python':
        monkeypatch.setattr(native, 'astar', lambda: None)
    elif native.astar() is None:
        pytest.skip('native kernel unavailable')


def _keepout(name, layers, bounds, *, tracks=False, vias=True):
    x0, y0, x1, y1 = bounds
    outline = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((x0, y0), (x1, y0), (x1, y1), (x0, y1)))))
    return CopperKeepout(name, layers, outline, block_tracks=tracks, block_vias=vias,
                         block_pads=False, block_zones=False)


@pytest.mark.parametrize('blocked_layer,connected', [(CopperLayer.BACK, True), (IN1, False)])
def test_blind_vias_check_only_their_copper_span(backend, blocked_layer, connected):
    board = _four_layer((THROUGH, BLIND), layers=(FRONT, IN1))
    wall = _keepout('wall', (FRONT,), (12.8, 0, 13.2, 26), tracks=True, vias=False)
    blocker = _keepout('via-blocker', (blocked_layer,), (0, 0, 26, 26))
    board = replace(board, tracks=(), copper_keepouts=(wall,))
    options = unified.UnifiedRouterOptions(maximum_iterations=3, commit_rounds=1)
    control = unified.route_unified(board, _guides(board), options)
    assert next(n for n in control.nets if n.net == 'A').connected
    assert control.board.vias and all(v.to_layer == IN1 for v in control.board.vias)
    board = replace(board, copper_keepouts=(wall, blocker))
    exact = RoutingClearanceIndex(board)
    assert all(exact.can_via(v.net, v.position, v.size_nm, v.from_layer, v.to_layer,
                            drill_nm=v.drill_nm) for v in control.board.vias) is connected
    result = unified.route_unified(board, _guides(board), options)
    assert next(n for n in result.nets if n.net == 'A').connected is connected
    assert not [f for f in run_physical_drc(result.board).findings
                if f.severity.value == 'error' and f.code not in {'DRC-OPEN-NET', 'DRC-ROUTE-INCOMPLETE'}]
    assert result.board.tracks[:len(board.tracks)] == board.tracks
    assert result.board.vias[:len(board.vias)] == board.vias


def test_through_fallback_still_checks_unvisited_layers():
    board = _four_layer((THROUGH, BLIND))
    board = replace(board, copper_keepouts=(
        _keepout('back', (CopperLayer.BACK,), (0, 0, 26, 26)),))
    router = unified._Router(board, unified.UnifiedRouterOptions(), list(board.nets), None)
    masks, _ = router._via_transitions(router.specs[0])
    g = router.grid
    xy = (g.ny // 2) * g.nx + g.nx // 2
    assert masks[0, 1][xy] == unified._FREE  # blind F-In1
    assert masks[0, 2][xy] == unified._FOREIGN  # F-In2 requires F-B through
    assert masks[0, 3][xy] == unified._FOREIGN


def test_blind_via_drill_still_clears_holes_on_other_layers():
    board = _four_layer((THROUGH, BLIND, BACK_BLIND))
    hole = Via('OTHER', Point.mm(13, 13), nm_from_mm('0.6'), nm_from_mm('0.3'),
               CopperLayer('In2.Cu'), CopperLayer.BACK, 'blind-b2')
    board = replace(board, vias=(hole,), nets=(*board.nets, PhysicalNet('OTHER', ())))
    router = unified._Router(board, unified.UnifiedRouterOptions(), list(board.nets), None)
    spec = next(s for s in router.specs if s.name == 'A')
    masks, _ = router._via_transitions(spec)
    g = router.grid
    xy = g.index(0, (hole.position.x_nm - g.x0) // g.pitch, (hole.position.y_nm - g.y0) // g.pitch)
    assert masks[0, 1][xy] == unified._FOREIGN


@pytest.mark.parametrize('covered', [True, False])
def test_return_via_coverage_uses_the_proposed_barrel(covered):
    board = _four_layer((THROUGH, BLIND, BACK_BLIND))
    signal = Point.mm(13, 13)
    returns = Via('RETURN', Point.mm(13, 14), nm_from_mm('0.6'), nm_from_mm('0.3'),
                  FRONT if covered else CopperLayer('In2.Cu'),
                  IN1 if covered else CopperLayer.BACK, 'blind-f1' if covered else 'blind-b2')
    rule = NetRoutingRule('A', require_return_vias=True, return_via_net='RETURN',
                          maximum_return_via_distance_nm=nm_from_mm('1.2'))
    board = replace(board, vias=(returns,), nets=(*board.nets, PhysicalNet('RETURN', ())),
                    net_routing_rules=(rule,))
    router = unified._Router(board, unified.UnifiedRouterOptions(), list(board.nets), None)
    spec = next(s for s in router.specs if s.name == 'A')
    masks, _ = router._via_transitions(spec)
    g = router.grid
    xy = g.index(0, (signal.x_nm - g.x0) // g.pitch, (signal.y_nm - g.y0) // g.pitch)
    assert (masks[0, 1][xy] != unified._FOREIGN) == covered


@pytest.mark.parametrize('pour', [False, True])
def test_guides_survive_every_repair_router(monkeypatch, pour):
    seen = []
    original = unified._Router
    class ObservedRouter(original):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            if self.specs:
                seen.append(bool(self.guides))
    monkeypatch.setattr(unified, '_Router', ObservedRouter)
    board = _walled_plane_pad_board() if pour else _grid_board()
    events = []
    unified.route_unified(board, _guides(board), unified.UnifiedRouterOptions(
        maximum_iterations=10 if pour else 1, commit_rounds=1 if pour else 2, guide_cost=0.1),
        on_progress=lambda phase, event, details: events.append(phase))
    assert len(seen) > 1
    assert all(seen), seen
    assert ('unified_pour_recheck' in events) is pour
