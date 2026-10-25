"""Whole-net via limits include retained source launch copper."""
from collections import Counter
from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, DetailedRouterOptions,
                   FootprintPad, NetRoutingRule, PadReference, PhysicalBoard,
                   PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing,
                   PolygonWithHoles, Size, TrackSegment, Via, nm_from_mm,
                   route_detailed, route_global, run_physical_drc)


@pytest.mark.parametrize("budget,connected", [(2, False), (4, True)])
def test_retained_launches_consume_the_via_budget(budget, connected):
    footprint = PhysicalFootprint("test/one", (FootprintPad("1", Point(0, 0),
        Size.mm("0.6", "0.6")),), Size.mm(1, 1))
    tracks = tuple(TrackSegment("S", Point.mm(x, 13), Point.mm(x, 12),
        nm_from_mm("0.2"), CopperLayer.FRONT) for x in (5, 21))
    vias = tuple(Via("S", Point.mm(x, 12), nm_from_mm("0.6"), nm_from_mm("0.3")) for x in (5, 21))
    barrier = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((12, 0), (14, 0), (14, 26), (12, 26)))))
    board = PhysicalBoard("ViaBudget", BoardOutline.rectangle(26, 26),
        {footprint.name: footprint},
        (Placement("A", footprint.name, Point.mm(5, 13)), Placement("B", footprint.name, Point.mm(21, 13))),
        (PhysicalNet("S", (PadReference("A", "1"), PadReference("B", "1"))),),
        tracks=tracks, vias=vias,
        net_routing_rules=(NetRoutingRule("S", width_nm=nm_from_mm("0.2"), max_vias=budget,
                                         allowed_layers=(CopperLayer.FRONT, CopperLayer.BACK)),),
        copper_keepouts=(CopperKeepout("wall", (CopperLayer.FRONT,), barrier,
            block_pads=False, block_tracks=True, block_vias=True, block_zones=False),))
    guide = route_global(replace(board, tracks=(), vias=()))
    result = route_detailed(board, guide, DetailedRouterOptions(maximum_passes=1, maximum_search_states=10000))
    assert result.nets[0].connected is connected
    assert Counter(board.tracks) <= Counter(result.board.tracks)
    assert Counter(board.vias) <= Counter(result.board.vias)
    assert len(result.board.vias) <= budget
    assert not any(f.code == "DRC-MAX-VIAS" for f in run_physical_drc(result.board).findings)
    if not connected:
        assert result.board.tracks == board.tracks and result.board.vias == board.vias
        assert "via count 4 exceeds 2" in " ".join(result.nets[0].diagnostics)
