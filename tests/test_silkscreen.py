"""Source artwork survives physical lowering and native board export."""
from dataclasses import replace
from decimal import Decimal
import json
import os
from pathlib import Path
import subprocess
import shutil

import pytest

from pcbir import compile_design_source, compile_source
from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.cli import main
from pcbir.drc import physical_board_digest
from pcbir.physical import (BoardOutline, BoardSide, FootprintPad, PhysicalBoard,
                            PhysicalFootprint, Placement, Point, Size)
from pcbir.physicalize import prototype_physicalize
from pcbir.process_drc import (FabricationAssemblyProfile, ProcessCapability,
                               ProcessGateStatus, run_process_drc)
from pcbir.serializer import board_to_dict
from pcbir.silkscreen import SilkscreenGraphic, SilkscreenText, compiler_version
from pcbir.syntax import CopperScriptError


ARTWORK = '''silkscreen {
    branding { enabled = false; }
    text TITLE { text = "Sensor \\"A\\"\\nRev 1"; position = (8mm, 10mm); side = back; rotation = 90; }
    line L { start = (1mm, 2mm); end = (3mm, 2mm); thickness = 200um; }
    rectangle R { start = (1mm, 4mm); end = (3mm, 6mm); filled = true; }
    circle C { center = (5mm, 5mm); radius = 1mm; side = back; }
    arc A { start = (5mm, 8mm); mid = (6mm, 9mm); end = (7mm, 8mm); }
    polygon P { vertices = [(10mm, 2mm), (12mm, 2mm), (11mm, 4mm)]; filled = true; thickness = 0mm; }
    logo CU { position = (20mm, 10mm); size = 5mm; rotation = 90; side = back; }
}'''


def physical(body=""):
    return prototype_physicalize(compile_design_source(f"board Artwork {{ {body} }}"))


def pcb(board):
    return KiCadPcbBackend().generate(board).artifacts[0].content


def test_default_mark_has_actual_logo_and_compiler_version_without_bom_items():
    design = compile_design_source("board Default {}")
    board = prototype_physicalize(design)
    assert not board.placements and not board.nets and not board.footprints
    assert not design.electrical.components
    cu = [item for item in board.silkscreen if ":cu:" in item.id]
    assert len(cu) == 2 and all(isinstance(item, SilkscreenGraphic) for item in cu)
    texts = {item.text for item in board.silkscreen if isinstance(item, SilkscreenText)}
    assert texts == {"29", "CopperScript", f"v{compiler_version()}"}
    assert pcb(board) == pcb(board)
    assert '(gr_poly' in pcb(board) and f'"v{compiler_version()}"' in pcb(board)
    assert physical("silkscreen {}").silkscreen == physical().silkscreen


