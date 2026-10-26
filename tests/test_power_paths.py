from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference,
                   PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
                   Point, Size, TrackSegment)
from pcbir.power_paths import PowerPathWidth, check_power_path_widths


def fixture():
    footprint = PhysicalFootprint('pad', (FootprintPad('1', Point(0, 0), Size.mm('.6', '.6')),), Size.mm(1, 1))
    poses = tuple(Placement(ref, 'pad', Point.mm(x, y)) for ref, x, y in
                  [('SOURCE', 2, 5), ('LOAD', 12, 5), ('SENSE', 7, 8)])
    net = PhysicalNet('PWR', tuple(PadReference(p.reference, '1') for p in poses))
    tracks = (TrackSegment('PWR', Point.mm(2, 5), Point.mm(12, 5), 1000000, CopperLayer.FRONT),
              TrackSegment('PWR', Point.mm(7, 5), Point.mm(7, 8), 200000, CopperLayer.FRONT))
    board = PhysicalBoard('power', BoardOutline.rectangle(15, 12), {'pad': footprint}, poses, (net,), tracks=tracks)
    requirement = PowerPathWidth('PWR', (PadReference('SOURCE', '1'), PadReference('LOAD', '1')), 1000000)
    return board, requirement


def test_narrow_sense_branch_does_not_fail_wide_load_path():
    board, requirement = fixture()
    result, = check_power_path_widths(board, (requirement,))
    assert result.explicit_connected and result.width_connected
    assert result.excluded_track_indexes == (1,)
    assert len(board.tracks) == 2


def test_short_narrow_trunk_is_detected_even_beside_same_net_sense_tap():
    board, requirement = fixture()
    board = replace(board, tracks=(
        TrackSegment('PWR', Point.mm(2, 5), Point.mm(6, 5), 1000000, CopperLayer.FRONT),
        TrackSegment('PWR', Point.mm(6, 5), Point.mm(8, 5), 200000, CopperLayer.FRONT),
        TrackSegment('PWR', Point.mm(8, 5), Point.mm(12, 5), 1000000, CopperLayer.FRONT), board.tracks[1]))
    result, = check_power_path_widths(board, (requirement,))
    assert result.explicit_connected and not result.width_connected
    assert result.excluded_track_indexes == (1, 3)


def test_wide_parallel_route_is_sufficient():
    board, requirement = fixture()
    board = replace(board, tracks=(*board.tracks,
        TrackSegment('PWR', Point.mm(2, 5), Point.mm(12, 5), 200000, CopperLayer.FRONT)))
    result, = check_power_path_widths(board, (requirement,))
    assert result.width_connected


def test_open_path_stays_open_and_unrelated_net_cannot_bridge_it():
    board, requirement = fixture()
    board = replace(board, nets=(*board.nets, PhysicalNet('OTHER', ())),
                    tracks=(replace(board.tracks[0], net='OTHER'), board.tracks[1]))
    result, = check_power_path_widths(board, (requirement,))
    assert not result.explicit_connected and not result.width_connected


def test_terminal_membership_and_width_are_validated():
    board, requirement = fixture()
    with pytest.raises(ValueError, match='belong'):
        check_power_path_widths(board, (replace(requirement, net='OTHER'),))
    with pytest.raises(ValueError, match='positive integer'):
        replace(requirement, minimum_width_nm=0)
    with pytest.raises(ValueError, match='distinct'):
        replace(requirement, terminals=(requirement.terminals[0],)*2)


def test_layer_transition_requires_a_real_plated_connection():
    from pcbir import Via
    board, requirement = fixture()
    board = replace(board, tracks=(
        TrackSegment('PWR', Point.mm(2, 5), Point.mm(7, 5), 1000000, CopperLayer.FRONT),
        TrackSegment('PWR', Point.mm(7, 5), Point.mm(12, 5), 1000000, CopperLayer.BACK)),
        vias=tuple(Via('PWR', Point.mm(x, 5), 600000, 300000, CopperLayer.FRONT, CopperLayer.BACK)
                   for x in (7, 12)))
    assert check_power_path_widths(board, (requirement,))[0].width_connected
    assert not check_power_path_widths(replace(board, vias=board.vias[:1]), (requirement,))[0].width_connected


def test_installed_component_connections_do_not_prove_pcb_width():
    from pcbir import InternalPadGroup
    bridge = PhysicalFootprint('bridge', (
        FootprintPad('1', Point.mm(-2, 0), Size.mm('.6', '.6')),
        FootprintPad('2', Point.mm(2, 0), Size.mm('.6', '.6'))), Size.mm(5, 1),
        internal_pad_groups=(InternalPadGroup(('1', '2')),))
    pads = (PadReference('BRIDGE', '1'), PadReference('BRIDGE', '2'))
    board = PhysicalBoard('bridge', BoardOutline.rectangle(10, 10), {'bridge': bridge},
                          (Placement('BRIDGE', 'bridge', Point.mm(5, 5)),), (PhysicalNet('PWR', pads),))
    result, = check_power_path_widths(board, (PowerPathWidth('PWR', pads, 1000000),))
    assert not result.explicit_connected and not result.width_connected
