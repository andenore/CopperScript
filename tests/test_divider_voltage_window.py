import pytest
from pcbir.power_integrity import divider_voltage_window, CALCULATIONS


def inputs():
    return dict(reference_min_v=.495, reference_max_v=.505,
                upper_resistors_ohms=[330000, 330000], upper_tolerances=[.01, .01],
                lower_ohms=100000, lower_tolerance=.01,
                negative_regulation_fraction=.01, positive_regulation_fraction=.06,
                operating_min_v=3.6, operating_max_v=4.2)


def test_exact_independent_corners_and_margin_are_not_droop_estimates():
    value = divider_voltage_window(**inputs())
    assert value["static_minimum_v"] == pytest.approx(.495 * (1 + 653400 / 101000) * .99)
    assert value["static_maximum_v"] == pytest.approx(.505 * (1 + 666600 / 99000) * 1.06)
    assert 0 < value["remaining_negative_margin_v"] < .1
    assert 0 < value["remaining_positive_margin_v"] < .1
    assert "passed" not in value
    assert CALCULATIONS["divider_voltage_window"] is divider_voltage_window


def test_negative_margin_is_preserved_not_clamped_or_hidden():
    value = divider_voltage_window(**{**inputs(), "operating_min_v": 3.8})
    assert value["remaining_negative_margin_v"] < 0


def test_individual_resistor_tolerances_are_not_averaged():
    value = divider_voltage_window(**{**inputs(), "upper_tolerances": [.01, .02]})
    assert value["static_minimum_v"] < divider_voltage_window(**inputs())["static_minimum_v"]
    assert value["static_maximum_v"] > divider_voltage_window(**inputs())["static_maximum_v"]


@pytest.mark.parametrize("changes", [
    {"reference_min_v": True}, {"reference_max_v": float("nan")},
    {"reference_min_v": .51}, {"operating_min_v": 5},
    {"upper_resistors_ohms": []}, {"upper_resistors_ohms": [0, 330000]},
    {"upper_tolerances": [.01]}, {"upper_tolerances": [.01, 1]},
    {"lower_tolerance": -1}, {"lower_tolerance": 1},
    {"negative_regulation_fraction": True}, {"positive_regulation_fraction": float("inf")},
    {"negative_regulation_fraction": 1}, {"lower_ohms": None},
])
def test_invalid_or_missing_explicit_inputs_rejected(changes):
    with pytest.raises(ValueError):
        divider_voltage_window(**{**inputs(), **changes})
