from pathlib import Path
from dataclasses import replace
import json
import re
import uuid
import os
import shutil
import subprocess
import xml.etree.ElementTree as ET

import pytest

from pcbir import KiCadSchematicBackend, compile_file
from pcbir.loader import load_board
from pcbir.backends.kicad_project import write_kicad_schematic


ROOT = Path(__file__).parents[1]


def test_kicad_backend_generates_self_contained_deterministic_schematic() -> None:
    board = compile_file(ROOT / "examples/valid_board/board.copper")
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
    assert '(property "Value" "100 nF"' in schematic
    assert '(property "Value" "1E+2 nF"' not in schematic
    assert '(property "Footprint" "LQFP-48"' in schematic
    assert '(label "I2C_SDA"' in schematic
    assert '(label "V3V3"' in schematic
    assert _parentheses_are_balanced(schematic)
    assert len(first.artifacts) == 4
    library, table = first.artifacts[1:3]
    assert library.name in table.content
    assert '(symbol "CopperScript:' not in library.content
    assert _parentheses_are_balanced(library.content)

    identifiers = re.findall(r'\(uuid "([0-9a-f-]+)"\)', schematic)
    assert identifiers
    assert len(identifiers) == len(set(identifiers))
    assert all(uuid.UUID(identifier).version == 4 for identifier in identifiers)


def test_kicad_backend_maps_effective_device_pad_capabilities() -> None:
    board = compile_file(ROOT / "examples/valid_board/board.copper")
    schematic = KiCadSchematicBackend().generate(board).artifacts[0].content

    assert '(pin power_in line\n          (at -10.16' in schematic
    assert '(pin bidirectional line' in schematic
    # Analog capability does not make resistor/capacitor pins active drivers.
    assert '(pin passive line' in schematic


def test_kicad_backend_explicitly_flattens_hierarchy() -> None:
    board = compile_file(ROOT / "examples/hierarchical_board/board.copper")
    manifest = KiCadSchematicBackend().generate(board)
    schematic = manifest.artifacts[0].content

    assert manifest.warnings
    assert "flat single-sheet" in manifest.warnings[0]
    assert '(property "Reference" "PWR_U1"' in schematic
    assert '(property "CopperScriptPath" "PWR/U1"' in schematic
    assert '(label "V3V3"' in schematic


def test_smd_cortex_debug_connectors_are_in_schematic_bom() -> None:
    board = load_board(ROOT / "examples/full_vertical/board.copper")
    target = board.library["swd.SWD_HEADER"]
    assert target.assembled
    assert target.footprints == ("Connector_Debug:FTSH-105-01-L-DV-007-K",)
    schematic = KiCadSchematicBackend().generate(board).artifacts[0].content
    assert schematic.count("(in_bom yes)") >= 3


def test_native_netlist_preserves_declared_pin_connectivity(tmp_path):
    cli = os.environ.get("KICAD_CLI") or shutil.which("kicad-cli")
    if not cli:
        pytest.skip("native schematic connectivity test requires KICAD_CLI")
    board = compile_file(ROOT / "examples/valid_board/board.copper")
    schematic = tmp_path / "ValidSensorBoard.kicad_sch"
    write_kicad_schematic(KiCadSchematicBackend().generate(board), schematic)
    netlist = tmp_path / "netlist.xml"
    result = subprocess.run([cli, "sch", "export", "netlist", "--format", "kicadxml",
                             "--output", str(netlist), str(schematic)],
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    actual = {net.attrib["name"].lstrip("/"): {
        (node.attrib["ref"], node.attrib["pin"]) for node in net.findall("node")}
        for net in ET.parse(netlist).findall("./nets/net")}
    components = {c.ref:c for c in board.components}
    for net in board.nets:
        expected = {(e.component, board.library[components[e.component].part].pins[e.pin].number)
                    for e in net.endpoints}
        assert actual[net.name] == expected, net.name


def test_schematic_export_preserves_unrelated_tables_and_checks_paths(tmp_path):
    manifest = KiCadSchematicBackend().generate(compile_file(ROOT / "examples/valid_board/board.copper"))
    output = tmp_path / "nested" / "renamed.kicad_sch"
    paths = write_kicad_schematic(manifest, output)
    assert len(paths) == 4 and output.exists()
    project = output.with_suffix(".kicad_pro")
    project.write_text('{"user": "settings"}', encoding="utf-8")
    assert write_kicad_schematic(manifest, output) == paths
    assert json.loads(project.read_text()) == {"user": "settings"}
    table = output.parent / "sym-lib-table"
    table.write_text("user library table", encoding="utf-8")
    with pytest.raises(FileExistsError, match="unrelated"):
        write_kicad_schematic(manifest, output)
    assert table.read_text() == "user library table"
    bad = replace(manifest, artifacts=(manifest.artifacts[0],
        replace(manifest.artifacts[1], name="../outside.kicad_sym"), *manifest.artifacts[2:]))
    with pytest.raises(ValueError, match="unsafe"):
        write_kicad_schematic(bad, tmp_path / "fresh" / "board.kicad_sch")
    assert not (tmp_path / "fresh").exists()


def test_native_schematic_symbol_library_matches_after_relocation(tmp_path):
    cli = os.environ.get("KICAD_CLI") or shutil.which("kicad-cli")
    if not cli:
        pytest.skip("native schematic library test requires KICAD_CLI")
    source = tmp_path / "original"
    output = source / "renamed.kicad_sch"
    manifest = KiCadSchematicBackend().generate(compile_file(ROOT / "examples/valid_board/board.copper"))
    write_kicad_schematic(manifest, output)
    relocated = tmp_path / "relocated project with spaces"
    shutil.copytree(source, relocated)
    report = relocated / "erc.json"
    result = subprocess.run([cli, "sch", "erc", str(relocated / output.name),
        "--format", "json", "--output", str(report)], capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    violations = [v for sheet in json.loads(report.read_text(encoding="utf-8"))["sheets"]
                  for v in sheet["violations"]]
    assert not [v for v in violations if v["type"] in {"lib_symbol_issues", "lib_symbol_mismatch"}]


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
