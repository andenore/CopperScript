"""Readback guards use real exported geometry, never synthesized native copper."""
from collections import Counter
from dataclasses import replace

import pytest

from pcbir import (BoardOutline, ComponentPlacementRule, CopperLayer, FootprintPad,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size)
from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.clusters import cluster_placements
from pcbir.hard_macros import materialize_hard_macros
from pcbir.importers.kicad_mod import _children, _first, _parse_sexpr
from pcbir.native_checkpoint import read_front_native_checkpoint
from pcbir.physical import PadKind, PadShape, TrackSegment, Via
from test_autonomous_plane_access import constrained_contact_board, with_test_owners
from test_hard_macros import fixture as macro_fixture


def render(board):
    return next(a.content for a in KiCadPcbBackend().generate(board).artifacts if a.name.endswith('.kicad_pcb'))


def sexpr(node):
    # Test-only serializer permits controlled native mutations without routing mocks.
    import json
    return '(' + ' '.join(sexpr(n) if isinstance(n, list) else json.dumps(n) for n in node) + ')'


def fixture(rotation=0, renamed=False):
    ref, net = ('PACKAGE_9', 'RAIL_Z') if renamed else ('U1', 'VCC')
    footprint = PhysicalFootprint('test-lands', (
        FootprintPad('1', Point.mm(-1, 0), Size.mm('0.6', '0.8')),
        FootprintPad('2', Point.mm(1, 0), Size.mm('0.6', '0.8'),
            kind=PadKind.THROUGH_HOLE, shape=PadShape.OVAL, drill=Size.mm('0.2', '0.3'))), Size.mm(3, 2))
    pose = Placement(ref, footprint.name, Point.mm(5, 5), rotation_degrees=rotation)
    track = TrackSegment(net, Point.mm(4, 5), Point.mm(6, 5), 200000, CopperLayer.FRONT)
    return PhysicalBoard('readback-test', BoardOutline.rectangle(12, 12), {footprint.name: footprint},
        (pose,), (PhysicalNet(net, (PadReference(ref, '1'), PadReference(ref, '2'))),),
        placement_rules=(ComponentPlacementRule(ref, fixed_position=pose.position,
            fixed_rotation_degrees=rotation, allowed_orientations=(rotation,)),),
        tracks=(track,), vias=(Via(net, Point.mm(7, 5), 600000, 300000),))


@pytest.mark.parametrize('rotation', (0, 90, 180, 270))
@pytest.mark.parametrize('renamed', (False, True))
def test_readback_preserves_real_lands_copper_and_input_bytes(tmp_path, rotation, renamed):
    source = fixture(rotation, renamed)
    tree = _parse_sexpr(render(source), 'fixture')
    fp = _children(tree, 'footprint')[0]
    pads = _children(fp, 'pad')
    fp[:] = [n for n in fp if not isinstance(n, list) or n[0] != 'pad'] + list(reversed(pads))
    segment = _children(tree, 'segment')[0]
    start, end = _first(segment, 'start'), _first(segment, 'end')
    start[1:], end[1:] = end[1:], start[1:]
    path = tmp_path / 'candidate.kicad_pcb'
    path.write_text(sexpr(tree))
    before = path.read_bytes()
    result = read_front_native_checkpoint(source, path)
    assert result.placements == source.placements
    assert Counter(result.tracks) == Counter(source.tracks)
    assert Counter(result.vias) == Counter(source.vias)
    assert path.read_bytes() == before
    assert result.metadata['fabrication_ready'] == 'false'
    assert not result.zone_fills


@pytest.mark.parametrize('mutation,reason', (
    ('pose', 'Illegal native placement'), ('land_position', 'Changed land position'),
    ('land_angle', 'Changed land angle'), ('land_size', 'Changed land size'),
    ('land_net', 'Changed land net'), ('land_kind', 'Changed pad shape/kind'),
    ('land_layer', 'Changed land copper layers'), ('drill', 'Changed land drill'),
    ('radius', 'Changed roundrect radius'), ('missing_component', 'Missing native components'),
    ('duplicate_component', 'Unexpected native reference'), ('missing_track', 'Missing source-owned native track'),
    ('missing_via', 'Missing source-owned native via'), ('changed_width', 'Missing source-owned native track'),
    ('changed_layers', 'Changed native copper layer order'), ('arc', 'Unsupported native copper primitive'),
))
def test_native_mutations_fail_closed(tmp_path, mutation, reason):
    source = fixture()
    tree = _parse_sexpr(render(source), 'fixture')
    fp = _children(tree, 'footprint')[0]
    pad, hole = _children(fp, 'pad')
    if mutation == 'pose':
        _first(fp, 'at')[1] = '5.1'
    elif mutation == 'land_position':
        _first(pad, 'at')[1] = '-1.1'
    elif mutation == 'land_angle':
        _first(pad, 'at')[3] = '90'
    elif mutation == 'land_size':
        _first(pad, 'size')[1] = '0.7'
    elif mutation == 'land_net':
        pad.remove(_first(pad, 'net'))
    elif mutation == 'land_kind':
        pad[2] = 'thru_hole'
    elif mutation == 'land_layer':
        _first(pad, 'layers')[1] = 'B.Cu'
    elif mutation == 'drill':
        _first(hole, 'drill')[2] = '0.25'
    elif mutation == 'radius':
        _first(pad, 'roundrect_rratio')[1] = '0.3'
    elif mutation == 'missing_component':
        tree.remove(fp)
    elif mutation == 'duplicate_component':
        tree.append(fp)
    elif mutation == 'missing_track':
        tree.remove(_children(tree, 'segment')[0])
    elif mutation == 'missing_via':
        tree.remove(_children(tree, 'via')[0])
    elif mutation == 'changed_width':
        _first(_children(tree, 'segment')[0], 'width')[1] = '0.3'
    elif mutation == 'changed_layers':
        _first(tree, 'layers')[1][1] = 'In1.Cu'
    elif mutation == 'arc':
        tree.append(['arc', ['layer', 'F.Cu']])
    path = tmp_path / 'changed.kicad_pcb'
    path.write_text(sexpr(tree))
    with pytest.raises(ValueError, match=reason):
        read_front_native_checkpoint(source, path)


