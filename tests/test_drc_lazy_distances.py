"""Lazy measurements preserve complete DRC findings and signoff output."""
from dataclasses import replace
from random import Random

import pytest

import pcbir.drc as drc
from pcbir import (BoardOutline, CopperLayer, DesignRules, FootprintPad, NetRoutingRule,
    PadReference, PadShape, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
    Point, Size, Stackup, TrackSegment, Via)
from pcbir.geometry import point_segment_distance_squared, segment_distance_squared, shape_distance_squared


def _board(seed):
    rng = Random(seed)
    names = ("A", "B", "C", "D")
    layers = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)
    footprints = {shape.value: PhysicalFootprint(shape.value,
        (FootprintPad("1", Point(0, 0), Size.mm("0.8", "1.2"), shape=shape),), Size.mm(1, 2))
        for shape in (PadShape.RECTANGLE, PadShape.CIRCLE, PadShape.OVAL, PadShape.ROUNDRECT)}
    placements = tuple(Placement(name, shape, Point.mm(8 + i * .4, 8 + i * .3),
                                 rotation_degrees=(45 * i))
                       for i, (name, shape) in enumerate(zip(names, footprints)))
    return PhysicalBoard("lazy-distance", BoardOutline.rectangle(20, 20), footprints, placements,
        tuple(PhysicalNet(name, (PadReference(name, "1"),)) for name in names),
        stackup=Stackup(copper_layers=layers),
        tracks=tuple(TrackSegment(names[i % 4], Point.mm(rng.randrange(2, 18), rng.randrange(2, 18)),
            Point.mm(rng.randrange(2, 18), rng.randrange(2, 18)), rng.choice((200001, 250000, 300001)),
            layers[i % 4]) for i in range(16)),
        vias=tuple(Via(names[i % 4], Point.mm(8 + i * .3, 8 + i * .2), 600001, 300000)
                   for i in range(5)),
        net_routing_rules=(NetRoutingRule("D", clearance_nm=300001),),
        metadata={"detailed_routing": "complete"})


def _rational_decisions(monkeypatch):
    # The original copper-spacing decision calculated every rational distance.
    monkeypatch.setattr(drc, "point_segment_distance_at_least", lambda p, a, b, n, denominator=1:
        denominator * denominator * point_segment_distance_squared(p, a, b) >= n * n)
    monkeypatch.setattr(drc, "segment_distance_at_least", lambda a, b, c, d, n, denominator=1:
        denominator * denominator * segment_distance_squared(a, b, c, d) >= n * n)
    monkeypatch.setattr(drc, "shapes_clear", lambda a, b, clearance=0:
        shape_distance_squared(a, b) >= (a.radius_nm + b.radius_nm + clearance) ** 2)


@pytest.mark.parametrize("seed", (12, 441, 925, 1024))
def test_complete_drc_reports_match_eager_rational_decisions(monkeypatch, seed):
    board = _board(seed)
    optimized = drc.run_physical_drc(board)
    assert any(finding.code == "DRC-SHORT" for finding in optimized.findings)
    assert any(finding.code == "DRC-CLEARANCE" for finding in optimized.findings)
    with monkeypatch.context() as context:
        _rational_decisions(context)
        reference = drc.run_physical_drc(board)
    assert optimized == reference
    assert optimized.to_json() == reference.to_json()


@pytest.mark.parametrize("distance,violation", ((4, True), (5, False)))
def test_odd_width_track_track_and_track_via_thresholds(distance, violation):
    # Required distance is 4.5 nm, not the 4 nm from truncated copper radii.
    board = PhysicalBoard("half-nm", BoardOutline.rectangle(10, 10), {}, (),
        (PhysicalNet("A", ()), PhysicalNet("B", ())),
        rules=DesignRules(minimum_clearance_nm=1),
        tracks=(TrackSegment("A", Point(100, 100), Point(200, 100), 3, CopperLayer.FRONT),
                TrackSegment("B", Point(100, 100 + distance), Point(200, 100 + distance), 4, CopperLayer.FRONT)),
        vias=(Via("B", Point(150, 100 + distance), 4, 2),))
    findings = []
    drc._check_copper_spacing(board, findings)
    assert bool(findings) is violation
    if violation:
        assert len(findings) == 2
        assert all(item.code == "DRC-CLEARANCE" and item.measured_nm == 4 and item.required_nm == 5
                   for item in findings)


def test_passing_copper_pairs_need_no_rational_measurement(monkeypatch):
    board = _board(12)
    board = replace(board, tracks=(
        TrackSegment("A", Point.mm(2, 2), Point.mm(4, 2), 250001, CopperLayer.FRONT),
        TrackSegment("B", Point.mm(2, 5), Point.mm(4, 5), 250000, CopperLayer.FRONT)),
        vias=(Via("C", Point.mm(2, 12), 600001, 300000),),
        placements=tuple(replace(p, position=Point.mm(8 + i * 3, 16))
                         for i, p in enumerate(board.placements)))

    def forbidden(*args, **kwargs):
        raise AssertionError("passing copper pairs must not measure rational distances")

    for name in ("point_segment_distance_squared", "segment_distance_squared", "shape_distance_squared"):
        monkeypatch.setattr(drc, name, forbidden)
    findings = []
    drc._check_copper_spacing(board, findings)
    assert findings == []
