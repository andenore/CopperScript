"""Normalized waveform data shared by measurements, exports, and visual reports."""
from __future__ import annotations

import cmath
import math
from dataclasses import dataclass
from types import MappingProxyType
from typing import Mapping

from ..backends.ngspice import source_names
from .model import Analysis, SimulationError
from .raw import RawPlot
from .select import SimulationCircuit


@dataclass(frozen=True, slots=True)
class Trace:
    name: str
    unit: str
    values: tuple[complex | None, ...]


@dataclass(frozen=True, slots=True)
class Waveforms:
    analysis: Analysis
    case: str
    axis_name: str
    axis_unit: str
    axis: tuple[float, ...]
    traces: Mapping[str, Trace]
    diagnostics: tuple[str, ...] = ()

    def __post_init__(self):
        object.__setattr__(self, "traces", MappingProxyType(dict(self.traces)))

    def to_dict(self):
        return {"analysis": self.analysis.name, "kind": self.analysis.kind, "case": self.case,
                "axis": {"name": self.axis_name, "unit": self.axis_unit, "values": list(self.axis)},
                "traces": {n: {"unit": t.unit, "real": [v.real if v is not None else None for v in t.values],
                    "imaginary": [v.imag if v is not None else None for v in t.values],
                    "valid": [v is not None for v in t.values]} for n, t in self.traces.items()},
                "diagnostics": list(self.diagnostics)}


def normalize(circuit: SimulationCircuit, analysis: Analysis, case: str, raw: RawPlot) -> Waveforms:
    expected = {"ac": "AC Analysis", "transient": "Transient Analysis", "op": "Operating Point"}[analysis.kind]
    if raw.name.lower() != expected.lower():
        raise SimulationError(f"expected {expected}, got {raw.name}")
    axis_name, axis_unit = {"ac": ("frequency", "Hz"), "transient": ("time", "s"), "op": ("sample", "1")}[analysis.kind]
    if analysis.kind == "op":
        axis = (0.0,)
        if any(len(v) != 1 for v in raw.vectors.values()):
            raise SimulationError("operating point must contain one sample")
    else:
        if axis_name not in raw.vectors:
            raise SimulationError(f"missing {axis_name} axis")
        axis = tuple(v.real for v in raw.vectors[axis_name])
        if len(axis) < 2 or any(b <= a for a, b in zip(axis, axis[1:])):
            raise SimulationError("simulation axis must increase strictly")
        stop = float(analysis.stop.base_value)
        if analysis.kind == "transient":
            if abs(axis[0]) > max(1e-15, stop * 1e-10) or abs(axis[-1] - stop) > max(1e-15, stop * 1e-7):
                raise SimulationError("transient results do not cover the requested time range")
        else:
            start = float(analysis.start.base_value)
            if analysis.sweep == "linear":
                expected_axis = tuple(start + i * (stop - start) / (analysis.points - 1) for i in range(analysis.points))
            else:
                base = 10 if analysis.sweep == "decade" else 2
                steps = math.floor(analysis.points * math.log(stop / start, base) + 1e-8)
                expected_axis = tuple(start * base ** (i / analysis.points) for i in range(steps + 1))
            if len(axis) != len(expected_axis) or any(not math.isclose(a, b, rel_tol=1e-6) for a, b in zip(axis, expected_axis)):
                raise SimulationError("AC results do not cover the requested frequency grid")
    count = len(axis)
    def voltage(node):
        if node == "0":
            return (0j,) * count
        name = f"v({node})".lower()
        if name not in raw.vectors or len(raw.vectors[name]) != count:
            raise SimulationError(f"missing voltage vector {name}")
        return raw.vectors[name]
    traces = {}
    source_map = source_names(circuit)
    for probe in circuit.plan.probes:
        if probe.kind == "voltage":
            positive, negative = voltage(circuit.node(probe.positive)), voltage(circuit.node(probe.negative))
            traces[probe.name] = Trace(probe.name, "V", tuple(p - n for p, n in zip(positive, negative)))
        else:
            source = source_map[probe.source]
            alternatives = (f"i({source})", f"{source}#branch")
            vector = next((raw.vectors[n] for n in alternatives if n in raw.vectors), None)
            if vector is None or len(vector) != count:
                raise SimulationError(f"missing current vector for source {probe.source}")
            traces[probe.name] = Trace(probe.name, "A", tuple(probe.sign * v for v in vector))
    diagnostics = []
    for signal in circuit.plan.derived_signals:
        if signal.analysis is not None and signal.analysis != analysis.name:
            continue
        left, right = traces[signal.left], traces[signal.right]
        if signal.kind in {"ratio", "difference"} and left.unit != right.unit:
            raise SimulationError(f"{signal.name}: operands must have the same dimensions")
        if signal.kind == "product":
            units = {("V", "A"): "W", ("A", "V"): "W", ("1", "V"): "V", ("V", "1"): "V",
                     ("1", "A"): "A", ("A", "1"): "A", ("1", "1"): "1"}
            if (left.unit, right.unit) not in units:
                raise SimulationError(f"{signal.name}: unsupported product dimensions")
            unit = units[(left.unit, right.unit)]
        else:
            unit = "1" if signal.kind == "ratio" else left.unit
        values = []
        for l, r in zip(left.values, right.values):
            if l is None or r is None or (signal.kind == "ratio" and abs(r) <= 1e-30):
                values.append(None); continue
            value = l / r if signal.kind == "ratio" else l * r if signal.kind == "product" else l - r
            values.append(value if math.isfinite(value.real) and math.isfinite(value.imag) else None)
        invalid = sum(v is None for v in values)
        if invalid:
            diagnostics.append(f"{signal.name}: {invalid} invalid samples (zero denominator or nonfinite result)")
        traces[signal.name] = Trace(signal.name, unit, tuple(values))
    return Waveforms(analysis, case, axis_name, axis_unit, axis, traces, tuple(diagnostics))


def represented(trace: Trace, representation: str, *, unwrap=False) -> tuple[float | None, ...]:
    values = []
    previous = None
    for value in trace.values:
        if value is None:
            converted = None
        elif representation == "real":
            converted = value.real
        elif representation == "magnitude":
            converted = abs(value)
        elif representation == "dB":
            converted = 20 * math.log10(abs(value)) if abs(value) > 0 else None
        elif representation == "phase":
            converted = math.degrees(cmath.phase(value)) if abs(value) > 1e-30 else None
            if unwrap and converted is not None and previous is not None:
                converted += 360 * round((previous - converted) / 360)
        else:
            raise SimulationError(f"unknown representation {representation}")
        if converted is not None and not math.isfinite(converted):
            converted = None
        values.append(converted)
        if representation == "phase":
            previous = converted
    return tuple(values)
