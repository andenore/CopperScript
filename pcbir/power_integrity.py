"""Explicit-input DC/transient screening, not ampacity or thermal certification.

SI units throughout. No guessed copper/plating, efficiency, effective capacitance
or temperature. Conductor sums require an explicitly selected series path; they
must not be applied indiscriminately to a branched net or a filled power plane.
"""
from __future__ import annotations

from math import isfinite, pi
from decimal import Decimal


def number(value, name: str, *, minimum=0.0, allow_zero=False) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{name} must be a finite number")
    value = float(value)
    if not isfinite(value) or (value < minimum if allow_zero else value <= minimum):
        raise ValueError(f"invalid {name}")
    return value


def trace_resistance(*, length_m, width_m, thickness_m, resistivity_ohm_m,
                     temperature_c, reference_temperature_c, temperature_coefficient_per_c) -> float:
    length = number(length_m, "length_m")
    area = number(width_m, "width_m") * number(thickness_m, "thickness_m")
    rho = temperature_resistivity(resistivity_ohm_m, temperature_c,
                                  reference_temperature_c, temperature_coefficient_per_c)
    return rho * length / area


def temperature_resistivity(resistivity, temperature, reference, coefficient):
    rho = number(resistivity, "resistivity_ohm_m")
    for value in (temperature, reference):
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not isfinite(value) or value < -273.15:
            raise ValueError("invalid conductor temperature")
    coefficient = number(coefficient, "temperature_coefficient_per_c", allow_zero=True)
    factor = 1 + coefficient * (temperature - reference)
    if factor <= 0:
        raise ValueError("temperature model outside positive-resistivity range")
    return rho * factor


def via_resistance(*, length_m, finished_hole_diameter_m, plating_thickness_m,
                   resistivity_ohm_m, temperature_c, reference_temperature_c,
                   temperature_coefficient_per_c) -> float:
    length = number(length_m, "length_m")
    diameter = number(finished_hole_diameter_m, "finished_hole_diameter_m")
    plating = number(plating_thickness_m, "plating_thickness_m")
    # Exact annular barrel cross-section; diameter is the FINISHED inside hole.
    area = pi * (diameter * plating + plating * plating)
    rho = temperature_resistivity(resistivity_ohm_m, temperature_c,
                                  reference_temperature_c, temperature_coefficient_per_c)
    return rho * length / area


def series_path(*, resistance_ohms, peak_current_a, rms_current_a) -> dict:
    if not isinstance(resistance_ohms, list) or not resistance_ohms:
        raise ValueError("explicit series path must contain at least one resistance")
    total = sum(number(value, "resistance_ohms", allow_zero=True) for value in resistance_ohms)
    peak = number(peak_current_a, "peak_current_a", allow_zero=True)
    rms = number(rms_current_a, "rms_current_a", allow_zero=True)
    if rms > peak:
        raise ValueError("RMS current cannot exceed peak current")
    return {"resistance_ohms": total, "peak_drop_v": peak * total, "rms_loss_w": rms * rms * total}


def reservoir_droop(*, load_step_a, duration_s, effective_capacitance_f, esr_ohms) -> dict:
    step = number(load_step_a, "load_step_a", allow_zero=True)
    duration = number(duration_s, "duration_s")
    capacitance = number(effective_capacitance_f, "effective_capacitance_f")
    esr = number(esr_ohms, "esr_ohms", allow_zero=True)
    # Deliberately assumes NO replenishment during the stated pulse, ignores ESL.
    return {"capacitive_drop_v": step * duration / capacitance,
            "esr_step_v": step * esr, "total_drop_v": step * duration / capacitance + step * esr}


def converter_demand(*, minimum_input_v, output_v, output_current_a, minimum_efficiency,
                     quiescent_current_a) -> dict:
    vin = number(minimum_input_v, "minimum_input_v")
    vout = number(output_v, "output_v")
    current = number(output_current_a, "output_current_a", allow_zero=True)
    efficiency = number(minimum_efficiency, "minimum_efficiency")
    if efficiency > 1:
        raise ValueError("efficiency must be in (0,1]")
    idle = number(quiescent_current_a, "quiescent_current_a", allow_zero=True)
    power = vout * current
    return {"input_current_a": power / (vin * efficiency) + idle,
            "estimated_loss_w": power * (1 / efficiency - 1) + idle * vin}


