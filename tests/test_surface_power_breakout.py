"""Local plane access must honor declared, bounded power-pin neckdowns."""
from dataclasses import replace

from pcbir import (BoardOutline, CopperLayer, FootprintPad, NetRoutingRule,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
    Point, Size, nm_from_mm)
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.surface_path import surface_path, surface_path_to_via


def board(neck=True):
    fp = PhysicalFootprint('fine-row', tuple(FootprintPad(str(i + 1),
        Point.mm(0, y), Size.mm('.6', '.25')) for i, y in enumerate(('-0.5', '0', '0.5'))),
        Size.mm(1, 2))
    rule = NetRoutingRule('P', width_nm=nm_from_mm('.8'),
        breakout_length_nm=nm_from_mm('1') if neck else None,
        breakout_width_nm=nm_from_mm('.2') if neck else None)
    return PhysicalBoard('power escape', BoardOutline.rectangle(20, 10),
        {fp.name: fp}, (Placement('U', fp.name, Point.mm(5, 5)),),
        (PhysicalNet('P', (PadReference('U', '2'),)),), net_routing_rules=(rule,))


def path(source, end=Point.mm(8, 5), committed=()):
    return surface_path(source, RoutingClearanceIndex(source), 'P', Point.mm(5, 5),
        end, nm_from_mm('.8'), CopperLayer.FRONT, committed)


def test_local_power_escape_necks_only_inside_declared_region():
    source = board()
    tracks = path(source)
    assert tracks and {t.width_nm for t in tracks} == {nm_from_mm('.2'), nm_from_mm('.8')}
    index = RoutingClearanceIndex(source)
    assert all(t.width_nm >= index.breakout.required_width_nm(t, nm_from_mm('.8'))
               and index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in tracks)
    assert path(board(neck=False)) is None


def test_valid_existing_neckdown_chain_is_reused_not_recreated():
    source = board()
    tracks = path(source)
    assert tracks
    assert path(replace(source, tracks=tracks), committed=tracks) == ()


def test_unapproved_narrow_trunk_cannot_be_reused():
    source = board()
    tracks = tuple(replace(t, width_nm=nm_from_mm('.2')) for t in path(source))
    result = path(replace(source, tracks=tracks), committed=tracks)
    assert result and any(t.width_nm == nm_from_mm('.8') for t in result)


def test_via_maze_emits_checked_neckdown_and_full_width_pieces():
    end = Point.mm(8, 5)
    def search(source):
        return surface_path_to_via(source, RoutingClearanceIndex(source), 'P', Point.mm(5, 5),
            nm_from_mm('.8'), CopperLayer.FRONT, step_nm=nm_from_mm('.1'),
            radius_nm=nm_from_mm(3), via_size_nm=nm_from_mm('.6'), via_drill_nm=nm_from_mm('.3'),
            via_layers=(CopperLayer.FRONT, CopperLayer.BACK), accept_via=lambda p: p == end,
            via_targets=(end,), state_budget=5000)
    source = board()
    found = search(source)
    assert found and found[1] == end
    assert {t.width_nm for t in found[0]} == {nm_from_mm('.2'), nm_from_mm('.8')}
    index = RoutingClearanceIndex(source)
    assert all(t.width_nm >= index.breakout.required_width_nm(t, nm_from_mm('.8'))
               and index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in found[0])
    assert search(board(neck=False)) is None
