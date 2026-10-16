"""Explicit inner copper must solve blocked surfaces without borrowing pours."""
from dataclasses import replace
from collections import Counter
import pytest
from pcbir.decoupling_paths import propose_crosslayer_decoupling, validate_crosslayer_witness
from pcbir.physical import CopperLayer, CopperKeepout, PolygonWithHoles, PolygonRing, Point
from pcbir.drc import run_physical_drc
from test_decoupling_crosslayer_paths import fixture

def blocked_surfaces(renamed=False, inner=True, via_budget=2):
    board, link = fixture(renamed=renamed)
    wall=PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((10,0),(11,0),(11,20),(10,20)))))
    board=replace(board,copper_keepouts=(CopperKeepout('surface-wall',
        (CopperLayer.FRONT,CopperLayer.BACK),wall,block_tracks=True,block_vias=True,block_zones=True),),
        net_routing_rules=tuple(replace(r,allowed_layers=board.stackup.copper_layers if inner else r.allowed_layers,
                                       max_vias=via_budget) for r in board.net_routing_rules))
    return board,link

@pytest.mark.parametrize('renamed',[False,True])
def test_inner_trace_and_two_vias_complete_both_halves(renamed):
    board,link=blocked_surfaces(renamed)
    result=propose_crosslayer_decoupling(board,link)
    assert result.witness is not None,result.reason
    assert validate_crosslayer_witness(result.board,result.witness)==''
    assert len(result.created_vias)==4
    assert any(t.layer not in (CopperLayer.FRONT,CopperLayer.BACK) for t in result.created_tracks)
    assert not result.board.zones
    assert not [f for f in run_physical_drc(result.board).findings if f.severity.value=='error'
                and f.code not in {'DRC-OPEN-NET','DRC-ROUTE-INCOMPLETE'}]
    # Reusing the admitted original geometry is read-only.
    repeated=propose_crosslayer_decoupling(result.board,link)
    assert repeated.board is result.board and not repeated.created_vias and not repeated.created_tracks

@pytest.mark.parametrize('inner,budget',[(False,2),(True,1)])
def test_source_layer_and_via_limits_cannot_be_relaxed(inner,budget):
    board,link=blocked_surfaces(inner=inner,via_budget=budget)
    result=propose_crosslayer_decoupling(board,link)
    assert result.witness is None and result.board is board
    assert not result.created_vias and not result.created_tracks
