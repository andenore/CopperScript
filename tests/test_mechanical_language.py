"""Mechanical source must work without importing any example builder."""
from dataclasses import fields
import json
from pathlib import Path

import pytest

from pcbir import compile_design_source, compile_source, nm_from_mm
from pcbir.cli import main
from pcbir.design import Design
from pcbir.model import Board
from pcbir.physical import Point
from pcbir.physicalize import PrototypePhysicalOptions, prototype_physicalize
from pcbir.serializer import board_to_dict
from pcbir.syntax import CopperScriptError


def design(body):
    return compile_design_source(f"board Sample {{ mechanical {{ {body} }} }}", "sample.copper")


def test_circle_separate_from_electrical_ir_and_json():
    result = design("outline circle { diameter = 50mm; center = (25mm, 25mm); }")
    assert isinstance(result, Design)
    assert result.mechanical.outline.circular_boundary.radius_nm == nm_from_mm(25)
    assert "mechanical" not in {f.name for f in fields(Board)}
    assert "mechanical" not in board_to_dict(result.electrical)
    assert board_to_dict(result)["mechanical"]["outline"]["circle"]["radius_nm"] == nm_from_mm(25)
    board = prototype_physicalize(result, PrototypePhysicalOptions(board_width_mm=70))
    assert board.outline == result.mechanical.outline


def test_rectangle_defaults_units_signed_origin_and_rules():
    result = design("""outline rectangle { width = 0.06m; height = 40000um; origin = (-5mm, +2mm); }
        rules { minimum_clearance = 150um; minimum_track_width = 0.15mm; }""")
    board = prototype_physicalize(result)
    assert board.outline.vertices[0] == Point.mm(-5, 2)
    assert board.outline.vertices[2] == Point.mm(55, 42)
    assert board.rules.minimum_clearance_nm == nm_from_mm("0.15")


def test_polygon_cutout_and_hole_lower_to_physical_not_bom():
    result = design("""outline polygon { vertices = [(0mm, 0mm), (40mm, 0mm), (40mm, 30mm), (0mm, 30mm)]; }
        cutout window polygon { vertices = [(20mm, 20mm), (24mm, 20mm), (24mm, 24mm), (20mm, 24mm)]; }
        hole H1 { position = (10mm, 10mm); diameter = 3mm; head_clearance_radius = 3mm; }""")
    board = prototype_physicalize(result)
    assert board.outline.cutouts[0].id == "window"
    assert board.mechanical_holes[0].diameter_nm == nm_from_mm(3)
    assert not result.electrical.components
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    manifest = KiCadPcbBackend().generate(board)
    assert manifest.artifacts


@pytest.mark.parametrize("body, message", [
    ("", "requires exactly one outline"),
    ("outline circle { diameter = -1mm; }", "positive"),
    ("outline circle { diameter = 50V; }", "unsupported Length"),
    ("outline circle { diameter = 50; }", "typed lengths"),
    ("outline circle { diameter = 50.0000001mm; }", "integer nanometres"),
    ("outline circle { radius = 25mm; }", "unknown mechanical property"),
    ("outline circle { diameter = 50mm; diameter = 50mm; }", "duplicate property"),
    ("outline circle { diameter = 50mm; } outline rectangle { width = 50mm; height = 50mm; }", "exactly one"),
    ("outline polygon { vertices = [(0mm,0mm), (10mm,10mm), (0mm,10mm), (10mm,0mm)]; }", "area|intersect"),
    ("outline circle { diameter = 50mm; } hole H { position = (50mm,50mm); diameter = 3mm; }", "material|outline|outside"),
    ("outline circle { diameter = 50mm; } hole H { position = (20mm,20mm); diameter = 3mm; } hole H { position = (30mm,30mm); diameter = 3mm; }", "duplicate mechanical"),
    ("outline rectangle { width = 50mm; height = 50mm; } rules { minimum_clearance = 0mm; }", "positive"),
])
def test_bad_geometry_is_source_located_even_electrical_projection(body, message):
    with pytest.raises(CopperScriptError, match=message) as error:
        compile_source(f"board Invalid {{ mechanical {{ {body} }} }}", "invalid.copper")
    assert error.value.location.filename == "invalid.copper"


def test_duplicate_block_and_board_only_scope():
    with pytest.raises(CopperScriptError, match="only one mechanical block"):
        compile_source("board B { mechanical { outline circle { diameter = 50mm; } } mechanical {} }")
    for kind in ("module", "part", "device"):
        with pytest.raises(CopperScriptError, match="board-only"):
            compile_source(f"{kind} X {{ mechanical {{ }} }}")


