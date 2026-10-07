"""A redundant route must not hide a lost electrical contact on one via layer."""
from collections import Counter
from dataclasses import replace
import json
from pathlib import Path
import shutil
import subprocess

import pytest

from pcbir import (BoardOutline, BoardSide, CopperLayer, FootprintPad, PadReference,
                   PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point,
                   Size, TrackSegment, Via, nm_from_mm)
from pcbir.copper_connectivity import (CopperContact, copper_contacts_overlap,
                                      via_copper_contact)
from pcbir.geometry import RoundedConvexShape
from pcbir.route_cleanup import _Settler, _net_pad_shapes
from pcbir.route_smoothing import _contacts_preserved, smooth_owned_tracks
from pcbir.routing_clearance import RoutingClearanceIndex

F, B = CopperLayer.FRONT, CopperLayer.BACK


def track(a, b, layer=F):
    return TrackSegment("N", Point.mm(*a), Point.mm(*b), nm_from_mm("0.2"), layer)


def via_layer_board():
    footprint = PhysicalFootprint("pad", (
        FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))
    # The second via keeps all pads connected even if the first loses F.Cu.
    fixed = (track((8, "5.4"), ("8.8", "5.4")), track((5, "5.8"), (5, 8), B),
             track(("8.8", "5.4"), ("8.8", 8), B), track(("8.8", 8), (5, 8), B))
    vias = tuple(Via("N", Point.mm(*xy), nm_from_mm("0.6"), nm_from_mm("0.3"))
                 for xy in ((5, "5.8"), ("8.8", "5.4")))
    board = PhysicalBoard("ViaLayer", BoardOutline.rectangle(14, 12), {footprint.name: footprint},
        (Placement("A", footprint.name, Point.mm(3, "5.4")),
         Placement("B", footprint.name, Point.mm(8, "5.4")),
         Placement("C", footprint.name, Point.mm(5, 8), side=BoardSide.BACK)),
        (PhysicalNet("N", tuple(PadReference(ref, "1") for ref in "ABC")),), tracks=fixed, vias=vias)
    points = ((3, "5.4"), ("3.25", "5.65"), ("7.75", "5.65"), (8, "5.4"))
    before = tuple(track(a, b) for a, b in zip(points, points[1:]))
    return board, before, (track(points[0], points[-1]),)


def test_smoothing_preserves_redundant_positive_via_layer_contact():
    board, before, tangent = via_layer_board()
    assert not _contacts_preserved(board, board.nets[0], before, tangent)
    complete = replace(board, tracks=(*board.tracks, *before))
    result = smooth_owned_tracks(board, before, RoutingClearanceIndex(complete))
    assert _contacts_preserved(board, board.nets[0], before, result)
    assert result != tangent


@pytest.mark.parametrize("finish", ["standard", "filled-capped"])
def test_via_contact_preserves_drill_cap_and_actual_span(finish):
    inner = CopperLayer.INTERNAL_1
    layers = (F, inner, B)
    via = Via("N", Point.mm(5, 5), nm_from_mm("0.6"), nm_from_mm("0.3"), finish=finish)
    contact = via_copper_contact(via, layers)
    inside = RoundedConvexShape((Point.mm(5, 5), Point.mm("5.1", 5)), nm_from_mm("0.02"))
    for layer in layers:
        copper = CopperContact("track", "N", (layer,), inside)
        assert copper_contacts_overlap(contact, copper) is (finish == "filled-capped" and layer in (F, B))
    blind = via_copper_contact(replace(via, to_layer=inner), layers)
    annulus = RoundedConvexShape((Point.mm("5.2", 5),), nm_from_mm("0.02"))
    assert copper_contacts_overlap(blind, CopperContact("track", "N", (inner,), annulus))
    assert not copper_contacts_overlap(blind, CopperContact("track", "N", (B,), annulus))


def test_cleanup_layer_count_rejects_robust_boundary_tangency_and_open_drill():
    board, _, tangent = via_layer_board()
    settle = _Settler(board, "N", board.nets[0], _net_pad_shapes(board, "N"),
                     {layer: i for i, layer in enumerate(board.stackup.copper_layers)},
                     RoutingClearanceIndex(board), Counter(), Counter())
    # Its annulus exactly touches the track after the cleanup contact margin.
    via = replace(board.vias[0], position=Point.mm(5, "5.799"))
    assert settle.via_layers(via, tangent) == set()
    inside = replace(tangent[0], start=via.position, end=Point.mm("5.1", "5.799"),
                     width_nm=nm_from_mm("0.04"))
    assert settle.via_layers(via, (inside,)) == set()
    assert settle.via_layers(replace(via, finish="filled-capped"), (inside,)) == {F}


def test_native_smoothing_does_not_create_dangling_via(tmp_path):
    cli = shutil.which("kicad-cli")
    windows_cli = Path("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if cli is None and windows_cli.is_file():
        cli = str(windows_cli)
    if cli is None:
        pytest.skip("optional native via-layer regression requires installed KiCad CLI")
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project

    board, before, _ = via_layer_board()
    complete = replace(board, tracks=(*board.tracks, *before))
    smoothed = smooth_owned_tracks(board, before, RoutingClearanceIndex(complete))
    write_kicad_project(KiCadPcbBackend().generate(replace(board, tracks=(*board.tracks, *smoothed))),
                        tmp_path / "board.kicad_pcb")
    report = tmp_path / "drc.json"
    result = subprocess.run([cli, "pcb", "drc", "--all-track-errors", "--severity-all", "--format", "json",
                             "--output", str(report), str(tmp_path / "board.kicad_pcb")],
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    findings = json.loads(report.read_text())
    assert not findings["unconnected_items"]
    assert not findings["violations"]