def thermal_estimate(*, power_w, thermal_resistance_k_per_w, ambient_c, maximum_c, model_source):
    """Reuse the established reduced-order solver; never substitute generic theta.

    The stated resistance must apply to this board, mounting and environment.
    This is not a package junction-to-case value applied to arbitrary ambient.
    """
    from .engineering import thermal_screen
    power = number(power_w, "power_w", allow_zero=True)
    theta = number(thermal_resistance_k_per_w, "thermal_resistance_k_per_w")
    for value in (ambient_c, maximum_c):
        number(value, "temperature_c", minimum=-273.15, allow_zero=True)
    if not isinstance(model_source, str) or not model_source.strip():
        raise ValueError("sourced board/environment thermal model required")
    result = thermal_screen(Decimal(str(power)), Decimal(str(theta)), Decimal(str(ambient_c)),
                            Decimal(str(maximum_c)), model_source=model_source)
    return {"estimated_temperature_c": float(result.value)}


def divider_voltage_window(*, reference_min_v, reference_max_v, upper_resistors_ohms,
                           upper_tolerances, lower_ohms, lower_tolerance,
                           negative_regulation_fraction, positive_regulation_fraction,
                           operating_min_v, operating_max_v):
    """Worst-case independent resistor/reference corners and explicit error budget.

    Assumes Vout = Vref * (1 + Rupper/Rlower). Regulation fractions are supplied
    conservative combined bounds, not inferred from device mode. Does not model
    feedback bias/leakage, ripple, transients, wiring loss, startup or thermal drift
    beyond the supplied bounds. Remaining voltage margins are NOT measured droop
    or permission to pass mandatory simulation/bench qualification.
    """
    def fraction(value, name):
        value = number(value, name, allow_zero=True)
        if value >= 1:
            raise ValueError(f"{name} must be in [0,1)")
        return value

    lo = number(reference_min_v, "reference_min_v")
    hi = number(reference_max_v, "reference_max_v")
    minimum = number(operating_min_v, "operating_min_v")
    maximum = number(operating_max_v, "operating_max_v")
    if lo > hi or minimum > maximum:
        raise ValueError("inverted voltage range")
    if (not isinstance(upper_resistors_ohms, list) or not upper_resistors_ohms
            or not isinstance(upper_tolerances, list)
            or len(upper_resistors_ohms) != len(upper_tolerances)):
        raise ValueError("explicit upper resistor/tolerance lists must match")
    resistors = [number(v, "upper resistor") for v in upper_resistors_ohms]
    tolerances = [fraction(v, "upper tolerance") for v in upper_tolerances]
    bottom = number(lower_ohms, "lower_ohms")
    bt = fraction(lower_tolerance, "lower_tolerance")
    neg = fraction(negative_regulation_fraction, "negative_regulation_fraction")
    pos = fraction(positive_regulation_fraction, "positive_regulation_fraction")
    upper_low = sum(r * (1 - t) for r, t in zip(resistors, tolerances))
    upper_high = sum(r * (1 + t) for r, t in zip(resistors, tolerances))
    low_output = lo * (1 + upper_low / (bottom * (1 + bt))) * (1 - neg)
    high_output = hi * (1 + upper_high / (bottom * (1 - bt))) * (1 + pos)
    return {"static_minimum_v": low_output, "static_maximum_v": high_output,
            "remaining_negative_margin_v": low_output - minimum,
            "remaining_positive_margin_v": maximum - high_output}


CALCULATIONS = {"trace_resistance": trace_resistance, "via_resistance": via_resistance,
                "series_path": series_path, "reservoir_droop": reservoir_droop,
                "converter_demand": converter_demand, "thermal_estimate": thermal_estimate,
                "divider_voltage_window": divider_voltage_window}
