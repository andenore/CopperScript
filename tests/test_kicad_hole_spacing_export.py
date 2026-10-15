"""Authored drilled-hole floors reach the independent native checker."""
from dataclasses import replace
from pathlib import Path
import json
import shutil
import subprocess

import pytest

from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.backends.kicad_project import write_kicad_project
from pcbir.physical import (BoardOutline, CopperLayer, DesignRules, PhysicalBoard,
                           PhysicalNet, Point, TrackSegment, Via)


def spacing_board(floor_nm=200_000, gap_nm=220_000):
    first = Point(10_000_000, 10_000_000)
    second = Point(first.x_nm + 200_000 + gap_nm, first.y_nm)
    front, back = CopperLayer.FRONT, CopperLayer.BACK
    return PhysicalBoard('HoleSpacing', BoardOutline.rectangle(20, 20), {}, (),
        (PhysicalNet('GND', ()),),
        tracks=tuple(TrackSegment('GND', first, second, 150_000, layer)
                     for layer in (front, back)),
        vias=tuple(Via('GND', point, 450_000, 200_000, front, back)
                   for point in (first, second)),
        rules=DesignRules(minimum_hole_clearance_nm=floor_nm,
            minimum_clearance_nm=100_000, minimum_track_width_nm=100_000,
            default_track_width_nm=150_000, default_via_size_nm=450_000,
            default_via_drill_nm=200_000))


@pytest.mark.parametrize('floor_nm', (200_000, 250_000, 300_000))
def test_project_serializes_authored_hole_spacing_without_changing_copper(floor_nm):
    board = spacing_board(floor_nm)
    manifest = KiCadPcbBackend().generate(board)
    project = json.loads(next(item.content for item in manifest.artifacts
                              if item.name.endswith('.kicad_pro')))
    rules = project['board']['design_settings']['rules']
    assert rules['min_hole_to_hole'] == floor_nm / 1_000_000
    # The established copper-to-hole setting remains independent and unchanged.
    assert rules['min_hole_clearance'] == floor_nm / 1_000_000
    other = replace(board, rules=replace(board.rules, minimum_hole_clearance_nm=400_000))
    original_pcb = next(item for item in manifest.artifacts if item.name.endswith('.kicad_pcb'))
    changed_pcb = next(item for item in KiCadPcbBackend().generate(other).artifacts
                       if item.name.endswith('.kicad_pcb'))
    assert original_pcb == changed_pcb


def kicad_cli():
    found = shutil.which('kicad-cli')
    if found:
        return found
    windows = Path('C:/Program Files/KiCad/10.0/bin/kicad-cli.exe')
    if windows.is_file():
        return str(windows)
    pytest.skip('KiCad CLI unavailable: native hole-spacing control not verified')


@pytest.mark.parametrize('floor_nm,gap_nm,expected', (
    (200_000, 220_000, 0),
    (200_000, 180_000, 1),
    (250_000, 220_000, 1),
    (300_000, 270_000, 1),
))
def test_native_hole_spacing_obeys_both_sides_of_authored_floor(tmp_path, floor_nm, gap_nm, expected):
    board = spacing_board(floor_nm, gap_nm)
    pcb = tmp_path / 'HoleSpacing.kicad_pcb'
    write_kicad_project(KiCadPcbBackend().generate(board), pcb)
    before = pcb.read_bytes()
    report = tmp_path / 'drc.json'
    completed = subprocess.run((kicad_cli(), 'pcb', 'drc', '--severity-all', '--format',
        'json', '--output', str(report), str(pcb)), capture_output=True, text=True, timeout=30)
    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert pcb.read_bytes() == before
    data = json.loads(report.read_text(encoding='utf-8'))
    holes = [item for item in data['violations'] if item['type'] == 'hole_to_hole']
    assert len(holes) == expected
    if holes:
        assert f'{floor_nm / 1_000_000:.4f} mm' in holes[0]['description']
