"""Dimension-aware measurements on full-resolution solver data."""
from __future__ import annotations

import bisect
import math

from ..quantities import Charge, Current, Energy, Frequency, Power, Time, Voltage
from .model import SimulationError, identifier, number, object_fields, quantity, required
from .result import Waveforms, represented


UNITS = {"V": Voltage, "A": Current, "W": Power, "J": Energy, "C": Charge, "s": Time, "Hz": Frequency}


def bound(value, unit: str, context: str) -> float:
    if unit in UNITS:
        return float(quantity(value, UNITS[unit], context).base_value)
    if unit in {"dB", "deg"}:
        if not isinstance(value, str) or not value.endswith(" " + unit):
            raise SimulationError(f"{context}: expected {unit}")
        try:
            return number(float(value[:-(len(unit) + 1)]), context)
        except ValueError as exc:
            raise SimulationError(f"{context}: invalid {unit} value") from exc
    return number(value, context)


def validate_checks(checks, signal_names, analyses):
    if not isinstance(checks, list):
        raise SimulationError("checks must be a list")
    names = set()
    analysis_names = {a.name for a in analyses}
    for raw in checks:
        check = object_fields(raw, {"name", "analysis", "probe", "operation", "window", "at", "representation", "unwrap_phase",
                                   "minimum", "maximum", "level", "direction", "crossing"}, "check")
        name = identifier(required(check, "name", "check"), "check name")
        if name in names:
            raise SimulationError("duplicate check name")
        names.add(name)
        if check.get("probe") not in signal_names:
            raise SimulationError(f"check {name}: unknown probe/derived signal")
        if check.get("analysis") not in analysis_names and ("analysis" in check or len(analyses) > 1):
            raise SimulationError(f"check {name}: specify an existing analysis")
        operation = required(check, "operation", name)
        if operation not in {"min", "max", "at", "integral", "mean", "rms", "peak_to_peak", "crossing"}:
            raise SimulationError(f"check {name}: unknown measurement operation")
        if not ({"minimum", "maximum"} & check.keys()):
            raise SimulationError(f"check {name}: provide minimum and/or maximum")
        if check.get("representation", "real") not in {"real", "magnitude", "dB", "phase"}:
            raise SimulationError(f"check {name}: invalid representation")
        if "unwrap_phase" in check and type(check["unwrap_phase"]) is not bool:
            raise SimulationError("unwrap_phase must be boolean")
        if operation == "at" and "at" not in check:
            raise SimulationError(f"check {name}: at requires an axis coordinate")
        if operation != "at" and "at" in check:
            raise SimulationError(f"check {name}: at only applies to the at operation")
        if operation == "crossing":
            required(check, "level", name)
            if check.get("direction", "either") not in {"either", "rising", "falling"} or check.get("crossing", "unique") not in {"unique", "first", "last"}:
                raise SimulationError(f"check {name}: invalid crossing direction/selection")
        elif {"level", "direction", "crossing"} & check.keys():
            raise SimulationError("crossing fields require the crossing operation")
        if "window" in check and (not isinstance(check["window"], list) or len(check["window"]) != 2):
            raise SimulationError("check window requires two axis coordinates")


def signal_units(plan, analysis=None):
    units = {p.name: "V" if p.kind == "voltage" else "A" for p in plan.probes}
    for d in plan.derived_signals:
        if analysis is not None and d.analysis is not None and d.analysis != analysis:
            continue
        if d.left not in units or d.right not in units:
            raise SimulationError(f"{d.name}: operand is unavailable in analysis {analysis}")
        left, right = units[d.left], units[d.right]
        if d.kind in {"ratio", "difference"}:
            if left != right:
                raise SimulationError(f"{d.name}: operands must have equal dimensions")
            units[d.name] = "1" if d.kind == "ratio" else left
        else:
            products = {("V", "A"): "W", ("A", "V"): "W", ("1", "V"): "V", ("V", "1"): "V",
                        ("1", "A"): "A", ("A", "1"): "A", ("1", "1"): "1"}
            if (left, right) not in products:
                raise SimulationError(f"{d.name}: unsupported product dimensions")
            units[d.name] = products[(left, right)]
    return units


def check_units(check, analysis, signal_unit):
    representation = check.get("representation", "real")
    if representation == "dB" and signal_unit != "1":
        raise SimulationError("dB checks require a dimensionless transfer ratio")
    if analysis.kind != "ac" and representation != "real":
        raise SimulationError("magnitude/dB/phase checks require AC analysis")
    unit = "dB" if representation == "dB" else "deg" if representation == "phase" else signal_unit
    axis_unit = "Hz" if analysis.kind == "ac" else "s" if analysis.kind == "transient" else "1"
    operation = check["operation"]
    if operation == "integral":
        if analysis.kind != "transient" or unit not in {"A", "W"}:
            raise SimulationError("integral supports transient current (charge) or power (energy)")
        unit = {"A": "C", "W": "J"}[unit]
    if operation == "crossing":
        bound(check["level"], unit, "crossing level")
        unit = axis_unit
    if "window" in check:
        window = [bound(v, axis_unit, "check window") for v in check["window"]]
        if window[0] >= window[1] or analysis.kind == "op":
            raise SimulationError("measurement window must increase and requires AC/transient analysis")
    if operation == "at":
        bound(check["at"], axis_unit, "measurement coordinate")
    bounds = {k: bound(check[k], unit, k) for k in ("minimum", "maximum") if k in check}
    if bounds.get("minimum", -math.inf) > bounds.get("maximum", math.inf):
        raise SimulationError("measurement minimum exceeds maximum")
    return unit, axis_unit, bounds


