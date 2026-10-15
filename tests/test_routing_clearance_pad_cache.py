"""Immutable pad geometry sharing never shares current clearance decisions."""
from dataclasses import replace
import json
import time

import pytest

from pcbir import (BoardOutline, BoardSide, CopperLayer, FootprintPad, PadKind,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
    Point, Size, Stackup, TrackSegment, Via)
from pcbir.physical import NetRoutingRule, PadShape
from pcbir.geometry import RoundedConvexShape
from pcbir import routing_clearance as module
from pcbir.breakout import BreakoutRegions, _nearest_region
from fractions import Fraction
from random import Random


@pytest.fixture(autouse=True)
def empty_geometry_caches():
    module._pad_index_template.cache_clear()
    module._posed_pad_geometry.cache_clear()
    yield
    module._pad_index_template.cache_clear()
    module._posed_pad_geometry.cache_clear()


def fixture(count=2):
    fp = PhysicalFootprint("mixed", (
        FootprintPad("1", Point.mm(-.7, 0), Size.mm(.5, .6)),
        FootprintPad("NC", Point.mm(.7, 0), Size(500001, 600003),
                     shape=PadShape.ROUNDRECT, roundrect_ratio_ppm=170001),
        FootprintPad("T", Point.mm(0, 1), Size.mm(.8, .8),
                     kind=PadKind.THROUGH_HOLE, drill=Size.mm(.4, .4)),
        FootprintPad("", Point.mm(0, -1), Size.mm(.6, .9),
                     kind=PadKind.NON_PLATED_THROUGH_HOLE, drill=Size.mm(.6, .9))), Size.mm(3, 3))
    poses=tuple(Placement(f"P{i}", fp.name, Point.mm(5+5*(i%8), 5+5*(i//8)),
                           rotation_degrees="17.3" if i%2 else 90,
                           side=BoardSide.BACK if i%2 else BoardSide.FRONT) for i in range(count))
    return PhysicalBoard("pad-cache", BoardOutline.rectangle(50, 50), {fp.name:fp}, poses,
        (PhysicalNet("A",tuple(PadReference(p.reference,"1") for p in poses)),
         PhysicalNet("B",tuple(PadReference(p.reference,"T") for p in poses))),
        stackup=Stackup((CopperLayer.FRONT,CopperLayer.INTERNAL_1,CopperLayer.INTERNAL_2,
                         CopperLayer.INTERNAL_3,CopperLayer.INTERNAL_4,CopperLayer.BACK)))


def test_template_shares_original_geometry_but_each_index_owns_reservations():
    board=fixture();first=module.RoutingClearanceIndex(board)
    track=TrackSegment("A",Point.mm(25,25),Point.mm(30,25),100000,CopperLayer.FRONT)
    via=Via("B",Point.mm(35,35),450000,200000)
    other=replace(board,tracks=(track,track),vias=(via,via))
    second=module.RoutingClearanceIndex(other)
    assert module._pad_index_template.cache_info().hits==1
    assert first._objects[0] is second._objects[0]
    assert first._objects is not second._objects and first._holes is not second._holes
    assert first._bins is not second._bins
    assert all(first._bins[key] is not second._bins[key] for key in first._bins)
    assert second.additions()==((track,True),(track,True),(via,True),(via,True))
    assert all(t is track for t,_ in second.additions()[:2])
    assert not second.can_track("B",Point.mm(26,25),Point.mm(29,25),100000,CopperLayer.FRONT)
    assert first.can_track("B",Point.mm(26,25),Point.mm(29,25),100000,CopperLayer.FRONT)
    first.add_track(track,locked=False)
    first.add_via(via,locked=False)
    third=module.RoutingClearanceIndex(board)
    assert third.additions()==() and second.additions()==((track,True),(track,True),(via,True),(via,True))
    assert third.can_track("B",Point.mm(26,25),Point.mm(29,25),100000,CopperLayer.FRONT)


def test_all_original_pads_and_pth_holes_match_direct_geometry_including_nc():
    board=fixture();index=module.RoutingClearanceIndex(board)
    direct=[]
    for pose in sorted(board.placements,key=lambda p:p.reference):
        for i,pad in enumerate(board.footprints[pose.footprint].pads):
            if pad.kind in {PadKind.NON_PLATED_THROUGH_HOLE,PadKind.APERTURE}:continue
            position=module.transformed_local_point(pose,pad.position)
            direct.append((PadReference(pose.reference,pad.number),
                           module.placed_pad_shape(position,pad,pose)))
    pads=[o for o in index._objects if o.is_pad]
    assert [(o.pad_reference,o.shape) for o in pads]==direct
    assert [o.net for o in pads if o.pad_reference.pad=="NC"]==[
        "<unconnected:P0.NC:1>","<unconnected:P1.NC:1>"]
    assert len(index._holes)==2
    assert all(h.net=="B" and h.radius_nm==200000 and h.locked for h in index._holes)
    assert len([o for o in index._objects if o.net=="<non-plated-hole>"])==2


@pytest.mark.parametrize("change",["pose","rotation","side","footprint","net","rule","budget","stackup","bins"])
def test_source_epoch_changes_invalidate_template_and_keep_queries_fresh(change):
    board=fixture();old=module._pad_template_for(board,2000000)
    changed=board;bin_size=2000000
    if change in {"pose","rotation","side"}:
        kwargs={"pose":{"position":Point.mm(25,25)},"rotation":{"rotation_degrees":45},
                "side":{"side":BoardSide.BACK}}[change]
        changed=replace(board,placements=(replace(board.placements[0],**kwargs),board.placements[1]))
    elif change=="footprint":
        fp=board.footprints['mixed'];changed=replace(board,footprints={fp.name:replace(fp,
            pads=(replace(fp.pads[0],size=Size.mm(2,2)),*fp.pads[1:]))})
    elif change=="net":
        changed=replace(board,nets=tuple(replace(n,name="renamed" if n.name=="A" else n.name) for n in board.nets))
    elif change=="rule":changed=replace(board,rules=replace(board.rules,minimum_clearance_nm=800000))
    elif change=="budget":changed=replace(board,net_routing_rules=(NetRoutingRule("A",max_vias=0,max_length_nm=1000000),))
    elif change=="stackup":changed=replace(board,stackup=Stackup())
    else:bin_size=500000
    current=module._pad_template_for(changed,bin_size)
    assert current is not old
    fresh=module.RoutingClearanceIndex(changed,bin_size)
    # Clearing both caches gives the same full index/query outputs: no old
    # source policy, ownership or generated copper is a cache authority.
    module._pad_index_template.cache_clear();module._posed_pad_geometry.cache_clear()
    cold=module.RoutingClearanceIndex(changed,bin_size)
    assert (fresh._objects,fresh._holes,fresh._bins,fresh.additions())==(
        cold._objects,cold._holes,cold._bins,cold.additions())
    for x in (3,5,6,10,25):
        for layer in (CopperLayer.FRONT,CopperLayer.BACK):
            args=("B",Point.mm(x,5),Point.mm(x+.2,5),100000,layer)
            assert fresh.can_track(*args)==cold.can_track(*args)
            assert fresh.track_blocker_records(TrackSegment(*args))==cold.track_blocker_records(TrackSegment(*args))


def test_rule_change_rechecks_current_exact_clearance_after_geometry_hit():
    board=fixture(1)
    probe=("B",Point.mm(5.8,5.7),Point.mm(5.8,5.8),100000,CopperLayer.FRONT)
    assert module.RoutingClearanceIndex(board).can_track(*probe)
    strict=replace(board,net_routing_rules=(NetRoutingRule("A",clearance_nm=1000000),))
    assert not module.RoutingClearanceIndex(strict).can_track(*probe)
    assert module._posed_pad_geometry.cache_info().hits


def test_breakout_owner_is_fresh_even_when_pad_geometry_is_shared():
    board=fixture(1)
    plain=module.RoutingClearanceIndex(board)
    changed=replace(board,net_routing_rules=(NetRoutingRule("A",width_nm=300000,
        breakout_length_nm=1000000,breakout_width_nm=200000,breakout_clearance_nm=200000),))
    current=module.RoutingClearanceIndex(changed)
    assert plain._objects[0].breakout_land is None
    assert current._objects[0].breakout_land=="pad:P0.1:0"
    assert plain.breakout is not current.breakout
    assert plain.breakout._region_cache is not current.breakout._region_cache


def test_current_keepout_is_not_part_of_the_shared_pad_template():
    from test_static_clearance import rectangle
    board=fixture();first=module.RoutingClearanceIndex(board)
    blocked=module.RoutingClearanceIndex(replace(board,copper_keepouts=(rectangle("new",25,25,2,2),)))
    probe=("A",Point.mm(25.5,25.5),Point.mm(26,25.5),100000,CopperLayer.FRONT)
    assert first.can_track(*probe) and not blocked.can_track(*probe)
    assert module._pad_index_template.cache_info().hits


def test_bounded_geometry_work_across_copper_snapshots(monkeypatch,tmp_path):
    board=fixture(32);original=module.placed_pad_shape;calls=[]
    def measured(*args):calls.append(1);return original(*args)
    monkeypatch.setattr(module,"placed_pad_shape",measured)
    start=time.perf_counter()
    for i in range(24):
        track=TrackSegment("A",Point.mm(25,25),Point.mm(25+(i+1)/100,25),100000,CopperLayer.FRONT)
        index=module.RoutingClearanceIndex(replace(board,tracks=(track,)))
        assert index.additions()==((track,True),)
    elapsed=time.perf_counter()-start
    assert len(calls)==96  # 32*three copper pads, once, not 24 times.
    assert module._pad_index_template.cache_info().hits==23
    moved=replace(board,placements=(replace(board.placements[0],position=Point.mm(6,6)),*board.placements[1:]))
    module.RoutingClearanceIndex(moved)
    assert len(calls)==99  # Only the one moved footprint is transformed again.
    report={"snapshot_count":24,"component_count":32,"copper_pad_count":96,
            "shape_constructions":96,"uncached_shape_constructions":24*96,
            "elapsed_seconds":elapsed,"timing_is_fixture_only":True}
    (tmp_path/'pad-template-work.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
    print(json.dumps(report))


def test_cache_bounds_and_strong_original_identity():
    board=fixture();template=module._pad_template_for(board,2000000)
    assert module._pad_index_template.cache_parameters()['maxsize']==8
    assert module._posed_pad_geometry.cache_parameters()['maxsize']==512
    equal_but_new=replace(board,placements=tuple(replace(p) for p in board.placements))
    assert module._pad_template_for(equal_but_new,2000000) is not template
    assert module._pad_template_for(board,2000000) is template


def test_materialized_macro_prefix_is_indexed_fresh_after_template_hit(tmp_path):
    from test_hard_macros import fixture as macro_fixture
    from pcbir.hard_macros import materialize_hard_macros
    _,_,bind=macro_fixture(tmp_path)
    board=materialize_hard_macros(bind())
    first=module.RoutingClearanceIndex(board)
    extra=TrackSegment(board.nets[0].name,Point.mm(25,25),Point.mm(26,25),200000,CopperLayer.FRONT)
    current=module.RoutingClearanceIndex(replace(board,tracks=(*board.tracks,extra)))
    assert module._pad_index_template.cache_info().hits
    assert current.additions()==tuple((item,True) for item in (*board.tracks,extra,*board.vias))
    assert all(actual is original for (actual,_),original in zip(current.additions(),(*board.tracks,extra,*board.vias)))
    assert first._macro_regions==current._macro_regions
    assert first._static_regions==current._static_regions
    assert first._static_bins is not current._static_bins


def test_required_array_occurrences_remain_current_original_vias():
    from test_decoupling_crosslayer_paths import fixture as crosslayer_fixture
    from pcbir.physical import CopperZone,PolygonRing,PolygonWithHoles,PadViaInPadRule
    from pcbir.hard_macros import materialize_hard_macros
    board,link=crosslayer_fixture()
    chip=board.footprints['chip']
    chip=replace(chip,pads=tuple(replace(p,size=Size.mm(2,2)) if p.number=='G' else p for p in chip.pads))
    board=replace(board,footprints={**board.footprints,'chip':chip},metadata={'fabrication_profile':'jlcpcb-six-layer'},
        nets=tuple(replace(n,name='GND') if n.name==link.return_net else n for n in board.nets),
        net_routing_rules=tuple(replace(r,net='GND') if r.net==link.return_net else r for r in board.net_routing_rules),
        decoupling_links=(replace(link,return_net='GND'),),
        zones=(CopperZone('ground','GND',(CopperLayer.INTERNAL_1,),PolygonWithHoles(PolygonRing((
            Point.mm(1,1),Point.mm(24,1),Point.mm(24,19),Point.mm(1,19))))),),
        via_in_pad_rules=(PadViaInPadRule(PadReference(link.target.component,'G'),rows=1,columns=2),))
    frame=materialize_hard_macros(board)
    assert len(frame.vias)==2
    module.RoutingClearanceIndex(board)
    index=module.RoutingClearanceIndex(frame)
    assert module._pad_index_template.cache_info().hits
    assert index.additions()==tuple((v,True) for v in frame.vias)
    assert all(actual is expected for (actual,_),expected in zip(index.additions(),frame.vias))
    assert len(index._holes)==2 and all(h.reusable for h in index._holes)


def linear_region(lands,points):
    """Original all-lands nearest/strict-first-tie oracle, with no early exit."""
    best=None
    for land in lands:
        farthest=Fraction(0)
        for point in points:
            distance=land.distance_squared(point)
            if distance is None:break
            farthest=max(farthest,distance)
        else:
            if best is None or farthest < best[0]:best=(farthest,land.identity)
    return None if best is None else best[1]


@pytest.mark.parametrize('renamed',[False,True])
@pytest.mark.parametrize('length',[1,1000000])
def test_all_pad_owners_match_fresh_regions_and_linear_oracle(renamed,length):
    name='RENAMED_SOURCE' if renamed else 'A'
    board=fixture(5)
    board=replace(board,nets=tuple(replace(n,name=name) if n.name=='A' else n for n in board.nets),
        net_routing_rules=(NetRoutingRule(name,width_nm=300000,
            breakout_length_nm=length,breakout_width_nm=200000),))
    index=module.RoutingClearanceIndex(board)
    regions=BreakoutRegions(board)
    for item in index._objects:
        if item.is_pad:
            expected=linear_region(regions._lands.get(item.net,()),item.shape.spine)
            assert item.breakout_land==expected==regions.region(item.net,item.shape.spine)


def test_nearest_region_first_zero_stops_and_preserves_strict_first_tie(monkeypatch):
    from pcbir.breakout import BreakoutLand
    from pcbir.geometry import Bounds
    shape=RoundedConvexShape((Point(0,0),))
    lands=tuple(BreakoutLand(f'land{i}','N',shape,100,Bounds(-100,-100,100,100)) for i in range(40))
    original=BreakoutLand.distance_squared;calls=[]
    def count(land,point):calls.append(land.identity);return original(land,point)
    monkeypatch.setattr(BreakoutLand,'distance_squared',count)
    assert _nearest_region(lands,(Point(0,0),))=='land0'
    assert calls==['land0']
    calls.clear()
    assert linear_region(lands,(Point(0,0),))=='land0' and len(calls)==40
    # Equal nonzero scores retain the same first land; these cannot early exit.
    calls.clear()
    assert _nearest_region(lands,(Point(1,0),))=='land0' and len(calls)==40
    assert _nearest_region(lands,())=='land0'


def test_changed_other_land_invalidates_owner_even_when_queried_pad_stays_fixed():
    fp=PhysicalFootprint('single',(FootprintPad('1',Point(0,0),Size.mm(.5,.5)),),Size.mm(1,1))
    board=PhysicalBoard('other-land',BoardOutline.rectangle(40,30),{fp.name:fp},
        (Placement('A1',fp.name,Point.mm(10,10)),Placement('B2',fp.name,Point.mm(20,10))),
        (PhysicalNet('N',(PadReference('A1','1'),PadReference('B2','1'))),),
        net_routing_rules=(NetRoutingRule('N',width_nm=300000,breakout_length_nm=1,breakout_width_nm=200000),))
    old=module.RoutingClearanceIndex(board)
    assert old._objects[1].breakout_land=='pad:B2.1:0'
    moved=replace(board,placements=(replace(board.placements[0],position=Point.mm(20,10)),board.placements[1]))
    current=module.RoutingClearanceIndex(moved)
    assert moved.placements[1] is board.placements[1]
    assert current._objects[1].shape is old._objects[1].shape
    assert current._objects[1].breakout_land=='pad:A1.1:0'
    assert current._objects[1].breakout_land==BreakoutRegions(moved).region('N',current._objects[1].shape.spine)


@pytest.mark.parametrize('side',list(BoardSide))
@pytest.mark.parametrize('shape',list(PadShape))
def test_nearest_oracle_rotated_odd_nm_duplicate_numbers_and_shapes(side,shape):
    fp=PhysicalFootprint('odd',tuple(FootprintPad('1',Point(i*123457,i*119357),
        Size(500003,700001),shape=shape,rotation_degrees='17.3') for i in range(3)),Size.mm(2,2))
    board=PhysicalBoard('odd-shapes',BoardOutline.rectangle(40,30),{fp.name:fp},
        (Placement('R',fp.name,Point(10000003,10000007),'37.15',side),),
        (PhysicalNet('N',(PadReference('R','1'),)),),
        net_routing_rules=(NetRoutingRule('N',width_nm=300000,breakout_length_nm=1000000,breakout_width_nm=200000),))
    regions=BreakoutRegions(board);index=module.RoutingClearanceIndex(board)
    for item in index._objects:
        assert item.breakout_land==linear_region(regions._lands['N'],item.shape.spine)
    rng=Random(19)
    for _ in range(40):
        points=tuple(Point(10000003+rng.randrange(-1800000,1800001),
                           10000007+rng.randrange(-1800000,1800001)) for _ in range(rng.randrange(4)))
        assert regions.region('N',points)==linear_region(regions._lands['N'],points)


def test_owner_geometry_work_is_once_per_complete_source_epoch(monkeypatch):
    board=fixture(12)
    board=replace(board,net_routing_rules=(NetRoutingRule('A',width_nm=300000,
        breakout_length_nm=1000000,breakout_width_nm=200000),))
    original=module._nearest_region;calls=[]
    def measure(*args):calls.append(1);return original(*args)
    monkeypatch.setattr(module,'_nearest_region',measure)
    first=module.RoutingClearanceIndex(board)
    assert len(calls)==12
    for i in range(8):
        track=TrackSegment('B',Point.mm(40,40),Point.mm(41+i/10,40),200000,CopperLayer.FRONT)
        current=module.RoutingClearanceIndex(replace(board,tracks=(track,)))
        assert current._objects[:36]==first._objects[:36]
        assert current.breakout is not first.breakout
    assert len(calls)==12
    changed=replace(board,placements=(replace(board.placements[-1],position=Point.mm(45,45)),*board.placements[:-1]))
    module.RoutingClearanceIndex(changed)
    assert len(calls)==24


def test_zero_length_source_rule_remains_rejected_by_existing_contract():
    with pytest.raises(ValueError,match='breakout length must be positive'):
        NetRoutingRule('N',width_nm=300000,breakout_length_nm=0,breakout_width_nm=200000)


def test_duplicate_rule_cannot_silently_remove_a_declared_land_in_raw_ir():
    board=fixture(1)
    declared=NetRoutingRule('A',width_nm=300000,breakout_length_nm=1000000,breakout_width_nm=200000)
    with pytest.raises(ValueError,match='multiple routing rules'):
        replace(board,net_routing_rules=(declared,NetRoutingRule('A')))


@pytest.mark.parametrize('same_net',[False,True])
def test_duplicate_pad_net_membership_is_rejected_before_index_construction(same_net):
    board=fixture(1);pad=PadReference('P0','1')
    nets=(replace(board.nets[0],pads=(*board.nets[0].pads,pad)),board.nets[1]) if same_net else (
        *board.nets,PhysicalNet('C',(pad,)))
    with pytest.raises(ValueError,match='belongs to both'):
        replace(board,nets=nets)
