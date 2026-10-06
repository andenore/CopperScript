"""Absolute-maximum voltage ratings, operating-range warnings and pad-relative limits."""

from dataclasses import replace
from pathlib import Path

import pytest

from pcbir import (
    CopperScriptError,
    RelativeVoltage,
    Severity,
    check,
    compile_source,
    has_errors,
)
from pcbir.quantities import volts
from pcbir.serializer import board_to_dict


# A serializer die: the core rail has an operating and an absolute range, the
# auxiliary rail and the lane input are limited relative to the termination
# supply. The auxiliary limit is ``min(VTERM + 0.1 V, 1.36 V)``.
DEVICE = """device SERDES_DIE {
    pad VDD {
        domains = "power"; directions = "input";
        voltage_min = 1.0V; voltage_max = 1.3V;
        absolute_min = -0.3V; absolute_max = 1.5V;
    }
    pad VTERM { domains = "power"; directions = "input"; }
    pad GND { domains = "ground"; directions = "input"; }
    pad AUX {
        domains = "power"; directions = "input";
        voltage_max = 1.3V; absolute_max = "VTERM+0.1V, 1.36V";
    }
    pad LANE_P { domains = "digital"; directions = "input"; absolute_max = "VTERM + 100mV"; }
}
"""

PART = """part SERDES {
    category = "interface.serdes";
    footprint = "QFN-5";
    device = SERDES_DIE;
    pin VDD { number = "1"; bond = VDD; }
    pin VTERM { number = "2"; bond = VTERM; }
    pin GND { number = "3"; bond = GND; }
    pin AUX { number = "4"; bond = AUX; }
    pin LANE_P { number = "5"; bond = LANE_P; }
}
"""

LEVEL_PART = """part LEVEL_INPUT {
    category = "logic.buffer";
    footprint = "SOT-3";
    pin VCC { number = "1"; domains = "power"; directions = "input"; voltage_max = 3.6V; }
    pin GND { number = "2"; domains = "ground"; directions = "input"; }
    pin IN {
        number = "3"; domains = "digital"; directions = "input";
        absolute_min = "GND-0.5V"; absolute_max = "VCC+0.5V";
    }
}
"""


def compile_limits(
    tmp_path: Path,
    body: str,
    device: str = DEVICE,
    part: str = PART,
    level_part: str = LEVEL_PART,
):
    root = tmp_path / f"fixture-{len(tuple(tmp_path.iterdir()))}"
    root.mkdir()
    (root / "copper.mod").write_text(
        "module limit-test\n"
        "require github.com/test/limits v1.0.0\n"
        "replace github.com/test/limits => .\n",
        encoding="utf-8",
    )
    (root / "serdes_device.copper").write_text(device, encoding="utf-8")
    (root / "serdes.copper").write_text(part, encoding="utf-8")
    (root / "level_input.copper").write_text(level_part, encoding="utf-8")
    return compile_source(
        f"""board Limits {{
            import lim "github.com/test/limits";
            {body}
            supply GND {{ voltage = 0V; external = true; }}
        }}""",
        str(root / "board.copper"),
    )


def serdes_board(tmp_path: Path, vdd: str = "1.2V", vterm: str = "1.2V", aux: str = "1.2V"):
    return compile_limits(
        tmp_path,
        f"""component U1: lim.SERDES;
            net VDD {{ U1.VDD; }}
            net VTERM {{ U1.VTERM; }}
            net AUX {{ U1.AUX; }}
            net GND {{ U1.GND; }}
            supply VDD {{ voltage = {vdd}; external = true; }}
            supply VTERM {{ voltage = {vterm}; external = true; }}
            supply AUX {{ voltage = {aux}; external = true; }}""",
    )


def _by_code(diagnostics, code: str):
    return [item for item in diagnostics if item.code == code]


def test_supply_inside_the_operating_range_is_clean(tmp_path) -> None:
    assert check(serdes_board(tmp_path)) == []


def test_supply_outside_the_absolute_range_is_an_error(tmp_path) -> None:
    diagnostics = check(serdes_board(tmp_path, vdd="1.6V"))

    [error] = _by_code(diagnostics, "SUPPLY_VOLTAGE_ABSOLUTE_HIGH")
    assert error.severity is Severity.ERROR
    assert error.subject == "U1.VDD"
    assert "absolute maximum 1.5 V" in error.message
    # The absolute violation replaces, rather than repeats, the operating one.
    assert not _by_code(diagnostics, "SUPPLY_VOLTAGE_HIGH")

    low = check(serdes_board(tmp_path, vdd="-0.5V"))
    assert _by_code(low, "SUPPLY_VOLTAGE_ABSOLUTE_LOW")[0].severity is Severity.ERROR


