from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path

from pcbir import check, has_errors


ROOT = Path(__file__).parents[1]


def load_python_fixture(name: str):
    path = ROOT / "tests" / "fixtures" / "python_ir" / name
    spec = spec_from_file_location(f"test_{path.stem}", path)
    assert spec is not None and spec.loader is not None
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.board


def test_valid_board_has_no_diagnostics() -> None:
    diagnostics = check(load_python_fixture("valid_board.py"))
    assert diagnostics == []
    assert not has_errors(diagnostics)


def test_invalid_board_exercises_required_erc_rules() -> None:
    diagnostics = check(load_python_fixture("invalid_board.py"))
    codes = {diagnostic.code for diagnostic in diagnostics}
    assert {
        "DUPLICATE_COMPONENT",
        "UNKNOWN_PART",
        "UNKNOWN_COMPONENT",
        "PIN_ON_MULTIPLE_NETS",
        "OUTPUT_CONFLICT",
        "SUPPLY_VOLTAGE_HIGH",
        "UNSOURCED_POWER_INPUT",
        "I2C_MISSING_PULLUP",
    } <= codes
    assert has_errors(diagnostics)
