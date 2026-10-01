"""Explicit copper contacts must agree with physical, not endpoint topology."""
from dataclasses import replace
import json
from pathlib import Path
import shutil
import subprocess

import pytest

from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference,
                   PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
                   Point, Size, TrackSegment, Via, BoardSide, PadKind, PadShape,
                   nm_from_mm, run_physical_drc)
from pcbir.copper_connectivity import CopperContact, copper_contact_roots
from pcbir.geometry import RoundedConvexShape
from pcbir.backends.kicad_pcb import KiCadPcbBackend

F, B = CopperLayer.FRONT, CopperLayer.BACK


def fixture_board():
    footprint = PhysicalFootprint("test/pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))
    return PhysicalBoard("Contacts", BoardOutline.rectangle(10, 10), {footprint.name: footprint},
                         (Placement("A", footprint.name, Point.mm(2, 5)),
                          Placement("B", footprint.name, Point.mm(8, 5)),
                          Placement("C", footprint.name, Point.mm(5, 8))),
                         (PhysicalNet("N", tuple(PadReference(ref, "1") for ref in ("A", "B", "C"))),))


def track(a, b, layer=F):
    return TrackSegment("N", Point.mm(*a), Point.mm(*b), nm_from_mm("0.2"), layer)


def opens(board):
    return [f for f in run_physical_drc(board).findings if f.code == "DRC-OPEN-NET"]


@pytest.mark.parametrize("joint", [(5, 5), (5, "5.15")])
def test_interior_t_and_width_contact_join(joint):
    board = replace(fixture_board(), tracks=(track((2, 5), (8, 5)), track((5, 8), joint)))
    assert not opens(board)
    assert run_physical_drc(board) == run_physical_drc(board)


def test_real_gap_at_t_is_still_open():
    assert opens(replace(fixture_board(), tracks=(track((2, 5), (8, 5)), track((5, 8), (5, "5.21")))))


def test_pad_edge_contact_works_without_pad_centre_endpoint():
    board = fixture_board()
    board = replace(board, nets=(replace(board.nets[0], pads=board.nets[0].pads[:2]),),
                    tracks=(track(("2.3", "5.2"), ("7.7", "5.2")),))
    assert not opens(board)
    assert opens(replace(board, tracks=(track(("2.41", "5.2"), ("7.7", "5.2")),)))


def test_via_annulus_overlap_joins_offset_track_interiors():
    board = fixture_board()
    placements = (*board.placements[:2], replace(board.placements[2], side=BoardSide.BACK))
    board = replace(board, placements=placements, tracks=(track((2, 5), (8, 5)), track((5, 8), (5, "5.2"), B)),
                    vias=(Via("N", Point.mm(5, "5.2"), nm_from_mm("0.6"), nm_from_mm("0.3")),))
    assert not opens(board)
    assert opens(replace(board, vias=()))


def contact(identity, start, end, layers=(F,), net="N", radius="0.1"):
    return CopperContact(identity, net, layers, RoundedConvexShape((Point.mm(*start), Point.mm(*end)), nm_from_mm(radius)))


def test_crossing_tracks_only_join_same_net_and_layer():
    a = contact("a", (2, 5), (8, 5))
    b = contact("b", (5, 2), (5, 8))
    roots = copper_contact_roots((a, b))
    assert roots["a"] == roots["b"]
    for foreign in (replace(b, net="OTHER"), replace(b, layers=(B,))):
        roots = copper_contact_roots((a, foreign))
        assert roots["a"] != roots["b"]


def test_blind_span_does_not_connect_back_layer():
    a = contact("a", (2, 5), (8, 5))
    b = contact("b", (5, 2), (5, 8), layers=(B,))
    via = CopperContact("via", "N", (F, CopperLayer.INTERNAL_1), RoundedConvexShape((Point.mm(5, 5),), nm_from_mm("0.3")))
    roots = copper_contact_roots((a, b, via))
    assert roots["a"] == roots["via"] != roots["b"]
    roots = copper_contact_roots((a, b, replace(via, layers=(F, CopperLayer.INTERNAL_1, B))))
    assert len(set(roots.values())) == 1


def test_open_drill_is_not_solid_copper_but_outer_cap_is():
    track_inside = contact("track", (5, 5), ("5.01", 5), radius="0.02")
    via = CopperContact("via", "N", (F, B), RoundedConvexShape((Point.mm(5, 5),), nm_from_mm("0.3")),
                        RoundedConvexShape((Point.mm(5, 5),), nm_from_mm("0.15")))
    roots = copper_contact_roots((track_inside, via))
    assert roots["track"] != roots["via"]
    roots = copper_contact_roots((track_inside, replace(via, capped_layers=(F, B))))
    assert roots["track"] == roots["via"]


def test_plated_pad_spans_layers_but_np_hole_does_not():
    board = fixture_board()
    original = board.footprints["test/pad"]
    through = replace(original, name="test/through", pads=(replace(original.pads[0], kind=PadKind.THROUGH_HOLE,
                      shape=PadShape.CIRCLE, drill=Size.mm("0.3", "0.3")),))
    board = replace(board, footprints={**board.footprints, through.name: through},
                    placements=(board.placements[0], replace(board.placements[1], side=BoardSide.BACK),
                                replace(board.placements[2], footprint=through.name)),
                    tracks=(track((2, 5), (5, 8)), track((5, 8), (8, 5), B)))
    assert not opens(board)
    np = replace(through, pads=(replace(through.pads[0], kind=PadKind.NON_PLATED_THROUGH_HOLE),))
    assert opens(replace(board, footprints={**board.footprints, through.name: np},
                         nets=(replace(board.nets[0], pads=board.nets[0].pads[:2]),)))


def test_duplicate_lands_are_not_virtually_shortened():
    board = fixture_board()
    original = board.footprints["test/pad"]
    dup = replace(original, name="test/duplicate", pads=(*original.pads, replace(original.pads[0], position=Point.mm(0, 1))))
    board = replace(board, footprints={**board.footprints, dup.name: dup},
                    placements=(replace(board.placements[0], footprint=dup.name), *board.placements[1:]),
                    tracks=(track((2, 5), (8, 5)), track((5, 8), (5, 5))))
    assert opens(board)
    assert not opens(replace(board, tracks=(*board.tracks, track((2, 5), (2, 6)))))


@pytest.mark.parametrize("case", ["t_joint", "pad_edge", "real_gap", "layer_cross", "via_overlap"])
def test_native_connectivity_agrees_with_installed_kicad(case, tmp_path):
    cli = shutil.which("kicad-cli")
    windows_cli = Path("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if cli is None and windows_cli.is_file():
        cli = str(windows_cli)
    if cli is None:
        pytest.skip("optional independent KiCad connectivity regression requires installed CLI")
    board = fixture_board()
    tracks = (track((2, 5), (8, 5)), track((5, 8), (5, 5)))
    if case == "pad_edge":
        board = replace(board, nets=(replace(board.nets[0], pads=board.nets[0].pads[:2]),))
        tracks = (track(("2.3", "5.2"), ("7.7", "5.2")),)
    elif case == "real_gap":
        tracks = (tracks[0], track((5, 8), (5, "5.21")))
    elif case in {"layer_cross", "via_overlap"}:
        board = replace(board, placements=(*board.placements[:2], replace(board.placements[2], side=BoardSide.BACK)))
        tracks = (tracks[0], track((5, 8), (5, "5.2"), B))
        if case == "via_overlap":
            board = replace(board, vias=(Via("N", Point.mm(5, "5.2"), nm_from_mm("0.6"), nm_from_mm("0.3")),))
    board = replace(board, tracks=tracks)
    for artifact in KiCadPcbBackend().generate(board).artifacts:
        (tmp_path / artifact.name).write_text(artifact.content, encoding="utf-8")
    output = tmp_path / "drc.json"
    result = subprocess.run([cli, "pcb", "drc", "--format", "json", "--severity-all", "--output",
                             str(output), str(tmp_path / "Contacts.kicad_pcb")], capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    assert bool(json.loads(output.read_text())["unconnected_items"]) == bool(opens(board))
