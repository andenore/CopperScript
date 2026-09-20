import pytest

from pcbir import kiloohms, millivolts, ohms, volts


def test_units_normalize_to_base_units() -> None:
    assert kiloohms(4.7).base_value == ohms(4700).base_value
    assert millivolts(3300).base_value == volts(3.3).base_value


def test_cross_dimension_comparison_is_rejected() -> None:
    with pytest.raises(TypeError):
        _ = volts(3.3) < ohms(10)


def test_unknown_unit_is_rejected() -> None:
    with pytest.raises(ValueError):
        type(volts(1)).of(1, "kV")