def test_default_mark_fits_curved_outline_material_when_space_allows():
    board = physical("mechanical { outline circle { diameter = 40mm; center = (20mm, 20mm); } }")
    from pcbir.geometry import RoundedConvexShape
    from pcbir.mechanical import shape_in_outline
    frame = next(item.graphic for item in board.silkscreen if item.id == "__copperscript:frame")
    assert shape_in_outline(RoundedConvexShape(frame.points, (frame.width_nm+1)//2), board.outline, 0)


def test_explicit_opt_out_does_not_remove_user_or_footprint_artwork():
    board = physical('''silkscreen { branding { enabled = false; }
        text LABEL { text = "Mine"; position = (5mm, 5mm); } }''')
    assert [item.id for item in board.silkscreen] == ["LABEL"]
    assert "CopperScript" not in [item.text for item in board.silkscreen]
    assert not physical("silkscreen { branding { enabled = false; } }").silkscreen


def test_shapes_exact_units_text_escaping_back_mirroring_and_rotation():
    board = physical(ARTWORK)
    line = next(item.graphic for item in board.silkscreen if item.id == "L")
    assert line.width_nm == 200_000 and line.start == Point(1_000_000, 2_000_000)
    text = next(item for item in board.silkscreen if item.id == "TITLE")
    assert text.side is BoardSide.BACK and text.rotation_degrees == Decimal(90)
    exported = pcb(board)
    for kind in ("text", "line", "rect", "circle", "arc", "poly"):
        assert f"(gr_{kind}" in exported
    assert '(justify mirror)' in exported and '(at 8 10 -90)' in exported
    assert 'Sensor \\"A\\"\\nRev 1' in exported
    assert '(layer "F.SilkS")' in exported and '(layer "B.SilkS")' in exported


def test_branding_custom_pose_and_version_are_captured_in_physical_ir(monkeypatch):
    monkeypatch.setattr("pcbir.silkscreen.compiler_version", lambda: "9.8.7")
    board = physical('''silkscreen { branding {
        position = (-5mm, 10mm); side = back; size = 4mm; rotation = 90;
    } }''')
    version = next(item for item in board.silkscreen if item.id == "__copperscript:version")
    assert version.text == "v9.8.7" and version.side is BoardSide.BACK
    assert '"v9.8.7"' in pcb(board)


@pytest.mark.parametrize("side, rotation, name_at, version_at", [
    ("front", 0, (27, 35), (27, 37.4)),
    ("back", 90, (21, 29), (18.6, 29)),
])
def test_stacked_branding_has_compact_labels_and_captures_version(
        monkeypatch, side, rotation, name_at, version_at):
    monkeypatch.setattr("pcbir.silkscreen.compiler_version", lambda: "9.8.7")
    design = compile_design_source(f'''board B {{ silkscreen {{ branding {{
        layout = stacked; position = (27mm, 29mm); size = 8mm;
        side = {side}; rotation = {rotation}; material = copper;
    }} }} }}''')
    value = prototype_physicalize(design)
    labels = {item.id: item for item in value.silkscreen if isinstance(item, SilkscreenText)}
    name, version = labels["__copperscript:name"], labels["__copperscript:version"]
    assert name.position == Point.mm(*name_at) and version.position == Point.mm(*version_at)
    assert name.size_nm == version.size_nm == 800_000
    assert name.thickness_nm == version.thickness_nm == 160_000
    assert version.text == "v9.8.7"
    assert len(value.copper_keepouts) == 6
    assert board_to_dict(design)["silkscreen"]["branding"]["layout"] == "stacked"


def test_stacked_branding_auto_position_fits_curved_board():
    from pcbir.geometry import RoundedConvexShape
    from pcbir.mechanical import shape_in_outline
    from pcbir.silkscreen import artwork_bounds
    value = physical('''mechanical { outline circle { diameter = 50mm; center = (25mm, 25mm); } }
        silkscreen { branding { layout = stacked; size = 8mm; rotation = 90; side = back; } }''')
    for item in value.silkscreen:
        left, low, right, high = artwork_bounds(item)
        corners = tuple(Point(x, y) for x, y in ((left, low), (right, low), (right, high), (left, high)))
        assert shape_in_outline(RoundedConvexShape(corners, 0), value.outline, 0)


def test_branding_layout_rejects_invalid_value_with_source_location():
    with pytest.raises(CopperScriptError, match="horizontal or stacked") as error:
        compile_design_source("board B { silkscreen { branding { layout = diagonal; } } }", "art.copper")
    assert error.value.location.filename == "art.copper"


def test_artwork_changes_invalidate_physical_evidence_and_leave_electrical_ir_equal():
    a = compile_design_source('board B { silkscreen { branding { enabled = false; } } }')
    b = compile_design_source('board B { silkscreen { branding { enabled = false; } text T { text = "X"; position = (1mm, 2mm); } } }')
    assert a.electrical == b.electrical
    assert "silkscreen" not in board_to_dict(a.electrical)
    assert board_to_dict(a)["silkscreen"]["branding"]["enabled"] is False
    assert board_to_dict(b)["silkscreen"]["artwork"][0]["position"] == {"x_nm": 1_000_000, "y_nm": 2_000_000}
    first, second = prototype_physicalize(a), prototype_physicalize(b)
    assert physical_board_digest(first) != physical_board_digest(second)
    changed = replace(second, silkscreen=(replace(second.silkscreen[0], text="Y"),))
    assert physical_board_digest(changed) != physical_board_digest(second)
    default = physical()
    changed_version = replace(default, silkscreen=tuple(
        replace(item, text="v2.0.0") if item.id == "__copperscript:version" else item
        for item in default.silkscreen))
    assert physical_board_digest(default) != physical_board_digest(changed_version)


@pytest.mark.parametrize("body, message", [
    ('text T { position = (1mm, 2mm); }', "missing.*text"),
    ('text T { text = ""; position = (1mm, 2mm); }', "nonempty"),
    ('text T { text = "x"; position = (1, 2mm); }', "typed lengths"),
    ('text T { text = "x"; position = (1V, 2mm); }', "unsupported Length"),
    ('text T { text = "x"; position = (1.0000001mm, 2mm); }', "integer nanometres"),
    ('text T { text = "x"; position = (1mm, 2mm); size = 0mm; }', "positive"),
    ('text T { text = "x"; position = (1mm, 2mm); thickness = 0.6mm; }', "half"),
    ('text T { text = "x"; position = (1mm, 2mm); rotation = 90mm; }', "degrees"),
    ('text T { text = "x"; position = (1mm, 2mm); side = inner; }', "BoardSide"),
    ('branding { enabled = 0; }', "boolean"),
    ('branding { enabled = false; } branding {}', "duplicate"),
    ('branding { version = "fake"; }', "unknown"),
    ('circle C { center = (0mm, 0mm); radius = -1mm; }', "positive"),
    ('rectangle R { start = (0mm, 0mm); end = (1mm, 0mm); }', "area"),
    ('line L { start = (0mm, 0mm); end = (1mm, 0mm); thickness = 0mm; }', "positive thickness"),
    ('arc A { start = (0mm, 0mm); mid = (1mm, 0mm); end = (2mm, 0mm); }', "collinear"),
    ('polygon P { vertices = [(0mm, 0mm), (2mm, 2mm), (0mm, 2mm), (2mm, 0mm)]; }', "area|intersect"),
    ('logo __copperscript { position = (1mm, 2mm); }', "reserved"),
])
def test_invalid_artwork_is_source_located_even_in_electrical_projection(body, message):
    with pytest.raises(CopperScriptError, match=message) as exc:
        compile_source(f"board B {{ silkscreen {{ {body} }} }}", "invalid.copper")
    assert exc.value.location.filename == "invalid.copper"


def test_single_block_and_board_only():
    with pytest.raises(CopperScriptError, match="only one silkscreen"):
        compile_source("board B { silkscreen {} silkscreen {} }")
    from pcbir.parser import parse
    for kind in ("module", "part", "device", "board_profile"):
        with pytest.raises(CopperScriptError, match="board-only"):
            parse(f"{kind} X {{ silkscreen {{ }} }}")


def test_cli_compile_and_board_export_preserve_opt_out(tmp_path):
    source = tmp_path / "board.copper"
    source.write_text('board CLI { silkscreen { branding { enabled = false; } text L { text = "CLI"; position = (5mm, 5mm); } } }', encoding="utf-8")
    compiled, exported = tmp_path / "board.json", tmp_path / "board.kicad_pcb"
    assert main(["compile", str(source), "-o", str(compiled)]) == 0
    assert json.loads(compiled.read_text())["silkscreen"]["branding"]["enabled"] is False
    assert main(["export-kicad-pcb", str(source), "--allow-proxy-footprints", "-o", str(exported)]) == 0
    assert '(gr_text "CLI"' in exported.read_text()
    assert '(gr_text "CopperScript"' not in exported.read_text()


def test_lsp_recovers_and_completes_artwork_and_reports_source_errors(tmp_path):
    from pcbir.lsp.analysis import Workspace
    from pcbir.lsp.frontend import parse_recovering
    from pcbir.lsp.text import Text
    source = "board B { silkscreen { branding { en } } }"
    path = tmp_path / "board.copper"
    ws = Workspace()
    ws.update(path.as_uri(), 1, source)
    labels = {c["label"] for c in ws.analysis(path.as_uri()).completion(Text(source).position(source.index(" en ") + 3))}
    assert "enabled" in labels
    valid = "board B { " + ARTWORK + " }"
    assert not parse_recovering(valid, str(path)).errors
    ws.update(path.as_uri(), 2, 'board B { silkscreen { branding { enabled = 1; } } }')
    assert ws.analysis(path.as_uri()).diagnostics()[0]["code"] == "SLK002"


def test_native_artwork_roundtrip(tmp_path):
    interpreter = os.environ.get("KICAD_PYTHON")
    if not interpreter:
        pytest.skip("set KICAD_PYTHON to verify artwork with the native board parser")
    path = tmp_path / "artwork.kicad_pcb"
    path.write_text(pcb(physical(ARTWORK)), encoding="utf-8")
    probe = '''import pcbnew, sys
b = pcbnew.LoadBoard(sys.argv[1])
items = list(b.GetDrawings())
text = next(i for i in items if isinstance(i, pcbnew.PCB_TEXT) and i.GetText().startswith('Sensor'))
assert text.IsMirrored() and text.GetLayer() == pcbnew.B_SilkS
assert text.GetTextAngle().AsDegrees() == -90
assert len(items) == 14, len(items)
pcbnew.SaveBoard(sys.argv[2], b)
assert len(list(pcbnew.LoadBoard(sys.argv[2]).GetDrawings())) == 14
'''
    result = subprocess.run([interpreter, "-c", probe, str(path), str(tmp_path / "roundtrip.kicad_pcb")], capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    path.write_text(pcb(physical()), encoding="utf-8")
    probe = '''import pcbnew, sys
b = pcbnew.LoadBoard(sys.argv[1])
text = {i.GetText() for i in b.GetDrawings() if isinstance(i, pcbnew.PCB_TEXT)}
assert text == {'29', 'CopperScript', sys.argv[2]}, text
'''
    result = subprocess.run([interpreter, "-c", probe, str(path), f"v{compiler_version()}"], capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr


def test_native_gerbers_include_both_artwork_layers(tmp_path):
    interpreter = os.environ.get("KICAD_PYTHON")
    cli = shutil.which("kicad-cli")
    if not cli and interpreter:
        candidate = Path(interpreter).with_name("kicad-cli.exe")
        cli = str(candidate) if candidate.is_file() else None
    if not cli:
        pytest.skip("native KiCad CLI is unavailable")
    source = tmp_path / "artwork.kicad_pcb"
    # Keep the generated default alongside explicitly authored back artwork.
    source.write_text(pcb(physical('silkscreen { text T { text = "Back"; position = (20mm, 20mm); side = back; } }')), encoding="utf-8")
    output = tmp_path / "gerbers"
    result = subprocess.run([cli, "pcb", "export", "gerbers", "--layers", "F.SilkS,B.SilkS",
                             "--output", str(output), str(source)], capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    layers = list(output.glob("*.gto")) + list(output.glob("*.gbo")) + list(output.glob("*.gbr"))
    assert len(layers) == 2
    assert all("D01*" in layer.read_text() for layer in layers)


def test_process_checks_lines_on_their_side_and_exposes_unsupported_artwork():
    front = physical('''silkscreen { branding { enabled = false; }
        line L { start = (4mm, 5mm); end = (6mm, 5mm); side = front; }
    }''').silkscreen
    footprint = PhysicalFootprint("pad", (FootprintPad("1", Point(0, 0), Size.mm(1, 1)),), Size.mm(2, 2))
    board = PhysicalBoard("Process", BoardOutline.rectangle(10, 10), {"pad": footprint},
        (Placement("U", "pad", Point.mm(5, 5)),), (), silkscreen=front)
    cap = lambda n: ProcessCapability(n, "fabricator", "1")
    profile = FabricationAssemblyProfile("p", cap(100_000), cap(100_000), cap(100_000), cap(500_000),
        require_courtyards=False, minimum_silkscreen_clearance_nm=cap(200_000))
    assert run_process_drc(board, profile).fabrication is ProcessGateStatus.FAIL
    back = replace(board, silkscreen=tuple(replace(item, side=BoardSide.BACK) for item in front))
    assert run_process_drc(back, profile).fabrication is ProcessGateStatus.PASS
    marked = replace(board, silkscreen=physical().silkscreen)
    report = run_process_drc(marked, profile)
    assert report.fabrication is ProcessGateStatus.INCOMPLETE
    assert any(f.code == "FAB-SILK-ARTWORK-UNSUPPORTED" for f in report.findings)
