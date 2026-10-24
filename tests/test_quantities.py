import pytest
from decimal import Decimal

from pcbir import kiloohms, millivolts, ohms, volts
from pcbir.model import Board, ComponentInstance
from pcbir.quantities import (
    Capacitance, Charge, Current, Energy, Frequency, Impedance, Inductance,
    Length, Power, Resistance, Time, Voltage, decimal_text,
)
from pcbir.serializer import board_to_dict


def test_units_normalize_to_base_units() -> None:
    assert kiloohms(4.7).base_value == ohms(4700).base_value
    assert millivolts(3300).base_value == volts(3.3).base_value


@pytest.mark.parametrize("kind", [Voltage, Resistance, Capacitance, Inductance,
    Length, Current, Frequency, Impedance, Time, Power, Energy, Charge])
def test_equality_and_hashing_use_normalized_values(kind):
    values = [kind.of(Decimal(1) / scale, unit) for unit, scale in kind.UNITS.items()]
    assert all(value == values[0] and value <= values[0] for value in values)
    assert len(set(values)) == 1
    assert values[0] != kind.of(2, next(iter(kind.UNITS)))
    assert len({str(value) for value in values}) == len(values)


def test_equality_keeps_dimensions_distinct():
    assert Voltage.of(1, "V") != Current.of(1, "A")
    assert Resistance.of(1, "ohm") != Impedance.of(1, "ohm")


def test_cross_dimension_comparison_is_rejected() -> None:
    with pytest.raises(TypeError):
        _ = volts(3.3) < ohms(10)


def test_unknown_unit_is_rejected() -> None:
    with pytest.raises(ValueError):
        type(volts(1)).of(1, "kV")


@pytest.mark.parametrize("value, expected", [
    ("1E+2", "100"), ("1E-7", "0.0000001"), ("100.000", "100"),
    ("4.700", "4.7"), ("-0.000", "0"), ("-1E+3", "-1000"),
    ("123456789012345678901234567890.123456789", "123456789012345678901234567890.123456789"),
])
def test_decimal_text_is_exact_and_never_scientific(value, expected):
    assert decimal_text(Decimal(value)) == expected


@pytest.mark.parametrize("kind", [
    Capacitance, Charge, Current, Energy, Frequency, Impedance, Inductance,
    Length, Power, Resistance, Time, Voltage,
])
def test_every_quantity_unit_uses_plain_numbers(kind):
    for unit in kind.UNITS:
        for value, text in (("100", "100"), ("0", "0"), ("-100", "-100"), ("4.7", "4.7")):
            quantity = kind.of(value, unit)
            assert str(quantity) == f"{text} {unit}"
            board = Board("NumericOutput", {}, (ComponentInstance("X1", "X", quantity),), ())
            encoded = board_to_dict(board)["components"][0]["value"]
            assert encoded["value"] == text
            assert encoded["unit"] == unit
            assert "e" not in encoded["base_value"].lower()
            assert Decimal(encoded["base_value"]) == quantity.base_value


def test_capacitor_ir_uses_integer_nanofarads_without_changing_si_value():
    board = Board("Capacitor", {}, (ComponentInstance("C1", "CAPACITOR", Capacitance.of(100, "nF")),), ())
    assert board_to_dict(board)["components"][0]["value"] == {
        "value": "100", "unit": "nF", "base_value": "0.0000001",
    }