def test_generic_cli_compiles_and_exports_independent_source(tmp_path):
    source = tmp_path / "own.copper"
    source.write_text("board Own { mechanical { outline circle { diameter = 36mm; } hole H { position = (10mm,18mm); diameter = 2mm; } } }", encoding="utf-8")
    assert main(["check", str(source)]) == 0
    output = tmp_path / "board.json"
    assert main(["compile", str(source), "-o", str(output)]) == 0
    assert json.loads(output.read_text())["mechanical"]["holes"][0]["id"] == "H"
    pcb = tmp_path / "board.kicad_pcb"
    # Even a too-small fallback rectangle cannot override source geometry.
    assert main(["export-kicad-pcb", str(source), "--allow-proxy-footprints",
                 "--width-mm", "1", "--height-mm", "1", "-o", str(pcb)]) == 0
    text = pcb.read_text()
    assert "(gr_circle" in text and "np_thru_hole" in text


def test_generic_fixed_placement_and_circle_zone():
    result = compile_design_source('''board B {
        use library "tiny";
        component R1: RESISTOR { value = 1kohm; footprint = "0603"; }
        net GND { R1.1; } net S { R1.2; }
        mechanical { outline circle { diameter = 30mm; } }
        constraint fixed_placement(R1) { x = 8mm; y = 9mm; rotation = 45; side = back; }
        constraint copper_zone(GND) { layers = "B.Cu"; inset = 1mm; }
    }''')
    board = prototype_physicalize(result)
    pose = board.placements[0]
    assert pose.position == Point.mm(8, 9) and pose.rotation_degrees == 45
    assert pose.side.value == "back"
    assert max(p.x_nm for p in board.zones[0].outline.outer.vertices) <= nm_from_mm(29)


def test_no_example_modules_or_example_imports_in_engine():
    root = Path(__file__).resolve().parents[1] / "pcbir"
    for name in ("round_led_example", "mechanical_example", "nrf52_example", "hard_macro_trial"):
        assert not (root / f"{name}.py").exists()
    for file in root.rglob("*.py"):
        text = file.read_text(encoding="utf-8")
        assert "from examples" not in text and "import examples" not in text


def test_installed_library_discovery_and_project_registry(monkeypatch):
    from pcbir.library import LIBRARIES, library_factory
    assert "RESISTOR" in library_factory("standard")()
    monkeypatch.setitem(LIBRARIES, "project-local", lambda: {})
    assert library_factory("project-local")() == {}
    assert library_factory("nonexistent-library-name") is None


def test_head_clearance_and_inset_crossing_void_reject():
    with pytest.raises(CopperScriptError, match="head clearance must fit"):
        design("outline rectangle { width = 50mm; height = 50mm; } hole H { position = (2mm, 25mm); diameter = 2mm; head_clearance_radius = 3mm; }")
    result = compile_design_source('''board B {
        use library "tiny";
        component R1: RESISTOR { footprint = "0603"; }
        net GND { R1.1; } net S { R1.2; }
        mechanical {
            outline rectangle { width = 50mm; height = 50mm; }
            cutout K polygon { vertices = [(0.5mm,10mm), (2mm,10mm), (2mm,12mm), (0.5mm,12mm)]; }
        }
        constraint copper_zone(GND) { layers = "B.Cu"; inset = 1mm; }
    }''')
    with pytest.raises(ValueError, match="cutout"):
        prototype_physicalize(result)


@pytest.mark.parametrize("reverse", [False, True])
def test_fixed_rotation_cannot_override_explicit_allowed_angles(reverse):
    constraints = [
        'constraint fixed_placement(R1) { x = 10mm; y = 10mm; rotation = 45; }',
        'constraint allowed_orientations(R1) { values = "0,90"; }',
    ]
    if reverse:
        constraints.reverse()
    result = compile_design_source('board B { use library "tiny"; component R1: RESISTOR { footprint = "0603"; } '
                                  + " ".join(constraints) + " }")
    with pytest.raises(ValueError, match="fixed rotation must be one of"):
        prototype_physicalize(result)


def test_surface_zone_escape_is_an_explicit_generic_cli_option():
    from pcbir.cli import _parser
    default = _parser().parse_args(["route-board", "board.copper"])
    enabled = _parser().parse_args(["route-board", "board.copper", "--stitch-surface-zones"])
    assert not default.stitch_surface_zones
    assert enabled.stitch_surface_zones
