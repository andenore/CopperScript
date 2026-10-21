"""Off-centre access retains opposite-face/process and exact path constraints."""
from dataclasses import replace
import json
import shutil
import subprocess

import pytest

from pcbir import KiCadPcbBackend
from pcbir.access_sites import circular_cell_sites, obstacle_boundary_sites
from pcbir.backends.kicad_project import write_kicad_project
from pcbir.drc import run_physical_drc, PhysicalDrcPolicy
from pcbir.fanout import _legal_choices
from pcbir.physical import (BoardSide, CopperKeepout, CopperLayer, FootprintPad,
    PadReference, PhysicalFootprint, Placement, Point, PolygonRing, PolygonWithHoles, Size)
from pcbir.placement import transformed_local_point
from pcbir.routing_clearance import RoutingClearanceIndex
from test_interstitial_fanout import fixture


def offset_scene(rotation=0, side=BoardSide.FRONT):
    board, pose, _, _ = fixture()
    package = replace(board.footprints['array'], pads=tuple(
        replace(p, size=Size.mm('.3', '.3')) for p in board.footprints['array'].pads))
    pose = replace(pose, rotation_degrees=rotation, side=side)
    blocker = PhysicalFootprint('opposite', (FootprintPad('1', Point(0, 0), Size.mm('.1', '.1')),), Size.mm('.2', '.2'))
    # A same-net SMD land overlaps the ordinary annulus at the cell centre.
    # There is room to shift the via; assembly permission cannot be inferred.
    opposite = Placement('C', 'opposite', transformed_local_point(pose, Point.mm('.64', '.375')),
                         side=BoardSide.BACK if side is BoardSide.FRONT else BoardSide.FRONT)
    board = replace(board, footprints={'array': package, 'opposite': blocker},
        placements=(pose, opposite), nets=tuple(replace(n, pads=(*n.pads, PadReference('C', '1')))
            if n.name == 'N5' else n for n in board.nets))
    return board, pose, package.pads[4], pose.position, transformed_local_point(pose, Point.mm('.375', '.375'))


def proposals(board, start, seed, limit=24):
    return obstacle_boundary_sites(RoutingClearanceIndex(board), 'N5', start,
        450000, 200000, (CopperLayer.FRONT, CopperLayer.BACK), 600000, limit, seeds=(seed,))


@pytest.mark.parametrize('rotation', (0, 90, 180, 270))
@pytest.mark.parametrize('side', (BoardSide.FRONT, BoardSide.BACK))
def test_off_centre_launch_without_same_net_opposite_pad_overlap(rotation, side):
    board, pose, pad, start, seed = offset_scene(rotation, side)
    index = RoutingClearanceIndex(board)
    assert not _legal_choices(board, index, 'N5', start, pose, pad, (seed,))
    sites = proposals(board, start, seed)
    assert len(sites) <= 24 and sites == proposals(board, start, seed)
    choices = _legal_choices(board, index, 'N5', start, pose, pad, sites, multiple_orders=True)
    assert choices
    path, via = choices[0]
    assert via.position != seed and via.finish != 'filled-capped'
    assert index.can_via('N5', via.position, via.size_nm, via.from_layer, via.to_layer, via.drill_nm)
    candidate = replace(board, tracks=path, vias=(via,))
    assert not [f for f in run_physical_drc(candidate,
        policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings
        if f.severity.value == 'error' and f.code != 'DRC-OPEN-NET']
    assert board.tracks == board.vias == board.via_in_pad_rules == ()


def test_cell_proposals_transform_actual_geometry_and_are_finite():
    board, pose, pad, _, _ = offset_scene(90, BoardSide.BACK)
    sites = circular_cell_sites(board, pose, pad, 1500000, 600000)
    assert len(sites) == 4
    assert set(sites) == {transformed_local_point(pose, Point(x, y))
                         for x in (-375000, 375000) for y in (-375000, 375000)}
    assert circular_cell_sites(board, pose, pad, 700000, 600000) == ()


@pytest.mark.parametrize('rotation', (0, 90, 180, 270))
@pytest.mark.parametrize('side', (BoardSide.FRONT, BoardSide.BACK))
def test_off_centre_frontier_can_keep_a_single_diagonal_dogbone(rotation, side):
    board, pose, pad, start, seed = offset_scene(rotation, side)
    index = RoutingClearanceIndex(board)
    assert not _legal_choices(board, index, 'N5', start, pose, pad, (seed,))
    sites = proposals(board, start, seed)
    assert len(sites) <= 24
    choices = _legal_choices(board, index, 'N5', start, pose, pad, sites, multiple_orders=True)
    # The reference-like property is a direction and topology, not a copied
    # manufacturer's offset or smaller via. Every layer still has to clear.
    path, via = next((path, via) for path, via in choices if len(path) == 1)
    dx, dy = via.position.x_nm-start.x_nm, via.position.y_nm-start.y_nm
    assert abs(dx) == abs(dy) and via.position != seed
    assert (via.size_nm, via.drill_nm) == (450000, 200000)
    assert index.can_via('N5', via.position, via.size_nm, via.from_layer, via.to_layer, drill_nm=via.drill_nm)
    assert not [f for f in run_physical_drc(replace(board, tracks=path, vias=(via,)),
        policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings
        if f.severity.value == 'error' and f.code != 'DRC-OPEN-NET']
    assert board.tracks == board.vias == board.via_in_pad_rules == ()


def test_site_cap_and_fully_blocked_negative_preserve_the_input():
    board, pose, pad, start, seed = offset_scene()
    assert proposals(board, start, seed, 0) == ()
    for limit in (1, 2, 8):
        assert len(proposals(board, start, seed, limit)) <= limit
    outline = PolygonWithHoles(PolygonRing((Point.mm(4, 4), Point.mm(6, 4), Point.mm(6, 6), Point.mm(4, 6))))
    board = replace(board, copper_keepouts=(CopperKeepout('blocked',
        (CopperLayer.FRONT, CopperLayer.BACK), outline),))
    assert not _legal_choices(board, RoutingClearanceIndex(board), 'N5', start, pose, pad,
                              proposals(board, start, seed))
    assert board.tracks == board.vias == ()


def test_off_centre_escape_passes_pinned_native_geometry(tmp_path):
    cli = shutil.which('kicad-cli')
    if not cli:
        pytest.skip('KiCad CLI unavailable')
    board, pose, pad, start, seed = offset_scene()
    path, via = _legal_choices(board, RoutingClearanceIndex(board), 'N5', start, pose, pad,
                              proposals(board, start, seed), multiple_orders=True)[0]
    assert len(path) == 1  # Validate the compact diagonal family natively too.
    candidate = replace(board, tracks=path, vias=(via,))
    output = tmp_path/'offset.kicad_pcb'
    write_kicad_project(KiCadPcbBackend().generate(candidate), output)
    report = tmp_path/'drc.json'
    result = subprocess.run([cli, 'pcb', 'drc', '--format', 'json', '--output', str(report), str(output)],
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    findings = json.loads(report.read_text())['violations']
    assert not [f for f in findings if f['severity'] == 'error'
                and f['type'] not in {'via_dangling', 'track_dangling', 'unconnected_items'}]