def test_supply_outside_operating_but_inside_absolute_is_a_warning(tmp_path) -> None:
    diagnostics = check(serdes_board(tmp_path, vdd="1.4V"))

    [warning] = _by_code(diagnostics, "SUPPLY_VOLTAGE_HIGH")
    assert warning.severity is Severity.WARNING
    assert "operating maximum 1.3 V" in warning.message
    assert not has_errors(diagnostics)


def test_operating_only_limits_remain_errors(tmp_path) -> None:
    board = compile_limits(
        tmp_path,
        """component U2: lim.LEVEL_INPUT;
            net VCC { U2.VCC; }
            net GND { U2.GND; }
            supply VCC { voltage = 5V; external = true; }""",
    )

    [error] = _by_code(check(board), "SUPPLY_VOLTAGE_HIGH")
    assert error.severity is Severity.ERROR
    assert error.message == "5 V exceeds the pin maximum 3.6 V"


def test_relative_limit_resolves_against_the_reference_supply(tmp_path) -> None:
    # AUX absolute maximum is VTERM + 0.1 V = 1.3 V; 1.25 V is inside it.
    assert check(serdes_board(tmp_path, vterm="1.2V", aux="1.25V")) == []

    # With VTERM = 1.3 V the absolute maximum is min(1.4 V, 1.36 V) = 1.36 V,
    # so 1.35 V only exceeds the 1.3 V operating maximum: a warning.
    raised = check(serdes_board(tmp_path, vterm="1.3V", aux="1.35V"))
    [warning] = _by_code(raised, "SUPPLY_VOLTAGE_HIGH")
    assert warning.severity is Severity.WARNING
    assert "absolute maximum 1.36 V (VTERM+0.1 V, 1.36 V; VTERM = 1.3 V)" in warning.message
    assert not has_errors(raised)


def test_relative_limit_violation_is_an_error(tmp_path) -> None:
    diagnostics = check(serdes_board(tmp_path, vterm="1.2V", aux="1.35V"))

    [error] = _by_code(diagnostics, "SUPPLY_VOLTAGE_ABSOLUTE_HIGH")
    assert error.severity is Severity.ERROR
    assert error.subject == "U1.AUX"
    assert error.message == (
        "1.35 V exceeds the pin absolute maximum 1.3 V (VTERM+0.1 V, 1.36 V; VTERM = 1.2 V)"
    )
    assert not _by_code(diagnostics, "SUPPLY_VOLTAGE_HIGH")


def test_fixed_cap_bounds_a_relative_limit(tmp_path) -> None:
    # VTERM + 0.1 V = 1.5 V, but the fixed 1.36 V cap is tighter.
    diagnostics = check(serdes_board(tmp_path, vterm="1.4V", aux="1.38V"))

    [error] = _by_code(diagnostics, "SUPPLY_VOLTAGE_ABSOLUTE_HIGH")
    assert "absolute maximum 1.36 V" in error.message


def test_standalone_pin_limits_relative_to_other_pins(tmp_path) -> None:
    board = compile_limits(
        tmp_path,
        """component U2: lim.LEVEL_INPUT;
            net V1V8 { U2.VCC; }
            net V3V3 { U2.IN; }
            net GND { U2.GND; }
            supply V1V8 { voltage = 1.8V; external = true; }
            supply V3V3 { voltage = 3.3V; external = true; }""",
    )

    [error] = _by_code(check(board), "SUPPLY_VOLTAGE_ABSOLUTE_HIGH")
    assert error.subject == "U2.IN"
    assert "2.3 V (VCC+0.5 V; VCC = 1.8 V)" in error.message


def test_unresolvable_reference_is_an_error(tmp_path) -> None:
    # VTERM is on a net without a declared supply, so VTERM+0.1V has no value.
    board = compile_limits(
        tmp_path,
        """component U1: lim.SERDES;
            net VDD { U1.VDD; }
            net VTERM_FLOAT { U1.VTERM; }
            net AUX { U1.AUX; }
            net GND { U1.GND; }
            supply VDD { voltage = 1.2V; external = true; }
            supply AUX { voltage = 1.35V; external = true; }""",
    )

    diagnostics = check(board)
    [unresolved] = _by_code(diagnostics, "VOLTAGE_LIMIT_UNRESOLVED")
    assert unresolved.severity is Severity.ERROR
    assert unresolved.subject == "U1.AUX"
    assert "VTERM is not connected to a declared supply" in unresolved.message
    # Without a resolved absolute bound the operating limit stays a hard limit.
    assert _by_code(diagnostics, "SUPPLY_VOLTAGE_HIGH")[0].severity is Severity.ERROR
    # The fixed cap still applies on its own.
    assert not _by_code(diagnostics, "SUPPLY_VOLTAGE_ABSOLUTE_HIGH")


