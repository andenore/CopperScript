from pathlib import Path

import pytest

from pcbir import CopperScriptError, compile_file, compile_source
from pcbir.quantities import kiloohms, volts


ROOT = Path(__file__).parents[1]


def test_valid_source_compiles_to_typed_ir() -> None:
    board = compile_file(ROOT / "examples" / "valid_board.copper")
    assert board.name == "ValidSensorBoard"
    assert board.components[4].value == kiloohms(4.7)
    assert board.supplies[1].voltage == volts(3.3)
    assert board.interfaces[0].type_name == "std.i2c"
    assert board.interfaces[0].bindings["U3"]["sda"] == "SDA"


def test_syntax_errors_include_file_line_and_column() -> None:
    source = """board Demo {
    use library "tiny"
}
"""
    with pytest.raises(CopperScriptError) as captured:
        compile_source(source, "broken.copper")
    assert captured.value.code == "PAR009"
    assert str(captured.value).startswith("broken.copper:3:1:")


def test_unknown_units_are_rejected_during_lowering() -> None:
    source = """board Demo {
    use library "tiny";
    component R1: RESISTOR { value = 10furlong; }
}
"""
    with pytest.raises(CopperScriptError) as captured:
        compile_source(source, "units.copper")
    assert captured.value.code == "CMP016"


def test_supply_net_defaults_to_supply_name() -> None:
    source = """board Demo {
    use library "tiny";
    component PS1: VOLTAGE_SOURCE;
    net VBUS { PS1.OUT; }
    supply VBUS { voltage = 5V; source = PS1.OUT; }
}
"""
    board = compile_source(source)
    assert board.supplies[0].net == "VBUS"
