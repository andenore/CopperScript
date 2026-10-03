"""Immutable simulation plans and strict, dimension-aware JSON loading."""
from __future__ import annotations

import json
import math
import re
from dataclasses import dataclass
from decimal import InvalidOperation
from pathlib import Path
from types import MappingProxyType
from typing import Mapping

from ..quantities import Current, Frequency, Quantity, Resistance, Time, Voltage


class SimulationError(ValueError):
    """An invalid plan, circuit, model, or simulator result."""


def object_fields(value: object, allowed: set[str], context: str) -> dict:
    if not isinstance(value, dict):
        raise SimulationError(f"{context}: expected an object")
    unknown = value.keys() - allowed
    if unknown:
        raise SimulationError(f"{context}: unknown fields {sorted(unknown)}")
    return value


def required(value: Mapping, key: str, context: str):
    if key not in value:
        raise SimulationError(f"{context}: missing {key!r}")
    return value[key]


def identifier(value: object, context: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]{0,63}", value):
        raise SimulationError(f"{context}: use a name beginning with a letter, followed by letters, digits, '_' or '-'")
    return value


def strings(value: object, context: str) -> tuple[str, ...]:
    if not isinstance(value, list) or any(not isinstance(v, str) or not v.strip() for v in value):
        raise SimulationError(f"{context}: expected a list of nonempty strings")
    if len(set(value)) != len(value):
        raise SimulationError(f"{context}: duplicate entries")
    return tuple(value)


