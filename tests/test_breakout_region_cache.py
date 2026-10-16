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



def test_copper_only_trial_indexes_reuse_source_membership(monkeypatch):
    import pcbir.breakout as module
    board = fixture()
    calls = []
    original = module._nearest_region
    def measured(*args):
        calls.append(1)
        return original(*args)
    monkeypatch.setattr(module, '_nearest_region', measured)
    points = (Point.mm(9,8), Point.mm(10,8))
    first = BreakoutRegions(board).region('P', points)
    changed = replace(board, tracks=(TrackSegment('G', Point.mm(9.5,7),
        Point.mm(9.5,9), 200000, CopperLayer.FRONT),))
    assert BreakoutRegions(changed).region('P', points) == first
    assert len(calls) == 1
    # Membership reuse must never reuse a previous copper-clearance answer.
    assert RoutingClearanceIndex(board).can_track('P', *points, 200000, CopperLayer.FRONT)
    assert not RoutingClearanceIndex(changed).can_track('P', *points, 200000, CopperLayer.FRONT)


def test_changed_source_epochs_recompute_exact_membership(monkeypatch):
    import pcbir.breakout as module
    from pcbir.physical import Size, Stackup
    board = fixture(); points = (Point.mm(9,8), Point.mm(10,8))
    calls = []
    original = module._nearest_region
    def measured(*args):
        calls.append(1)
        return original(*args)
    monkeypatch.setattr(module, '_nearest_region', measured)
    BreakoutRegions(board).region('P', points)
    footprint = board.footprints[board.placements[0].footprint]
    changed_footprint = replace(footprint, pads=tuple(replace(p, size=Size.mm('.1','.1')) for p in footprint.pads))
    variants = (
        replace(board, placements=tuple(replace(p, position=Point.mm(20,20)) if p.reference=='U' else p for p in board.placements)),
        replace(board, footprints={**board.footprints, footprint.name:changed_footprint}),
        replace(board, net_routing_rules=tuple(replace(r, breakout_length_nm=1) for r in board.net_routing_rules)),
        replace(board, rules=replace(board.rules, minimum_clearance_nm=board.rules.minimum_clearance_nm+1)),
        replace(board, nets=tuple(replace(n, name='G' if n.name=='P' else 'P') for n in board.nets), decoupling_links=()),
        replace(board, stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK))),
    )
    for variant in variants:
        index = BreakoutRegions(variant)
        before = len(calls)
        assert index.region('P', points) == original(index._lands['P'], points)
        assert len(calls) == before+1
        index.region('P', points)
        assert len(calls) == before+1



def test_shared_source_epoch_storage_is_bounded():
    import pcbir.breakout as module
    module._source_region_cache.cache_clear()
    for _ in range(9):
        index = BreakoutRegions(fixture())
        index.region('P', (Point.mm(9,8),))
    assert module._source_region_cache.cache_info().maxsize == 4
    assert module._source_region_cache.cache_info().currsize == 4
