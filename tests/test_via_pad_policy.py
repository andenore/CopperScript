from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference,
                   PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
                   Point, Size, Via, nm_from_mm, run_physical_drc)
from pcbir.routing_clearance import RoutingClearanceIndex


def board():
    footprint = PhysicalFootprint("pad", (FootprintPad("1", Point(0, 0),
                                  Size.mm(1, 1)),), Size.mm(2, 2))
    return PhysicalBoard("test", BoardOutline.rectangle(20, 20),
                         {"pad": footprint}, (Placement("U1", "pad", Point.mm(5, 5)),),
                         (PhysicalNet("GND", (PadReference("U1", "1"),)),))


@pytest.mark.parametrize("x", [5, 5.7, 5.8])
def test_full_annulus_cannot_overlap_or_touch_same_net_pad(x):
    index = RoutingClearanceIndex(board())
    via = Via("GND", Point.mm(x, 5), nm_from_mm(.6), nm_from_mm(.3),
              CopperLayer.FRONT, CopperLayer.BACK)
    assert not index.can_via(via.net, via.position, via.size_nm, via.from_layer, via.to_layer)
    assert index.blocking_via_nets(via) == (frozenset(), True)
    assert not index.candidate_vias_clear((via,))
    assert index.candidate_via_conflict((via,), allow_movable_conflicts=True) == via
    findings = run_physical_drc(replace(board(), vias=(via,))).findings
    assert any(f.code == "DRC-VIA-PAD-OVERLAP" for f in findings)


def test_off_pad_via_allowed_and_explicit_qualified_overlap_remains_separate():
    index = RoutingClearanceIndex(board())
    assert index.can_via("GND", Point.mm(5.81, 5), nm_from_mm(.6),
                         CopperLayer.FRONT, CopperLayer.BACK)
    assert index.can_via("GND", Point.mm(5, 5), nm_from_mm(.6),
                         CopperLayer.FRONT, CopperLayer.BACK, allow_pad_overlap=True)
    assert not index.can_via("OTHER", Point.mm(5, 5), nm_from_mm(.6),
                             CopperLayer.FRONT, CopperLayer.BACK, allow_pad_overlap=True)


def test_pad_on_other_layer_does_not_block_blind_via():
    from pcbir import Stackup
    base = replace(board(), stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                                           CopperLayer.INTERNAL_2, CopperLayer.BACK)))
    assert RoutingClearanceIndex(base).can_via("GND", Point.mm(5, 5), nm_from_mm(.6),
                                              CopperLayer.INTERNAL_1, CopperLayer.BACK)