def test_missing_duplicate_owner_occurrence_is_not_reinserted(tmp_path):
    source = fixture()
    source = replace(source, tracks=source.tracks * 2)
    native = replace(source, tracks=source.tracks[:1])
    path = tmp_path / 'one-occurrence.kicad_pcb'
    path.write_text(render(native))
    with pytest.raises(ValueError, match='Missing source-owned native track'):
        read_front_native_checkpoint(source, path)


def test_pose_only_mode_does_not_claim_native_copper_validation(tmp_path):
    source = fixture()
    path = tmp_path / 'poses-only.kicad_pcb'
    path.write_text(render(replace(source, tracks=(), vias=())))
    result = read_front_native_checkpoint(source, path, include_copper=False)
    assert result.metadata['native_copper_checked'] == 'false'
    assert result.metadata['fabrication_ready'] == 'false'
    with pytest.raises(ValueError, match='Missing source-owned native'):
        read_front_native_checkpoint(source, path)


@pytest.mark.parametrize('rotation', (0, 45, 90))
def test_native_rigid_macro_can_move_only_as_a_legal_unit(tmp_path, rotation):
    _, _, bind = macro_fixture(tmp_path)
    source = bind()
    poses = {p.reference: p for p in source.placements}
    poses.update(cluster_placements(source, source.rigid_clusters[0],
        replace(poses['U1'], position=Point.mm(15, 15), rotation_degrees=rotation)))
    native = materialize_hard_macros(replace(source, placements=tuple(poses.values())))
    path = tmp_path / 'moved-owner.kicad_pcb'
    path.write_text(render(native))
    readback = read_front_native_checkpoint(source, path)
    assert readback.placements == native.placements
    assert Counter(readback.tracks) == Counter(native.tracks)
    tree = _parse_sexpr(render(native), 'moved-owner')
    _first(_children(tree, 'footprint')[1], 'at')[1] = '16'
    path.write_text(sexpr(tree))
    with pytest.raises(ValueError, match='Illegal native placement'):
        read_front_native_checkpoint(source, path)


@pytest.mark.parametrize('mutation', ('reverse', 'missing', 'vertex', 'net', 'stroke', 'unfilled', 'duplicate'))
def test_fixed_polygon_geometry_and_occurrences_are_observed(tmp_path, mutation):
    _, asset, bind = macro_fixture(tmp_path)
    asset.update(schema='copperlib-physical-hard-macro/v0.3', zones=[], plane_returns=[],
        polygons=[dict(id='bridge', net='signal', layer='F.Cu', vertices=[
            [1400000, -200000], [2200000, -200000], [2200000, 200000],
            [1900000, 200000], [1900000, 100000], [1400000, 100000]])])
    asset['tracks'][0]['points'] = [{'pad': ['chip', '1']}, [1500000, 0]]
    if mutation == 'duplicate':
        asset['polygons'].append({**asset['polygons'][0], 'id': 'second-bridge'})
    source = bind()
    owned = materialize_hard_macros(source)
    tree = _parse_sexpr(render(owned), 'polygon')
    item = _children(tree, 'gr_poly')[0]
    if mutation == 'reverse':
        _first(item, 'pts')[1:] = list(reversed(_first(item, 'pts')[1:]))
    elif mutation in ('missing', 'duplicate'):
        tree.remove(item)
    elif mutation == 'vertex':
        _first(item, 'pts')[1][1] = '11.41'
    elif mutation == 'net':
        item.remove(_first(item, 'net'))
    elif mutation == 'stroke':
        _first(_first(item, 'stroke'), 'width')[1] = '0.1'
    elif mutation == 'unfilled':
        _first(item, 'fill')[1] = 'no'
    path = tmp_path / 'polygon.kicad_pcb'
    path.write_text(sexpr(tree))
    if mutation == 'reverse':
        result = read_front_native_checkpoint(source, path)
        assert Counter(result.polygons) == Counter(owned.polygons)
    else:
        with pytest.raises(ValueError, match='native.*polygon'):
            read_front_native_checkpoint(source, path)


@pytest.mark.parametrize('required_array', (False, True))
@pytest.mark.parametrize('renamed', (False, True))
def test_macro_and_array_geometry_must_be_observed(tmp_path, required_array, renamed):
    source = with_test_owners(constrained_contact_board(renamed=renamed), tmp_path,
                              required_array=required_array)
    owned = materialize_hard_macros(source)
    path = tmp_path / 'owned.kicad_pcb'
    path.write_text(render(owned))
    result = read_front_native_checkpoint(source, path)
    assert Counter(result.tracks) == Counter(owned.tracks)
    assert Counter(result.vias) == Counter(owned.vias)
    if required_array:
        assert sum(v.finish == 'filled-capped' for v in result.vias) == 4
    for kind in ('segment', 'via'):
        tree = _parse_sexpr(render(owned), 'owned')
        tree.remove(_children(tree, kind)[-1])
        path.write_text(sexpr(tree))
        with pytest.raises(ValueError, match='Missing source-owned native'):
            read_front_native_checkpoint(source, path)
