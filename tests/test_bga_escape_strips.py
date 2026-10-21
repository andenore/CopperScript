"""Ball arrays keep their surface escape strips free of supply and return vias."""
from dataclasses import replace

from pcbir.bga_fanout import escape_strips, plan_bga_fanouts
from pcbir.drc import PhysicalDrcPolicy, run_physical_drc
from pcbir.physical import (BoardOutline, CopperLayer, CopperZone, DesignRules, FootprintPad,
                            PadReference, PadShape, PhysicalBoard, PhysicalFootprint, PhysicalNet,
                            Placement, Point, PolygonRing, PolygonWithHoles, Size)


def _board() -> PhysicalBoard:
    # 5x5 array, 0.75 mm pitch, 0.4 mm lands, centred at (6, 6): the south row is y = 7.5.
    pads = tuple(FootprintPad(str(y * 5 + x + 1), Point.mm(str((x - 2) * 0.75), str((y - 2) * 0.75)),
                              Size.mm(".4", ".4"), shape=PadShape.CIRCLE) for y in range(5) for x in range(5))
    ball = lambda n: PadReference("U", str(n))
    # South-row signals leave towards their partners on the north row; ball 23 is ground.
    nets = (*(PhysicalNet(f"S{n}", (ball(n), ball(n - 20))) for n in (21, 22, 24, 25)),
            PhysicalNet("GND", (ball(23), ball(13))))
    outline = BoardOutline.rectangle(12, 12)
    plane = PolygonWithHoles(PolygonRing((Point.mm(0, 0), Point.mm(12, 0), Point.mm(12, 12), Point.mm(0, 12))))
    return PhysicalBoard("strips", outline, {"array": PhysicalFootprint("array", pads, Size.mm(4, 4))},
                         (Placement("U", "array", Point.mm(6, 6)),), nets,
                         rules=DesignRules(minimum_clearance_nm=100_000, minimum_hole_clearance_nm=200_000,
                                           minimum_track_width_nm=100_000, default_track_width_nm=150_000,
                                           default_via_size_nm=450_000, default_via_drill_nm=200_000),
                         zones=(CopperZone("gnd", "GND", (CopperLayer.BACK,), plane),))


def _ground_via_y(board: PhysicalBoard) -> float:
    (via,) = [v for v in plan_bga_fanouts(board).vias if v.net == "GND" and v.position.y_nm > 7_000_000]
    return via.position.y_nm / 1e6


def test_a_signal_edge_gets_a_via_strip_and_its_ground_ball_vias_inward() -> None:
    board = _board()
    strips = escape_strips(board)
    south = {s.id: s for s in strips}["bga-escape-strip-U-bottom"]
    assert south.block_vias and not south.block_tracks and not south.block_zones
    assert min(p.y_nm for p in south.outline.outer.vertices) == 7_700_000  # just outside the 0.4 mm lands
    # Without the strip the south ground ball's dogbone points outward, into the channel.
    assert _ground_via_y(board) > 7.5
    held = replace(board, copper_keepouts=strips)
    assert _ground_via_y(held) < 7.5
    placed = plan_bga_fanouts(held).board
    findings = run_physical_drc(replace(placed, copper_keepouts=()),
                                policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings
    assert not [f for f in findings if f.severity.value == "error" and f.code != "DRC-OPEN-NET"]


def test_an_edge_without_signal_balls_gets_no_strip() -> None:
    board = _board()
    only_ground = replace(board, nets=tuple(n for n in board.nets if n.name == "GND"))
    assert escape_strips(only_ground) == ()
