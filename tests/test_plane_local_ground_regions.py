"""Authored local ground regions keep adjacent package approaches available."""
from dataclasses import replace
import pytest
from pcbir import (BoardOutline, CopperKeepout, CopperLayer, CopperZone, DesignRules, FootprintPad,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point,
    PolygonRing, PolygonWithHoles, Size, Stackup, Via, nm_from_mm)
from pcbir.placement import transformed_local_point
from pcbir.plane import PlaneStitchOptions, stitch_zone_pads
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.drc import run_physical_drc


def scene(rotation=0, name="U"):
    pads = tuple(FootprintPad(str(i), Point.mm(x, -3), Size.mm(".25", "1.9"))
        for i, x in enumerate(("-1.5", "-1", "-.5", "0", ".5", "1", "1.5"), 1))
    fp = PhysicalFootprint("interleaved", pads, Size.mm(8, 8))
    pose = Placement(name, fp.name, Point.mm(10, 10), rotation_degrees=rotation)
    def polygon(points):
        return PolygonWithHoles(PolygonRing(tuple(transformed_local_point(pose, Point.mm(x, y))
            for x, y in points)))
    plane = CopperZone("plane", "GND", (CopperLayer.INTERNAL_1,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in ((1,1),(19,1),(19,19),(1,19))))))
    local = CopperZone("local-return", "GND", (CopperLayer.FRONT, CopperLayer.BACK),
        polygon(((-2,-1.8),(2,-1.8),(2,2),(-2,2))), reserve_routing=True)
    ground = PhysicalNet("GND", tuple(PadReference(name, str(i)) for i in (2,4,6)))
    signals = tuple(PhysicalNet(f"S{i}", (PadReference(name, str(i)),)) for i in (1,3,5,7))
    held = Via("GND", Point.mm(17,17), 450_000, 200_000)
    board = PhysicalBoard("LocalReturns", BoardOutline.rectangle(20,20), {fp.name:fp}, (pose,),
        (ground, *signals), stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK)), zones=(plane,local), vias=(held,),
        rules=DesignRules(default_track_width_nm=200_000, minimum_clearance_nm=200_000,
            default_via_size_nm=450_000, default_via_drill_nm=200_000))
    return board, pose


@pytest.mark.parametrize("rotation,name", ((0,"U"),(90,"RENAMED"),(180,"ADC_FREE"),(270,"OTHER")))
def test_local_region_keeps_individual_returns_behind_interleaved_signal_lands(rotation,name):
    board, pose = scene(rotation,name)
    result = stitch_zone_pads(board, PlaneStitchOptions(include_surface_zones=True))
    assert result.complete and result.added_via_count == 3
    # The explicit region, rather than a package or net-specific direction,
    # supplies the return topology. Every foreign signal retains its approach.
    index = RoutingClearanceIndex(result.board)
    for i, x in ((1,"-1.5"),(3,"-.5"),(5,".5"),(7,"1.5")):
        start, end = (transformed_local_point(pose,Point.mm(x,y)) for y in (-3,-6))
        assert index.can_track(f"S{i}",start,end,200_000,CopperLayer.FRONT)
    assert result.board.vias[:1] == board.vias
    assert len({use.vias for use in result.copper_uses}) == 3
    assert all(use.kind == "contact" for use in result.copper_uses)
    assert not [f for f in run_physical_drc(result.board).findings if f.severity.value == "error"
                and f.code not in {"DRC-OPEN-NET","DRC-ROUTE-INCOMPLETE"}]
    repeated = stitch_zone_pads(result.board, PlaneStitchOptions(include_surface_zones=True))
    assert repeated.board.tracks == result.board.tracks
    assert repeated.board.vias == result.board.vias


def test_unreserved_or_distant_surface_region_does_not_rewrite_default_via_ranking():
    board, _ = scene()
    legacy = replace(board,zones=board.zones[:1])
    baseline = stitch_zone_pads(legacy,PlaneStitchOptions(include_surface_zones=True))
    for region in (replace(board.zones[1],reserve_routing=False),
                   replace(board.zones[1],outline=PolygonWithHoles(PolygonRing(tuple(
                       Point.mm(x,y) for x,y in ((15,15),(18,15),(18,18),(15,18))))))):
        result = stitch_zone_pads(replace(board,zones=(board.zones[0],region)),
                                 PlaneStitchOptions(include_surface_zones=True))
        assert result.board.tracks == baseline.board.tracks
        assert result.board.vias == baseline.board.vias


def test_unreachable_local_region_keeps_legal_outside_contacts_available():
    board, _ = scene()
    barrier = CopperKeepout("blocked-local-approach",(CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in
            ((2,"8.1"),(18,"8.1"),(18,"8.7"),(2,"8.7"))))),block_pads=False)
    board = replace(board,copper_keepouts=(barrier,))
    result = stitch_zone_pads(board,PlaneStitchOptions(include_surface_zones=True))
    assert result.complete and result.added_via_count == 3
    assert all(v.position.y_nm < 7_000_000 for v in result.board.vias[1:])
    assert result.board.vias[:1] == board.vias


@pytest.mark.parametrize("rotation", (0,90,180,270))
def test_equal_cost_passive_returns_prefer_departure_away_from_signal_land(rotation):
    board, _ = scene()
    fp = PhysicalFootprint("passive", (
        FootprintPad("1",Point.mm(0,".775"),Size.mm(".85",".95")),
        FootprintPad("2",Point.mm(0,"-.775"),Size.mm(".85",".95"))), Size.mm("1.46","2.96"))
    pose = Placement("PASSIVE",fp.name,Point.mm(10,10),rotation_degrees=rotation)
    board = replace(board,footprints={fp.name:fp},placements=(pose,),
        nets=(PhysicalNet("GND",(PadReference("PASSIVE","2"),)),
              PhysicalNet("INPUT",(PadReference("PASSIVE","1"),))),zones=board.zones[:1],vias=())
    result = stitch_zone_pads(board)
    assert result.complete and result.added_via_count == 1
    start = transformed_local_point(pose,fp.pads[1].position)
    outward = transformed_local_point(pose,Point.mm(0,"-1.775"))
    assert result.board.vias[0].position == outward
    assert result.board.tracks[0].start == start
