"""Existing source copper is an access graph, independent of root labels."""
from dataclasses import replace
from collections import Counter
import pytest
from pcbir import CopperLayer, Point, TrackSegment, Via, nm_from_mm
from pcbir.physical import NetRoutingRule, RouteKind, CopperKeepout, PolygonWithHoles, PolygonRing
from pcbir.drc import explicit_copper_connectivity
from test_unified_router import _board, _route


def existing_exits(*, blind=False):
    board = _board({'S': ((2, 10), (18, 10))}, layers=4 if blind else 2)
    tracks = tuple(TrackSegment('S', Point.mm(x, 13), Point.mm(x, 12),
                               nm_from_mm('0.2'), CopperLayer.FRONT) for x in (5, 21))
    vias = tuple(Via('S', Point.mm(x, 12), nm_from_mm('0.6'), nm_from_mm('0.3'),
                     from_layer=CopperLayer.FRONT,
                     to_layer=CopperLayer('In1.Cu') if blind else CopperLayer.BACK)
                 for x in (5, 21))
    barrier = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((12, 0), (14, 0), (14, 26), (12, 26)))))
    rule = NetRoutingRule('S', RouteKind.GENERAL, width_nm=nm_from_mm('0.2'), max_vias=2,
                          allowed_layers=(CopperLayer.FRONT, CopperLayer.BACK))
    return replace(board, tracks=tracks, vias=vias, net_routing_rules=(rule,),
                   copper_keepouts=(CopperKeepout('front-wall', (CopperLayer.FRONT,), barrier,
                        block_pads=False, block_tracks=True, block_vias=True, block_zones=False),))


@pytest.mark.parametrize('native', (False, True))
def test_existing_pad_rooted_exits_are_reused_without_new_vias(monkeypatch, native):
    if not native:
        import pcbir.native
        monkeypatch.setattr(pcbir.native, 'astar', lambda: None)
    board = existing_exits()
    graph = explicit_copper_connectivity(board)
    for i in range(2):
        assert graph.roots[f'via:{i}'].startswith('pad:')
    result, drc, summary = _route(board, maximum_iterations=8, commit_rounds=2)
    assert all(net.connected for net in result.nets)
    assert summary['signal']['opens'] == 0
    assert summary['drc']['errors'] == 0
    assert Counter(board.tracks) <= Counter(result.board.tracks)
    assert Counter(board.vias) == Counter(result.board.vias)


def test_existing_blind_vias_do_not_create_back_layer_anchors():
    board = existing_exits(blind=True)
    result, drc, summary = _route(board, maximum_iterations=8, commit_rounds=2)
    assert not all(net.connected for net in result.nets)
    assert summary['signal']['opens'] == 1
    assert not any(t.layer is CopperLayer.BACK for t in result.board.tracks)
    assert Counter(board.tracks) <= Counter(result.board.tracks)
    assert Counter(board.vias) == Counter(result.board.vias)
