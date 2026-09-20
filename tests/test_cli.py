import subprocess
import sys
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
