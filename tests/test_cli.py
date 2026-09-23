import subprocess
import sys
import json
from pathlib import Path


ROOT = Path(__file__).parents[1]


def run_cli(example: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "copperscript", "check", f"examples/{example}"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_cli_accepts_valid_copper_board() -> None:
    result = run_cli("valid_board.copper")
    assert result.returncode == 0
    assert "passed ERC" in result.stdout


def test_cli_rejects_invalid_copper_board() -> None:
    result = run_cli("invalid_board.copper")
    assert result.returncode == 1
    assert "OUTPUT_CONFLICT" in result.stdout
    assert "I2C_MISSING_PULLUP" in result.stdout


def test_cli_footprint_audit_json_is_machine_readable() -> None:
    command = [sys.executable, "-m", "copperscript", "audit-footprints",
               "examples/valid_board.copper", "--json"]
    first = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
    second = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
    assert first.stdout == second.stdout
    document = json.loads(first.stdout)
    assert document["schema"] == "copperscript-footprint-audit/v0.1"
    assert document["total"] == len(document["entries"])
    assert first.returncode == (0 if document["passed"] else 1)


def test_cli_compiles_to_json() -> None:
    result = subprocess.run(
        [sys.executable, "-m", "copperscript", "compile", "examples/valid_board.copper"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0
    assert '"schema": "copperscript-ir/v0.1"' in result.stdout
    assert '"name": "ValidSensorBoard"' in result.stdout
    assert '"name": "STM32_LIKE"' in result.stdout


def test_cli_json_preserves_hierarchy() -> None:
    result = subprocess.run(
        [sys.executable, "-m", "copperscript", "compile", "examples/hierarchical_board.copper"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0
    assert '"module_definitions"' in result.stdout
    assert '"module": "power.Buck5VTo3V3"' in result.stdout
    assert '"ref": "PWR"' in result.stdout
    assert '"ref": "PWR/U1"' not in result.stdout


def test_cli_rejects_python_board_sources() -> None:
    result = run_cli("../tests/fixtures/python_ir/valid_board.py")
    assert result.returncode == 2
    assert "expected .copper" in result.stdout


def test_cli_runs_power_state_analysis() -> None:
    result = subprocess.run(
        [sys.executable, "-m", "copperscript", "power-check", "examples/valid_board.copper"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0
    assert "passed power-state analysis" in result.stdout


def test_cli_exports_kicad_schematic(tmp_path: Path) -> None:
    output = tmp_path / "valid.kicad_sch"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad",
            "examples/valid_board.copper",
            "-o",
            str(output),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0
    assert "Generated KiCad 8.0 schematic" in result.stdout
    assert output.read_text(encoding="utf-8").startswith("(kicad_sch")


def test_cli_refuses_kicad_export_when_erc_fails(tmp_path: Path) -> None:
    output = tmp_path / "invalid.kicad_sch"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad",
            "examples/invalid_board.copper",
            "-o",
            str(output),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 1
    assert "stopped because ERC reported errors" in result.stdout
    assert not output.exists()


def test_cli_exports_prototype_kicad_pcb(tmp_path: Path) -> None:
    output = tmp_path / "valid.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad-pcb",
            "examples/valid_board.copper",
            "-o",
            str(output),
            "--allow-proxy-footprints",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0
    assert "Generated KiCad 8.0 PCB" in result.stdout
    assert "proxy footprints" in result.stdout
    assert output.read_text(encoding="utf-8").startswith("(kicad_pcb")


def test_cli_exports_pcb_with_resolved_kicad_mod(tmp_path: Path) -> None:
    output = tmp_path / "resolved.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad-pcb",
            "examples/resolved_footprint_board.copper",
            "-o",
            str(output),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout
    assert "proxy footprints" not in result.stdout
    content = output.read_text(encoding="utf-8")
    assert '(footprint "footprints/R_0402_CopperScript.kicad_mod"' in content
    assert '(fp_rect' in content


def test_cli_exports_provisional_four_layer_board(tmp_path: Path) -> None:
    output = tmp_path / "four-layer.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable, "-m", "copperscript", "export-kicad-pcb",
            "examples/resolved_footprint_board.copper", "--layers", "4",
            "-o", str(output),
        ],
        cwd=ROOT, text=True, capture_output=True, check=False,
    )

    assert result.returncode == 0, result.stdout
    content = output.read_text(encoding="utf-8")
    assert '(1 "In1.Cu" signal)' in content
    assert '(2 "In2.Cu" signal)' in content


def test_cli_does_not_silently_fall_back_to_proxy_footprints(
    tmp_path: Path,
) -> None:
    output = tmp_path / "unresolved.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad-pcb",
            "examples/valid_board.copper",
            "-o",
            str(output),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 2
    assert "cannot resolve footprint" in result.stdout
    assert not output.exists()


def test_cli_plans_layout_and_writes_readiness_report(tmp_path: Path) -> None:
    output = tmp_path / "planned.kicad_pcb"
    report = tmp_path / "layout-report.json"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "plan-layout",
            "examples/valid_board.copper",
            "-o",
            str(output),
            "--report",
            str(report),
            "--allow-proxy-footprints",
            "--candidates",
            "2",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout
    assert output.read_text(encoding="utf-8").startswith("(kicad_pcb")
    document = json.loads(report.read_text(encoding="utf-8"))
    assert document["schema"] == "copperscript-layout-report/v0.1"
    assert 1 <= len(document["candidates"]) <= 2
    assert document["selected_candidate"].startswith("candidate-")
    assert "PLACE:" in result.stdout
    assert "ROUTE: not_run" in result.stdout
    assert "VERIFY: blocked" in result.stdout
    assert "Pareto candidate" in result.stdout


def test_cli_writes_global_routing_guides(tmp_path: Path) -> None:
    output = tmp_path / "global-route.json"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "route-global",
            "examples/valid_board.copper",
            "-o",
            str(output),
            "--allow-proxy-footprints",
            "--candidates",
            "1",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout
    document = json.loads(output.read_text(encoding="utf-8"))
    assert document["schema"] == "copperscript-global-route/v0.1"
    assert document["status"] == "success"
    assert document["routes"]
    assert "GLOBAL ROUTE: success" in result.stdout


def test_cli_checks_kicad_mod_footprint() -> None:
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "check-footprint",
            "tests/fixtures/footprints/R_0402_Test.kicad_mod",
            "--strict",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0
    assert "R_0402_Test: 2 pads, 6 graphics" in result.stdout
