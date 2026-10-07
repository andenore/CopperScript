"""Drill-to-copper spacing must agree for search, repair and final DRC."""
from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference,
                   PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
                   Point, Size, TrackSegment, Via, run_physical_drc)
from pcbir.physical import DesignRules
from pcbir.routing_clearance import RoutingClearanceIndex


def board():
    return PhysicalBoard("hole-clearance", BoardOutline.rectangle(20, 20), {}, (),
        (PhysicalNet("A", ()), PhysicalNet("B", ())),
        rules=DesignRules(minimum_clearance_nm=100_000))


def via(net="A", x=5):
    return Via(net, Point.mm(x, 5), 400_000, 200_000)


@pytest.mark.parametrize("finish", ["standard", "filled-capped"])
@pytest.mark.parametrize("distance,legal", [(0.399999, False), (0.4, True)])
def test_foreign_track_clearance_is_symmetric_at_exact_limit(finish, distance, legal):
    base = board()
    v = replace(via(), finish=finish)
    track = TrackSegment("B", Point.mm(4, 5 + distance), Point.mm(6, 5 + distance),
                         100_000, CopperLayer.FRONT)
    index = RoutingClearanceIndex(replace(base, vias=(v,)))
    assert index.can_track(track.net, track.start, track.end, track.width_nm, track.layer) is legal
    assert index.blocking_track_nets(track)[1] is not legal
    reverse = RoutingClearanceIndex(replace(base, tracks=(track,)))
    assert reverse.can_via(v.net, v.position, v.size_nm, v.from_layer, v.to_layer, v.drill_nm) is legal
    assert reverse.blocking_via_nets(v)[1] is not legal
    findings = run_physical_drc(replace(base, tracks=(track,), vias=(v,))).findings
    assert any(f.code == "DRC-HOLE-CLEARANCE" for f in findings) is not legal
    from pcbir.drc import placement_copper_findings
    editor_findings = placement_copper_findings(replace(base, tracks=(track,), vias=(v,)))
    assert any(f.code == "DRC-HOLE-CLEARANCE" for f in editor_findings) is not legal


def test_repair_identifies_movable_drill_clearance_blocker():
    index = RoutingClearanceIndex(board())
    index.add_via(via(), locked=False)
    track = TrackSegment("B", Point.mm(4, 5.38), Point.mm(6, 5.38), 100_000, CopperLayer.FRONT)
    assert index.blocking_track_nets(track) == (frozenset({"A"}), False)
    index = RoutingClearanceIndex(board())
    index.add_track(track, locked=False)
    assert index.blocking_via_nets(via()) == (frozenset({"B"}), False)


def test_pad_and_peer_vias_observe_hole_to_copper_not_only_annulus_or_drill_spacing():
    base = board()
    fp = PhysicalFootprint("pad", (FootprintPad("1", Point(0, 0), Size.mm(.2, .2)),), Size.mm(.3, .3))
    pads = replace(base, footprints={fp.name: fp},
        placements=(Placement("U", fp.name, Point.mm(5.44, 5)),),
        nets=(base.nets[0], PhysicalNet("B", (PadReference("U", "1"),))))
    v = via()
    assert not RoutingClearanceIndex(pads).can_via(v.net, v.position, v.size_nm, v.from_layer, v.to_layer, v.drill_nm)
    assert any(f.code == "DRC-HOLE-CLEARANCE" for f in run_physical_drc(replace(pads, vias=(v,))).findings)
    other = via("B", 5.54)  # 0.24 mm drill-to-annulus, but legal 0.34 mm drill spacing.
    assert not RoutingClearanceIndex(base).candidate_vias_clear((v, other))
    assert not RoutingClearanceIndex(replace(base, vias=(v,))).can_via(
        other.net, other.position, other.size_nm, other.from_layer, other.to_layer, other.drill_nm)
    assert any(f.code == "DRC-HOLE-CLEARANCE" for f in run_physical_drc(replace(base, vias=(v, other))).findings)
    assert RoutingClearanceIndex(base).candidate_vias_clear((v, replace(other, net="A")))


def test_tentative_foreign_vias_also_observe_net_specific_copper_spacing():
    from pcbir.physical import NetRoutingRule
    base = replace(board(), net_routing_rules=(NetRoutingRule("A", clearance_nm=800000),))
    assert not RoutingClearanceIndex(base).candidate_vias_clear((via(), via("B", 5.65)))


def test_global_via_capacity_sites_can_coexist_on_distinct_nets():
    from pcbir.routing import _legal_via_sites
    base = replace(board(), rules=replace(board().rules,
        default_via_size_nm=450_000, default_via_drill_nm=200_000))
    index = RoutingClearanceIndex(base)
    sites = _legal_via_sites(base, index, Point.mm(10, 10), 3_000_000,
                            CopperLayer.FRONT, CopperLayer.BACK)
    assert len(sites) > 1
    # Annulus clearance alone permits 0.55 mm pitch; drill-to-annulus
    # clearance requires 0.575 mm for this common 0.45/0.20 mm via.
    proposals = tuple(Via(f"net-{i}", p, 450_000, 200_000)
                      for i, p in enumerate(sites))
    assert index.candidate_vias_clear(proposals)
