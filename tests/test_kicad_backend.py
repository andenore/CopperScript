from pathlib import Path
import re
import uuid

from pcbir import KiCadSchematicBackend, compile_file
from pcbir.loader import load_board


ROOT = Path(__file__).parents[1]


def test_kicad_backend_generates_self_contained_deterministic_schematic() -> None:
    board = compile_file(ROOT / "examples" / "valid_board.copper")
    backend = KiCadSchematicBackend()

    first = backend.generate(board)
    second = backend.generate(board)

    assert first == second
    assert first.backend == "kicad-schematic"
    assert first.target_version == "8.0"
    assert first.warnings == ()
    assert first.artifacts[0].name == "ValidSensorBoard.kicad_sch"
    schematic = first.artifacts[0].content
    assert schematic.startswith("(kicad_sch\n  (version 20231120)")
    assert '(generator "copperscript")' in schematic
    assert '(lib_symbols' in schematic
    assert '(lib_id "CopperScript:' in schematic
    assert '(property "Reference" "U2"' in schematic
    assert '(property "Value" "4.7 kohm"' in schematic
    assert '(property "Footprint" "LQFP-48"' in schematic
    assert '(label "I2C_SDA"' in schematic
    assert '(label "V3V3"' in schematic
    assert _parentheses_are_balanced(schematic)

    identifiers = re.findall(r'\(uuid "([0-9a-f-]+)"\)', schematic)
    assert identifiers
    assert len(identifiers) == len(set(identifiers))
    assert all(uuid.UUID(identifier).version == 4 for identifier in identifiers)


def test_kicad_backend_maps_effective_device_pad_capabilities() -> None:
    board = compile_file(ROOT / "examples" / "valid_board.copper")
    schematic = KiCadSchematicBackend().generate(board).artifacts[0].content

    assert '(pin power_in line\n          (at -10.16' in schematic
    assert '(pin bidirectional line' in schematic


def test_kicad_backend_explicitly_flattens_hierarchy() -> None:
    board = compile_file(ROOT / "examples" / "hierarchical_board.copper")
    manifest = KiCadSchematicBackend().generate(board)
    schematic = manifest.artifacts[0].content

    assert manifest.warnings
    assert "flat single-sheet" in manifest.warnings[0]
    assert '(property "Reference" "PWR_U1"' in schematic
    assert '(property "CopperScriptPath" "PWR/U1"' in schematic
    assert '(label "V3V3"' in schematic


def test_smd_cortex_debug_connectors_are_in_schematic_bom() -> None:
    board = load_board(ROOT / "examples" / "full_vertical_board.copper")
    target = board.library["swd.SWD_HEADER"]
    assert target.assembled
    assert target.footprints == ("Connector_Debug:FTSH-105-01-L-DV-007-K",)
    schematic = KiCadSchematicBackend().generate(board).artifacts[0].content
    assert schematic.count("(in_bom yes)") >= 3


def _parentheses_are_balanced(text: str) -> bool:
    depth = 0
    quoted = False
    escaped = False
    for character in text:
        if escaped:
            escaped = False
        elif character == "\\" and quoted:
            escaped = True
        elif character == '"':
            quoted = not quoted
        elif not quoted and character == "(":
            depth += 1
        elif not quoted and character == ")":
            depth -= 1
            if depth < 0:
                return False
    return depth == 0 and not quoted