def validate_dimensions(plan):
    for analysis in plan.analyses:
        units = signal_units(plan, analysis.name)
        for check in plan.data.get("checks", []):
            if check.get("analysis", analysis.name) == analysis.name:
                if check["probe"] not in units:
                    raise SimulationError(f"check {check['name']}: signal unavailable in analysis {analysis.name}")
                check_units(check, analysis, units[check["probe"]])
    from .report import validate_plot_dimensions
    validate_plot_dimensions(plan)


def _interpolate(x, y, at, *, logarithmic=False):
    if at < x[0] or at > x[-1]:
        raise SimulationError("measurement coordinate lies outside available samples")
    index = bisect.bisect_left(x, at)
    if index < len(x) and math.isclose(x[index], at, rel_tol=1e-12, abs_tol=1e-18):
        return y[index]
    if index == 0 or index == len(x) or y[index - 1] is None or y[index] is None:
        return None
    a, b, p = (math.log(x[index - 1]), math.log(x[index]), math.log(at)) if logarithmic else (x[index - 1], x[index], at)
    return y[index - 1] + (y[index] - y[index - 1]) * ((p - a) / (b - a))


def evaluate(check: dict, waveforms: Waveforms) -> dict:
    analysis = waveforms.analysis
    trace = waveforms.traces[check["probe"]]
    representation = check.get("representation", "real")
    unit, axis_unit, limits = check_units(check, analysis, trace.unit)
    result = {"name": check["name"], "case": waveforms.case, "analysis": analysis.name,
              "probe": check["probe"], "operation": check["operation"], "representation": representation,
              "unit": unit, **limits, "status": "incomplete", "value": None}
    try:
        x = waveforms.axis
        y = represented(trace, representation, unwrap=check.get("unwrap_phase", False))
        logarithmic = analysis.kind == "ac" and analysis.sweep != "linear"
        if "window" in check:
            lower, upper = [bound(v, axis_unit, "window") for v in check["window"]]
            if lower < x[0] or upper > x[-1]:
                raise SimulationError("measurement window is outside the simulated range")
            result["window"] = [lower, upper]
            coordinates = tuple(v for v in x if lower < v < upper)
            inner_values = tuple(v for t, v in zip(x, y) if lower < t < upper)
            y = (_interpolate(x, y, lower, logarithmic=logarithmic), *inner_values,
                 _interpolate(x, y, upper, logarithmic=logarithmic))
            x = (lower, *coordinates, upper)
        if any(v is None for v in y):
            raise SimulationError("measurement includes invalid or undefined samples")
        operation = check["operation"]
        if operation in {"min", "max"}:
            index = (min if operation == "min" else max)(range(len(y)), key=y.__getitem__)
            value = y[index]; result["x"] = x[index]
        elif operation == "at":
            at = bound(check["at"], axis_unit, "at")
            value = _interpolate(x, y, at, logarithmic=logarithmic); result["x"] = at
        elif operation == "peak_to_peak":
            value = max(y) - min(y)
        elif operation == "crossing":
            level_unit = "dB" if representation == "dB" else "deg" if representation == "phase" else trace.unit
            level = bound(check["level"], level_unit, "level")
            crossings = []
            direction = check.get("direction", "either")
            for i in range(len(x) - 1):
                a, b = y[i] - level, y[i + 1] - level
                if a == b:
                    continue
                if a * b <= 0 and (direction == "either" or (direction == "rising" and b > a) or (direction == "falling" and b < a)):
                    left, right = (math.log(x[i]), math.log(x[i + 1])) if logarithmic else (x[i], x[i + 1])
                    crossing = left + (right - left) * (-a / (b - a))
                    crossing = math.exp(crossing) if logarithmic else crossing
                    if not crossings or not math.isclose(crossing, crossings[-1], rel_tol=1e-10, abs_tol=1e-18):
                        crossings.append(crossing)
            choice = check.get("crossing", "unique")
            if not crossings or (choice == "unique" and len(crossings) != 1):
                raise SimulationError("crossing absent or ambiguous in requested window")
            value = crossings[-1] if choice == "last" else crossings[0]
            result["x"] = value; result["marker_y"] = level
        else:
            if analysis.kind == "transient":
                values = tuple(v * v for v in y) if operation == "rms" else y
                integral = sum((b - a) * (va + vb) / 2 for a, b, va, vb in zip(x, x[1:], values, values[1:]))
                value = integral if operation == "integral" else integral / (x[-1] - x[0])
            else:
                value = sum(v * v if operation == "rms" else v for v in y) / len(y)
            if operation == "rms":
                value = math.sqrt(value)
        if value is None or not math.isfinite(value):
            raise SimulationError("measurement is not finite")
        result["value"] = value
        result["status"] = "passed" if limits.get("minimum", -math.inf) <= value <= limits.get("maximum", math.inf) else "failed"
    except SimulationError as exc:
        result["reason"] = str(exc)
    return result
