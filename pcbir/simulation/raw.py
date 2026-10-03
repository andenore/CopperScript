"""Strict ASCII ngspice raw reader; complex samples are retained losslessly."""
from __future__ import annotations

import math
from dataclasses import dataclass
from types import MappingProxyType
from typing import Mapping

from .model import SimulationError


@dataclass(frozen=True, slots=True)
class RawPlot:
    name: str
    vectors: Mapping[str, tuple[complex, ...]]
    units: Mapping[str, str]

    def __post_init__(self):
        object.__setattr__(self, "vectors", MappingProxyType(dict(self.vectors)))
        object.__setattr__(self, "units", MappingProxyType(dict(self.units)))


def parse_raw(content: str) -> tuple[RawPlot, ...]:
    lines = content.splitlines()
    index = 0
    plots = []
    try:
        while index < len(lines):
            if not lines[index].strip():
                index += 1; continue
            header = {}
            while index < len(lines) and lines[index].strip() != "Variables:":
                key, separator, value = lines[index].partition(":")
                if not separator:
                    raise SimulationError("invalid raw header")
                header[key.lower().strip()] = value.strip()
                index += 1
            count = int(header["no. variables"])
            points = int(header["no. points"])
            if count < 1 or points < 1:
                raise SimulationError("raw output contains no samples or variables")
            if "binary" in header.get("flags", ""):
                raise SimulationError("expected ASCII raw output")
            index += 1
            names, units = [], {}
            for variable in range(count):
                tokens = lines[index].split()
                if len(tokens) < 3 or int(tokens[0]) != variable or tokens[1].lower() in units:
                    raise SimulationError("invalid or duplicate raw variable")
                name = tokens[1].lower()
                names.append(name); units[name] = tokens[2].lower(); index += 1
            if lines[index].strip() != "Values:":
                raise SimulationError("raw output is missing ASCII Values")
            index += 1
            vectors = {n: [] for n in names}
            for point in range(points):
                while index < len(lines) and not lines[index].strip():
                    index += 1
                for variable, name in enumerate(names):
                    line = lines[index].strip()
                    if variable == 0:
                        tokens = line.split(None, 1)
                        if len(tokens) != 2 or int(tokens[0]) != point:
                            raise SimulationError("raw sample index is invalid")
                        line = tokens[1]
                    parts = line.split(",")
                    if len(parts) not in {1, 2}:
                        raise SimulationError("invalid complex raw value")
                    value = complex(float(parts[0]), float(parts[1]) if len(parts) == 2 else 0)
                    if not math.isfinite(value.real) or not math.isfinite(value.imag):
                        raise SimulationError("raw output contains nonfinite values")
                    vectors[name].append(value); index += 1
            plots.append(RawPlot(header["plotname"], {n: tuple(v) for n, v in vectors.items()}, units))
    except (IndexError, KeyError, ValueError) as exc:
        if isinstance(exc, SimulationError):
            raise
        raise SimulationError("truncated or malformed ASCII raw output") from exc
    if not plots:
        raise SimulationError("raw output contains no plots")
    return tuple(plots)