def test_unresolved_reference_still_applies_the_fixed_cap(tmp_path) -> None:
    board = compile_limits(
        tmp_path,
        """component U1: lim.SERDES;
            net VDD { U1.VDD; }
            net AUX { U1.AUX; }
            net GND { U1.GND; }
            supply VDD { voltage = 1.2V; external = true; }
            supply AUX { voltage = 1.4V; external = true; }""",
    )

    codes = {item.code for item in check(board)}
    assert {"VOLTAGE_LIMIT_UNRESOLVED", "SUPPLY_VOLTAGE_ABSOLUTE_HIGH"} <= codes


def test_reference_to_an_unknown_pad_is_unresolved(tmp_path) -> None:
    board = serdes_board(tmp_path)
    device = board.devices["lim.SERDES_DIE"]
    aux = device.pads["AUX"]
    absolute = replace(
        aux.profile.absolute_voltage, maximum=RelativeVoltage("VREF", volts("0.1"))
    )
    patched = replace(aux, profile=replace(aux.profile, absolute_voltage=absolute))
    board = replace(
        board,
        devices={device.name: replace(device, pads={**device.pads, "AUX": patched})},
    )

    [unresolved] = _by_code(check(board), "VOLTAGE_LIMIT_UNRESOLVED")
    assert "U1 has no pin or active device pad 'VREF'" in unresolved.message


def test_absolute_limits_are_serialized_only_when_declared(tmp_path) -> None:
    document = board_to_dict(serdes_board(tmp_path))
    device = next(item for item in document["devices"] if item["name"] == "lim.SERDES_DIE")
    pads = {pad["name"]: pad["profile"] for pad in device["pads"]}

    assert pads["VDD"]["absolute_voltage"]["rating"] == "absolute"
    assert pads["VDD"]["absolute_voltage"]["maximum"]["base_value"] == "1.5"
    assert pads["VDD"]["voltage"]["rating"] == "operating"
    assert pads["AUX"]["absolute_voltage"]["maximum"] == {
        "reference": "VTERM",
        "offset": {"value": "0.1", "unit": "V", "base_value": "0.1"},
        "limit": {"value": "1.36", "unit": "V", "base_value": "1.36"},
    }
    assert pads["LANE_P"]["absolute_voltage"]["maximum"]["limit"] is None
    assert pads["LANE_P"]["absolute_voltage"]["maximum"]["offset"]["unit"] == "mV"
    assert "absolute_voltage" not in pads["GND"]


@pytest.mark.parametrize(
    ("attribute", "code"),
    [
        ('absolute_max = "1.35";', "CMP120"),
        ('absolute_max = "VTERM+0.1mA";', "CMP120"),
        ('absolute_max = "1.3V, 1.4V";', "CMP120"),
        ('absolute_max = "VTERM+0.1V, VDD";', "CMP120"),
        ('absolute_max = "VTERM, 1.3V, 1.4V";', "CMP120"),
        ("absolute_max = 10mA;", "CMP120"),
        ("voltage_max = 1.3V; absolute_max = 1.2V;", "CMP121"),
        ("absolute_min = 1.0V; absolute_max = 0.5V;", "CMP121"),
        ('voltage_max = 1.3V; absolute_max = "VTERM+0.1V, 1.2V";', "CMP121"),
        ('absolute_max = "VREF+0.1V";', "CMP122"),
    ],
)
def test_malformed_absolute_limits_are_rejected(tmp_path, attribute: str, code: str) -> None:
    device = DEVICE.replace(
        'pad GND { domains = "ground"; directions = "input"; }',
        f'pad GND {{ domains = "ground"; directions = "input"; {attribute} }}',
    )
    with pytest.raises(CopperScriptError, match=code):
        compile_limits(tmp_path, "component U1: lim.SERDES;", device=device)


def test_quoted_fixed_and_spaced_relative_limits_compile(tmp_path) -> None:
    device = DEVICE.replace(
        'pad GND { domains = "ground"; directions = "input"; }',
        'pad GND { domains = "ground"; directions = "input"; '
        'absolute_min = "-0.3V"; absolute_max = "VDD - 300mV"; }',
    )
    board = compile_limits(tmp_path, "component U1: lim.SERDES;", device=device)
    absolute = board.devices["lim.SERDES_DIE"].pads["GND"].profile.absolute_voltage

    assert absolute.minimum == volts("-0.3")
    assert absolute.maximum.reference == "VDD"
    assert absolute.maximum.offset.base_value == volts("-0.3").base_value
    assert str(absolute.maximum) == "VDD-300 mV"


def test_part_pin_reference_must_name_a_pin_or_pad(tmp_path) -> None:
    part = LEVEL_PART.replace('absolute_max = "VCC+0.5V";', 'absolute_max = "VDDIO+0.5V";')
    with pytest.raises(CopperScriptError, match="CMP122"):
        compile_limits(tmp_path, "component U2: lim.LEVEL_INPUT;", level_part=part)
