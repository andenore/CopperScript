from dataclasses import replace
import pytest
from pcbir import (BoardOutline, PhysicalBoard, PhysicalFootprint, FootprintPad,
                   Size, Point, Placement, PhysicalNet, PadReference, NetRoutingRule, CopperLayer)
from pcbir.power_feed import propose_feed_corridors
from pcbir.drc import physical_board_digest, run_physical_drc


def board():
    footprint = PhysicalFootprint('terminal', (FootprintPad('1', Point(0, 0), Size.mm(1, 1)),), Size.mm(2, 2))
    return PhysicalBoard('feed', BoardOutline.rectangle(20, 10), {'terminal': footprint},
        (Placement('SRC', 'terminal', Point.mm(3, 5)), Placement('LOAD', 'terminal', Point.mm(15, 5))),
        (PhysicalNet('RAIL', (PadReference('SRC', '1'), PadReference('LOAD', '1'))),),
        net_routing_rules=(NetRoutingRule('RAIL', width_nm=250000, allowed_layers=(CopperLayer.FRONT,)),))


def test_real_corridors_respect_source_width_and_preserve_input():
    source = board()
    digest = physical_board_digest(source)
    result = propose_feed_corridors(source, net='RAIL', source=PadReference('SRC', '1'),
        target=PadReference('LOAD', '1'), widths_nm=(150000, 250000, 500000))
    assert result.snapshot == digest == physical_board_digest(source)
    assert result.candidates[0].status == 'below_source_width'
    for candidate in result.candidates[1:]:
        assert candidate.status == 'proposed'
        routed = replace(source, tracks=candidate.tracks)
        assert not [f for f in run_physical_drc(routed).findings
                    if f.severity.value == 'error' and f.code != 'DRC-ROUTE-INCOMPLETE']


def test_foreign_endpoint_and_unbounded_candidates_rejected():
    args = dict(board=board(), net='RAIL', source=PadReference('SRC', '1'), target=PadReference('LOAD', '1'), widths_nm=(250000,))
    with pytest.raises(ValueError, match='members'):
        propose_feed_corridors(**{**args, 'target': PadReference('INVENTED', '1')})
    with pytest.raises(ValueError, match='excessive'):
        propose_feed_corridors(**{**args, 'widths_nm': (250000, 500000), 'maximum_candidates': 1})


def test_blocked_feed_is_inconclusive_and_does_not_move_obstacle():
    from pcbir import TrackSegment
    source = board()
    wall = TrackSegment('FOREIGN', Point.mm(9, 0), Point.mm(9, 10), 250000, CopperLayer.FRONT)
    source = replace(source, nets=(*source.nets, PhysicalNet('FOREIGN', ())), tracks=(wall,))
    result = propose_feed_corridors(source, net='RAIL', source=PadReference('SRC', '1'),
        target=PadReference('LOAD', '1'), widths_nm=(250000,), state_budget=100)
    assert result.candidates[0].status == 'no_path_within_budget'
    assert source.tracks == (wall,)
