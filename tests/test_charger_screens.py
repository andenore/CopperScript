"""Generic mathematical screens do not manufacture component/bench guarantees."""
import pytest
from pcbir.power_integrity import ratiometric_ntc_divider, resistor_programming_window
from pcbir.engineering_qualification import evaluate_requirement


PARAMETERS = dict(upper_ohms=43000, upper_tolerance=0.01,
                  shunt_ohms=39000, shunt_tolerance=0.01,
                  isolation_ohms=100000, isolation_tolerance=0.01,
                  ntc_min_ohms=10000, ntc_max_ohms=10000,
                  input_min_v=4.35, input_max_v=6.4, maximum_input_leakage_a=0.0000001)


def test_exact_unloaded_ratio_and_signed_leakage():
    result = ratiometric_ntc_divider(**{**PARAMETERS, "upper_tolerance": 0,
        "shunt_tolerance": 0, "isolation_tolerance": 0, "maximum_input_leakage_a": 0})
    parallel = 10000 * 39000 / (10000 + 39000)
    assert result["minimum_ratio"] == pytest.approx(parallel / (43000 + parallel))
    leaked = ratiometric_ntc_divider(**PARAMETERS)
    assert leaked["minimum_ratio"] < result["minimum_ratio"] < leaked["maximum_ratio"]


def test_monotonic_resistance_and_faults():
    cold = ratiometric_ntc_divider(**{**PARAMETERS, "ntc_min_ohms": 27280 * 0.95,
                                    "ntc_max_ohms": 27280 * 1.05})
    hot = ratiometric_ntc_divider(**{**PARAMETERS, "ntc_min_ohms": 5827 * 0.95,
                                   "ntc_max_ohms": 5827 * 1.05})
    assert cold["minimum_ratio"] > 0.255
    assert hot["maximum_ratio"] < 0.12
    assert hot["open_minimum_ratio"] > 0.255
    assert cold["short_maximum_ratio"] < 0.12


@pytest.mark.parametrize("key,value", [("upper_ohms", 0), ("shunt_tolerance", 1),
    ("ntc_min_ohms", -1), ("input_min_v", True), ("maximum_input_leakage_a", float("nan")),
    ("ntc_max_ohms", 9999), ("input_max_v", 4)])
def test_invalid_inputs_rejected(key, value):
    with pytest.raises(ValueError):
        ratiometric_ntc_divider(**{**PARAMETERS, key: value})


def test_unknown_leakage_is_incomplete_not_zero():
    result = evaluate_requirement("ntc", {"algorithm": "calculation", "stage": "design"},
        {"basis": "synthetic fixture", "calculation": "ratiometric_ntc_divider",
         "parameters": {**PARAMETERS, "maximum_input_leakage_a": None},
         "limits": {"maximum_ratio": {"maximum": 0.12}}}, None, {}, None)
    assert result["status"] == "incomplete"


def test_programmed_current_and_timer_corners():
    current = resistor_programming_window(factor_min=797, factor_max=975,
                                         resistor_ohms=2200, tolerance=0.01, inverse=True)
    assert current["programmed_minimum"] == pytest.approx(797 / 2222)
    assert current["programmed_maximum"] == pytest.approx(975 / 2178)
    timer = resistor_programming_window(factor_min=0.35, factor_max=0.55,
                                       resistor_ohms=68000, tolerance=0.01, inverse=False)
    assert timer == {"programmed_minimum": pytest.approx(23562),
                     "programmed_maximum": pytest.approx(37774)}


@pytest.mark.parametrize("change", [{"factor_max": 1}, {"tolerance": 1}, {"inverse": 1},
                                    {"resistor_ohms": 0}, {"factor_min": float("inf")}])
def test_programming_invalid_inputs_rejected(change):
    with pytest.raises(ValueError):
        resistor_programming_window(**{**dict(factor_min=797, factor_max=975,
            resistor_ohms=2200, tolerance=0.01, inverse=True), **change})
