"""A pin whose net may not change layer escapes on its own surface (plan R13)."""
from dataclasses import replace

import pytest

import pcbir.fanout as fanout
from pcbir import (BoardOutline, CopperLayer, DesignRules, FootprintPad, NetRoutingRule, PadReference,
    PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size, Stackup, nm_from_mm)
from pcbir.boundary_access import BoundaryAccessOptions, package_collar
from pcbir.drc import run_physical_drc
from pcbir.physical import CopperKeepout, PolygonRing, PolygonWithHoles
from pcbir.pin_escape import verified_routing_access
from pcbir.routing import GlobalRouterOptions, GlobalRoutingStatus, route_global
from pcbir.routing_clearance import RoutingClearanceIndex

PIN = PadReference("U", "1")
FOUR = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)


def qfn():
    """A 4 mm, 0.5 mm-pitch QFN-24; pin 1 is next to a package corner."""
    lands = []
    for side in range(4):
        for i in range(6):
            t = (i - 2.5) * .5
            x, y, w, h = ((-1.9, t, .7, .25), (t, 1.9, .25, .7), (1.9, -t, .7, .25), (-t, -1.9, .25, .7))[side]
            lands.append(FootprintPad(str(len(lands) + 1), Point.mm(x, y), Size.mm(w, h)))
    lands.append(FootprintPad("25", Point(0, 0), Size.mm(2.4, 2.4)))
    return PhysicalFootprint("qfn24", tuple(lands), Size.mm(4, 4))


def board(**rule):
    """Crystal-like net X1 from pin 1 to Y; pins 2-7 carry ordinary two-layer nets."""
    one = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm(.6, .6)),), Size.mm(1, 1))
    poses = [Placement("U", "qfn24", Point.mm(12, 10)), Placement("Y", "one", Point.mm(6, 7))]
    nets = [PhysicalNet("X1", (PIN, PadReference("Y", "1")))]
    for k, pin in enumerate(range(2, 8)):
        poses.append(Placement(f"J{pin}", "one", Point.mm(4 + k * 1.5, 16)))
        nets.append(PhysicalNet(f"N{pin}", (PadReference("U", str(pin)), PadReference(f"J{pin}", "1"))))
    return PhysicalBoard("surface escape", BoardOutline.rectangle(24, 20),
        {"qfn24": qfn(), "one": one}, tuple(poses), tuple(nets),
        net_routing_rules=(NetRoutingRule("X1", **rule),), stackup=Stackup(copper_layers=FOUR),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm(".1"), minimum_track_width_nm=nm_from_mm(".1"),
                          default_track_width_nm=nm_from_mm(".15"), default_via_size_nm=nm_from_mm(".45"),
                          default_via_drill_nm=nm_from_mm(".2"), minimum_hole_clearance_nm=nm_from_mm(".2")))


ONE_LAYER = dict(allowed_layers=(CopperLayer.FRONT,))
# The crystal constraint that left two QFNs with pending pins.
CRYSTAL = dict(allowed_layers=(CopperLayer.FRONT,), max_vias=0, clearance_nm=nm_from_mm(".3"),
               breakout_length_nm=nm_from_mm("1"), breakout_clearance_nm=nm_from_mm(".1"))
NO_VIAS = dict(max_vias=0)


def guides(source):
    # This fixture tests physical surface handoff, not access to 5 mm coarse
    # tiles: that grid leaves two of the neighbouring QFN pins unreachable.
    result = route_global(source, GlobalRouterOptions(tile_size_nm=nm_from_mm('2.5')))
    assert result.status is GlobalRoutingStatus.SUCCESS
    return result


def hard_errors(source):
    return [f for f in run_physical_drc(source).findings if f.severity.value == "error"
            and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


def assert_surface_escape(source, result):
    access = result.surface_accesses[PIN]
    assert PIN not in result.accesses and PIN not in result.pending_pads
    assert result.escaped_pads == {PIN, *result.accesses}
    assert result.routing_accesses[PIN] == access
    assert not any(via.net == "X1" for via in result.board.vias)
    assert access.layer is CopperLayer.FRONT and access.launch_position == Point.mm(10.1, 8.75)
    assert all(track.layer is CopperLayer.FRONT for track in access.path)
    assert set(access.path) <= set(result.created_tracks)
    collar = package_collar(source, "U", BoundaryAccessOptions().collar_margin_nm).bounds
    assert not (collar.min_x <= access.position.x_nm <= collar.max_x
                and collar.min_y <= access.position.y_nm <= collar.max_y)
    clearance = RoutingClearanceIndex(result.board)
    assert verified_routing_access(result.board, PIN, "X1", access, clearance) == access.path
    assert not hard_errors(result.board)


@pytest.mark.parametrize("layers", [FOUR, (CopperLayer.FRONT, CopperLayer.BACK)])
def test_nets_that_may_change_layer_keep_their_dogbones(monkeypatch, layers):
    source = board(allowed_layers=layers)
    result = fanout.route_fanout(source)
    assert result.surface_accesses is None
    assert PIN in result.accesses and any(via.net == "X1" for via in result.created_vias)
    assert result.routing_accesses == result.accesses
    monkeypatch.setattr(fanout, "_surface_layer", lambda *args: None)
    assert fanout.route_fanout(source) == result


def test_blocked_surface_pin_stays_pending_without_a_via():
    # An F.Cu track keepout ring across every collar port; vias stay legal.
    def wall(name, left, top, right, bottom):
        return CopperKeepout(name, (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
            Point.mm(left, top), Point.mm(right, top), Point.mm(right, bottom), Point.mm(left, bottom)))),
            block_tracks=True, block_vias=False, block_zones=False)
    ring = (wall("left", 8.8, 6.8, 9.3, 13.2), wall("right", 14.7, 6.8, 15.2, 13.2),
            wall("top", 8.8, 6.8, 15.2, 7.3), wall("bottom", 8.8, 12.7, 15.2, 13.2))
    source = replace(board(**ONE_LAYER), copper_keepouts=ring)
    result = fanout.route_fanout(source)
    assert PIN in result.pending_pads and PIN not in result.escaped_pads
    assert not any(via.net == "X1" for via in result.board.vias)
    analysis = next(item for item in result.pin_analysis if item.pad == PIN)
    assert analysis.selected_candidate_index is None
