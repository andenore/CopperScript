"""Static region caching must never cache mutable copper legality."""
from dataclasses import replace
from pcbir.breakout import BreakoutRegions
from pcbir.physical import Point, NetRoutingRule, TrackSegment, CopperLayer
from pcbir.routing_clearance import RoutingClearanceIndex
from test_routing_quality import bypass_board

def fixture():
    b=bypass_board()
    return replace(b,net_routing_rules=(NetRoutingRule('P',width_nm=250000,
        breakout_length_nm=3000000,breakout_width_nm=200000),))

def test_source_region_answers_reused_and_new_poses_get_a_fresh_index():
    board=fixture(); index=BreakoutRegions(board)
    points=(Point.mm(9,8),Point.mm(10,8))
    before=index.region('P',points)
    assert before and index.region('P',points)==before
    assert index.region('P',(Point.mm(0,0),)) is None
    assert index.region('P',(Point.mm(0,0),)) is None
    moved=replace(board,placements=tuple(replace(p,position=Point.mm(20,20)) if p.reference=='U' else p for p in board.placements))
    assert BreakoutRegions(moved).region('P',points) is None

def test_static_region_cache_does_not_hide_a_new_foreign_track():
    board=fixture(); index=RoutingClearanceIndex(board)
    a,b=Point.mm(9,8),Point.mm(10,8)
    assert index.can_track('P',a,b,200000,CopperLayer.FRONT)
    index.add_track(TrackSegment('G',Point.mm(9.5,7),Point.mm(9.5,9),200000,CopperLayer.FRONT),locked=True)
    assert not index.can_track('P',a,b,200000,CopperLayer.FRONT)

def test_source_region_storage_stays_bounded():
    index=BreakoutRegions(fixture())
    for i in range(8300): index.region('P',(Point(i,0),))
    assert len(index._region_cache)<=8192