def text(value: object, context: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise SimulationError(f"{context}: expected a nonempty string")
    return value


def quantity(value: object, kind: type[Quantity], context: str, *, positive=False, nonnegative=False) -> Quantity:
    if not isinstance(value, str):
        raise SimulationError(f"{context}: expected a number with {kind.__name__} units")
    match = re.fullmatch(r"\s*([+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)\s*([^\s]+)\s*", value)
    if not match:
        raise SimulationError(f"{context}: invalid quantity {value!r}")
    unit = match[2].replace("µ", "u").replace("μ", "u").replace("Ω", "ohm")
    try:
        result = kind.of(match[1], unit)
        numeric = float(result.base_value)
    except (ValueError, InvalidOperation, OverflowError) as exc:
        raise SimulationError(f"{context}: {exc}") from exc
    if not math.isfinite(numeric) or (numeric == 0 and result.base_value != 0) or (positive and numeric <= 0) or (nonnegative and result.base_value < 0):
        raise SimulationError(f"{context}: expected a finite {'positive' if positive else 'nonnegative' if nonnegative else ''} quantity")
    return result


def number(value: object, context: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise SimulationError(f"{context}: expected a finite number")
    return float(value)


def angle(value: object, context: str) -> float:
    if not isinstance(value, str) or not value.endswith(" deg"):
        raise SimulationError(f"{context}: expected degrees, e.g. '0 deg'")
    try:
        result = float(value[:-4])
    except ValueError as exc:
        raise SimulationError(f"{context}: invalid angle") from exc
    return number(result, context)


def read_json(path: Path) -> dict:
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise SimulationError(f"{path}: duplicate JSON key {key!r}")
            result[key] = value
        return result
    try:
        result = json.loads(path.read_text(encoding="utf-8-sig"), object_pairs_hook=unique,
                            parse_constant=lambda v: (_ for _ in ()).throw(SimulationError(f"nonfinite JSON value {v}")))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SimulationError(f"cannot read {path}: {exc}") from exc
    if not isinstance(result, dict):
        raise SimulationError(f"{path}: expected a JSON object")
    return result


@dataclass(frozen=True, slots=True)
class Stimulus:
    name: str
    kind: str
    positive: str
    negative: str
    dc: Quantity
    ac_magnitude: Quantity | None = None
    ac_phase: float = 0
    waveform: tuple[tuple[Time, Quantity], ...] = ()
    series_resistance: Resistance | None = None
    covers: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class Analysis:
    name: str
    kind: str
    start: Quantity | None = None
    stop: Quantity | None = None
    max_step: Time | None = None
    sweep: str = "decade"
    points: int = 100


@dataclass(frozen=True, slots=True)
class Probe:
    name: str
    kind: str
    positive: str | None = None
    negative: str | None = None
    source: str | None = None
    sign: int = 1


@dataclass(frozen=True, slots=True)
class DerivedSignal:
    name: str
    kind: str
    left: str
    right: str
    analysis: str | None = None


@dataclass(frozen=True, slots=True)
class SimulationPlan:
    name: str
    document: str
    directory: Path
    modules: tuple[str, ...]
    components: tuple[str, ...]
    all_components: bool
    reference_node: str
    sources: tuple[Stimulus, ...]
    analyses: tuple[Analysis, ...]
    probes: tuple[Probe, ...]
    derived_signals: tuple[DerivedSignal, ...]
    registry: Path | None
    bindings: Mapping[str, str]
    values: Mapping[str, str]
    open_boundaries: Mapping[str, str]
    initial_method: str
    initial_voltages: Mapping[str, Voltage]
    cases: tuple[str, ...]
    assumptions: tuple[str, ...]

    def __post_init__(self):
        for key in ("bindings", "values", "open_boundaries", "initial_voltages"):
            object.__setattr__(self, key, MappingProxyType(dict(getattr(self, key))))

    @property
    def data(self) -> dict:
        """Return a detached JSON copy; callers cannot mutate the plan."""
        return json.loads(self.document)

    def for_case(self, name: str) -> SimulationPlan:
        if name not in self.cases:
            raise SimulationError(f"unknown simulation case {name!r}")
        data = self.data
        overrides = data.pop("cases", {}).get(name, {})
        for key in ("values", "sources", "loads"):
            if key in overrides:
                data.setdefault(key, {})
                for entry, value in overrides[key].items():
                    if key != "values" and entry not in data[key]:
                        raise SimulationError(f"case {name}: unknown {key} entry {entry!r}")
                    if key == "values":
                        data[key][entry] = value
                    else:
                        data[key][entry] = _merge(data[key][entry], value)
        return parse_plan(data, directory=self.directory)


def _merge(base, update):
    if not isinstance(update, dict):
        raise SimulationError("source/load case override must be an object")
    result = dict(base)
    for key, value in update.items():
        result[key] = _merge(result[key], value) if isinstance(result.get(key), dict) and isinstance(value, dict) else value
    return result


def _stimulus(name, raw, *, load=False) -> Stimulus:
    context = f"{'loads' if load else 'sources'}.{name}"
    identifier(name, context)
    data = object_fields(raw, {"kind", "positive", "negative", "dc", "ac", "waveform", "series_resistance", "covers",
                               "current", "enable_at", "rise_time"}, context)
    kind = required(data, "kind", context)
    if kind not in {"voltage", "current", "current_sink"} or (load and kind != "current_sink"):
        raise SimulationError(f"{context}: unsupported source/load kind {kind!r}")
    dimension = Voltage if kind == "voltage" else Current
    dc = quantity(data.get("dc", "0 V" if dimension is Voltage else "0 A"), dimension, context + ".dc")
    ac = object_fields(data.get("ac", {}), {"magnitude", "phase"}, context + ".ac")
    ac_magnitude = quantity(required(ac, "magnitude", context), dimension, context + ".ac.magnitude", nonnegative=True) if ac else None
    ac_phase = angle(ac.get("phase", "0 deg"), context + ".ac.phase")
    waveform = ()
    if "waveform" in data:
        w = object_fields(data["waveform"], {"kind", "from", "to", "duration", "delay", "points"}, context + ".waveform")
        if w.get("kind") == "ramp":
            if "points" in w:
                raise SimulationError(f"{context}: a ramp does not accept points")
            first = quantity(required(w, "from", context), dimension, context)
            last = quantity(required(w, "to", context), dimension, context)
            delay = quantity(w.get("delay", "0 s"), Time, context, nonnegative=True)
            duration = quantity(required(w, "duration", context), Time, context, positive=True)
            end = Time.of(delay.base_value + duration.base_value, "s")
            waveform = ((Time.of(0, "s"), first), *(((delay, first),) if delay.base_value else ()), (end, last))
        elif w.get("kind") == "pwl":
            if set(w) - {"kind", "points"}:
                raise SimulationError(f"{context}: PWL accepts only points")
            points = required(w, "points", context)
            if not isinstance(points, list) or len(points) < 2 or any(not isinstance(p, list) or len(p) != 2 for p in points):
                raise SimulationError(f"{context}: PWL requires at least two [time, value] points")
            waveform = tuple((quantity(p[0], Time, context, nonnegative=True), quantity(p[1], dimension, context)) for p in points)
            if waveform[0][0].base_value != 0 or any(b[0].base_value <= a[0].base_value for a, b in zip(waveform, waveform[1:])):
                raise SimulationError(f"{context}: PWL must start at zero and increase strictly in time")
        else:
            raise SimulationError(f"{context}: waveform kind must be ramp or pwl")
    if "current" in data:
        if kind != "current_sink" or waveform or "dc" in data:
            raise SimulationError(f"{context}: current shorthand requires a current_sink without dc/waveform")
        current = quantity(data["current"], Current, context, nonnegative=True)
        delay = quantity(data.get("enable_at", "0 s"), Time, context, nonnegative=True)
        rise = quantity(data.get("rise_time", "1 us"), Time, context, positive=True)
        zero = Current.of(0, "A")
        waveform = ((Time.of(0, "s"), zero), *(((delay, zero),) if delay.base_value else ()),
                    (Time.of(delay.base_value + rise.base_value, "s"), current))
    elif "enable_at" in data or "rise_time" in data:
        raise SimulationError(f"{context}: enable_at/rise_time require current")
    if waveform:
        if "dc" in data and dc.base_value != waveform[0][1].base_value:
            raise SimulationError(f"{context}: dc must equal the waveform's initial value for consistent startup")
        dc = waveform[0][1]
    resistance = quantity(data["series_resistance"], Resistance, context, positive=True) if "series_resistance" in data else None
    if resistance is not None and kind != "voltage":
        raise SimulationError(f"{context}: series_resistance is only supported for voltage sources")
    return Stimulus(name, kind, text(required(data, "positive", context), context), text(required(data, "negative", context), context),
                    dc, ac_magnitude, ac_phase, waveform, resistance, strings(data.get("covers", []), context + ".covers"))


def _analysis(name, raw) -> Analysis:
    identifier(name, "analysis name")
    data = object_fields(raw, {"kind", "start", "stop", "max_step", "sweep", "points_per_decade", "points_per_octave", "points"}, name)
    kind = required(data, "kind", name)
    if kind == "op":
        if set(data) != {"kind"}:
            raise SimulationError("operating point accepts only kind")
        return Analysis(name, kind)
    if kind == "transient":
        if set(data) - {"kind", "stop", "max_step"}:
            raise SimulationError("transient accepts kind, stop and max_step")
        stop = quantity(required(data, "stop", name), Time, name, positive=True)
        step = quantity(required(data, "max_step", name), Time, name, positive=True)
        if step.base_value > stop.base_value:
            raise SimulationError("max_step must not exceed stop time")
        return Analysis(name, kind, Time.of(0, "s"), stop, step)
    if kind != "ac":
        raise SimulationError(f"unsupported analysis kind {kind!r}")
    sweep = data.get("sweep", "decade")
    resolution = {"decade": "points_per_decade", "octave": "points_per_octave", "linear": "points"}.get(sweep)
    if resolution is None or set(data) - {"kind", "start", "stop", "sweep", resolution}:
        raise SimulationError("AC sweep requires decade/points_per_decade, octave/points_per_octave or linear/points")
    points = required(data, resolution, name)
    if isinstance(points, bool) or not isinstance(points, int) or points < (2 if sweep == "linear" else 1) or points > 1_000_000:
        raise SimulationError("invalid AC point count")
    start = quantity(required(data, "start", name), Frequency, name, positive=True)
    stop = quantity(required(data, "stop", name), Frequency, name, positive=True)
    if start.base_value >= stop.base_value:
        raise SimulationError("AC start frequency must be below stop")
    return Analysis(name, kind, start, stop, sweep=sweep, points=points)


def parse_plan(raw: dict, *, directory: Path | None = None) -> SimulationPlan:
    data = object_fields(raw, {"schema_version", "name", "circuit", "models", "sources", "loads", "analysis", "analyses", "probes",
                              "derived_signals", "checks", "plots", "outputs", "initial_state", "values", "cases", "assumptions"}, "plan")
    if type(data.get("schema_version")) is not int or data["schema_version"] != 1:
        raise SimulationError("expected schema_version: 1")
    name = identifier(required(data, "name", "plan"), "plan name")
    circuit = object_fields(required(data, "circuit", "plan"), {"modules", "components", "all", "reference_node", "open_boundaries"}, "circuit")
    modules = strings(circuit.get("modules", []), "circuit.modules")
    components = strings(circuit.get("components", []), "circuit.components")
    all_components = circuit.get("all", False)
    if not isinstance(all_components, bool) or not (all_components or modules or components) or (all_components and (modules or components)):
        raise SimulationError("select circuit.all or explicit modules/components")
    opens = circuit.get("open_boundaries", {})
    if not isinstance(opens, dict):
        raise SimulationError("open_boundaries must map endpoints to reasons")
    for endpoint, reason in opens.items():
        text(endpoint, "open boundary endpoint"); text(reason, "open boundary reason")
    sources_raw, loads_raw = data.get("sources", {}), data.get("loads", {})
    if not isinstance(sources_raw, dict) or not isinstance(loads_raw, dict) or sources_raw.keys() & loads_raw.keys():
        raise SimulationError("sources and loads must be objects with distinct names")
    sources = tuple(_stimulus(n, r) for n, r in sorted(sources_raw.items())) + tuple(_stimulus(n, r, load=True) for n, r in sorted(loads_raw.items()))
    if not sources:
        raise SimulationError("at least one explicit source is required")
    if ("analysis" in data) == ("analyses" in data):
        raise SimulationError("provide exactly one of analysis or analyses")
    analyses_raw = data.get("analyses") or {"main": data.get("analysis")}
    if not isinstance(analyses_raw, dict) or not analyses_raw:
        raise SimulationError("analyses must be a nonempty object")
    analyses = tuple(_analysis(n, r) for n, r in sorted(analyses_raw.items()))
    if any(a.kind == "ac" for a in analyses) and not any(s.ac_magnitude and s.ac_magnitude.base_value > 0 for s in sources):
        raise SimulationError("AC analysis requires a nonzero AC source excitation")
    probes_raw = required(data, "probes", "plan")
    if not isinstance(probes_raw, dict) or not probes_raw:
        raise SimulationError("probes must be a nonempty object")
    probes = []
    for n, r in sorted(probes_raw.items()):
        identifier(n, "probe name")
        p = object_fields(r, {"kind", "positive", "negative", "source", "positive_direction"}, f"probe {n}")
        if p.get("kind") == "voltage" and not (set(p) - {"kind", "positive", "negative"}):
            probes.append(Probe(n, "voltage", text(required(p, "positive", n), n), text(required(p, "negative", n), n)))
        elif p.get("kind") == "source_current" and not (set(p) - {"kind", "source", "positive_direction"}):
            source = required(p, "source", n)
            if source not in {s.name for s in sources if s.kind == "voltage"}:
                raise SimulationError(f"probe {n}: source_current requires a voltage source")
            direction = p.get("positive_direction", "delivered")
            if direction not in {"delivered", "absorbed"}:
                raise SimulationError(f"probe {n}: invalid current direction")
            probes.append(Probe(n, "source_current", source=source, sign=-1 if direction == "delivered" else 1))
        else:
            raise SimulationError(f"probe {n}: unsupported fields or probe kind")
    derived_raw = data.get("derived_signals", {})
    if not isinstance(derived_raw, dict):
        raise SimulationError("derived_signals must be an object")
    derived = []
    for n, r in derived_raw.items():
        identifier(n, "derived signal name")
        d = object_fields(r, {"kind", "numerator", "denominator", "left", "right", "analysis"}, n)
        kind = d.get("kind")
        keys = ("numerator", "denominator") if kind == "ratio" else ("left", "right")
        if kind not in {"ratio", "product", "difference"} or set(d) - {"analysis"} != {"kind", *keys}:
            raise SimulationError(f"derived signal {n}: invalid typed operation")
        if "analysis" in d and d["analysis"] not in {a.name for a in analyses}:
            raise SimulationError(f"derived signal {n}: unknown analysis")
        derived.append(DerivedSignal(n, kind, text(d[keys[0]], n), text(d[keys[1]], n), d.get("analysis")))
    names = {p.name for p in probes}
    ordered = []
    pending = sorted(derived, key=lambda s: s.name)
    if names.intersection(s.name for s in pending):
        raise SimulationError("derived signal repeats a probe name")
    while pending:
        ready = [s for s in pending if s.left in names and s.right in names]
        if not ready:
            raise SimulationError("derived signals contain unknown operands or a dependency cycle")
        for signal in ready:
            ordered.append(signal); names.add(signal.name); pending.remove(signal)
    derived = ordered
    models = object_fields(data.get("models", {}), {"registry", "bindings"}, "models")
    bindings = models.get("bindings", {})
    if not isinstance(bindings, dict) or any(not isinstance(v, str) for v in bindings.values()):
        raise SimulationError("model bindings must map component references to model IDs")
    if bindings and "registry" not in models:
        raise SimulationError("model bindings require a registry")
    directory = (directory or Path.cwd()).resolve()
    registry = (directory / text(models["registry"], "models.registry")).resolve() if "registry" in models else None
    values = data.get("values", {})
    if not isinstance(values, dict) or any(not isinstance(v, str) for v in values.values()):
        raise SimulationError("values must map component references to quantities")
    initial = object_fields(data.get("initial_state", {}), {"method", "node_voltages"}, "initial_state")
    method = initial.get("method", "operating_point")
    if method not in {"operating_point", "uic"}:
        raise SimulationError("initial method must be operating_point or explicitly uic")
    initial_raw = initial.get("node_voltages", {})
    if not isinstance(initial_raw, dict):
        raise SimulationError("initial node_voltages must be an object")
    if (initial_raw or method == "uic") and any(a.kind != "transient" for a in analyses):
        raise SimulationError("initial voltages/UIC require a transient-only plan")
    initial_voltages = {n: quantity(v, Voltage, "initial node voltage") for n, v in initial_raw.items()}
    cases = data.get("cases", {"nominal": {}})
    if not isinstance(cases, dict) or not cases:
        raise SimulationError("cases must be a nonempty object")
    for n, case in cases.items():
        identifier(n, "case name")
        c = object_fields(case, {"values", "sources", "loads"}, f"case {n}")
        if any(not isinstance(v, dict) for v in c.values()):
            raise SimulationError(f"case {n}: overrides must be objects")
    # Validate graph/check fields here; dimensional compatibility is checked after circuit resolution.
    from .measure import validate_checks
    from .report import validate_plots
    validate_checks(data.get("checks", []), names, analyses)
    validate_plots(data.get("plots", []), names, analyses)
    outputs = object_fields(data.get("outputs", {}), {"report", "plots", "data"}, "outputs")
    output_plots = strings(outputs.get("plots", ["svg", "png"]), "outputs.plots")
    output_data = strings(outputs.get("data", ["csv", "json"]), "outputs.data")
    if outputs.get("report", "html") != "html" or set(output_plots) - {"svg", "png"} or set(output_data) - {"csv", "json"}:
        raise SimulationError("outputs support html, svg/png plots and csv/json data")
    return SimulationPlan(name, json.dumps(data, sort_keys=True, indent=2, allow_nan=False), directory, modules, components,
                          all_components, text(required(circuit, "reference_node", "circuit"), "reference_node"), sources, analyses,
                          tuple(probes), tuple(derived), registry, bindings, values, opens, method, initial_voltages,
                          tuple(sorted(cases)), strings(data.get("assumptions", []), "assumptions"))


def load_plan(path: str | Path) -> SimulationPlan:
    path = Path(path).resolve()
    return parse_plan(read_json(path), directory=path.parent)
