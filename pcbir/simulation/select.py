"""Resolve an explicitly selected circuit without changing authoritative IR."""
from __future__ import annotations

from dataclasses import dataclass
import math
from types import MappingProxyType
from typing import Mapping

from ..elaborate import elaborate
from ..model import Board, ComponentInstance, Endpoint
from ..pin_resolution import resolve_package_pin
from ..quantities import Capacitance, Inductance, Resistance, Quantity
from .model import SimulationError, SimulationPlan, quantity
from .models import ModelBundle, load_registry


@dataclass(frozen=True, slots=True)
class Element:
    ref: str
    kind: str
    nodes: tuple[str, ...]
    value: Quantity | None = None
    model: ModelBundle | None = None


@dataclass(frozen=True, slots=True)
class SimulationCircuit:
    board_name: str
    plan: SimulationPlan
    elements: tuple[Element, ...]
    nodes: Mapping[str, str]
    aliases: Mapping[str, str]
    boundaries: tuple[tuple[str, str, str], ...]
    models: tuple[ModelBundle, ...]

    def __post_init__(self):
        object.__setattr__(self, "nodes", MappingProxyType(dict(self.nodes)))
        object.__setattr__(self, "aliases", MappingProxyType(dict(self.aliases)))

    def node(self, name: str) -> str:
        resolved = self.aliases.get(name, name)
        try:
            return self.nodes[resolved]
        except KeyError as exc:
            raise SimulationError(f"node {name!r} is unknown or outside the selected circuit") from exc


def select_circuit(board: Board, plan: SimulationPlan) -> SimulationCircuit:
    try:
        flat = elaborate(board)
    except ValueError as exc:
        raise SimulationError(str(exc)) from exc
    components = {c.ref: c for c in flat.components}
    if len(components) != len(flat.components) or len({n.name for n in flat.nets}) != len(flat.nets):
        raise SimulationError("duplicate component or net names in electrical IR")
    selected = set(components) if plan.all_components else set(plan.components)
    module_paths = {m.path for m in flat.module_instances}
    for path in plan.modules:
        if path not in module_paths:
            raise SimulationError(f"unknown module instance {path!r}")
        selected.update(ref for ref in components if ref.startswith(path + "/"))
    if not selected or selected - components.keys():
        raise SimulationError(f"empty circuit or unknown components: {sorted(selected - components.keys())}")
    if plan.bindings.keys() - selected or plan.values.keys() - selected:
        raise SimulationError("model bindings/value overrides refer to excluded or unknown components")
    aliases = {f"{m.path}.{port}": net for m in flat.module_instances for port, net in m.connections.items()}
    pin_nets = {}
    endpoints = {}
    def pin(component: ComponentInstance, terminal: str):
        try:
            return resolve_package_pin(flat, component, terminal)
        except (ValueError, KeyError) as exc:
            raise SimulationError(str(exc)) from exc
    for net in flat.nets:
        for endpoint in net.endpoints:
            if endpoint.component not in components:
                raise SimulationError(f"unresolved endpoint {endpoint} in elaborated IR")
            p = pin(components[endpoint.component], endpoint.pin)
            key = (endpoint.component, p.number)
            if key in pin_nets and pin_nets[key] != net.name:
                raise SimulationError(f"{endpoint}: package pin is connected to more than one net")
            pin_nets[key] = net.name
            endpoints[str(endpoint)] = (endpoint.component, p.number, net.name)
            for alias in (p.name, p.number):
                aliases[f"{endpoint.component}.{alias}"] = net.name
    # Internal conductive package groups can provide a contact for another terminal.
    for component in components.values():
        part = flat.library[component.part]
        for group in part.internal_pad_groups:
            nets = {pin_nets[(component.ref, n)] for n in group.numbers if (component.ref, n) in pin_nets}
            if len(nets) > 1:
                raise SimulationError(f"{component.ref}: internally connected contacts span different named nets")
            if nets:
                net = next(iter(nets))
                for number in group.numbers:
                    pin_nets[(component.ref, number)] = net
        for p in part.pins.values():
            if (component.ref, p.number) in pin_nets:
                aliases[f"{component.ref}.{p.name}"] = pin_nets[(component.ref, p.number)]
                aliases[f"{component.ref}.{p.number}"] = pin_nets[(component.ref, p.number)]
    registry = load_registry(plan.registry)
    elements = []
    used_models = {}
    for ref in sorted(selected):
        component = components[ref]
        part = flat.library[component.part]
        def connection(terminal):
            p = pin(component, terminal)
            try:
                return pin_nets[(ref, p.number)]
            except KeyError as exc:
                raise SimulationError(f"{ref}.{terminal}: model terminal is not connected") from exc
        if ref in plan.bindings:
            model_id = plan.bindings[ref]
            if model_id not in registry:
                raise SimulationError(f"{ref}: unknown model ID {model_id!r}")
            model = registry[model_id]
            if component.part not in model.supported_parts and part.name not in model.supported_parts:
                raise SimulationError(f"{ref}: model {model_id} is not qualified for part {component.part}")
            if {a.kind for a in plan.analyses} - set(model.analyses):
                raise SimulationError(f"{ref}: model {model_id} does not support the requested analyses")
            if ref in plan.values:
                raise SimulationError(f"{ref}: value override on a vendor model is unsupported; use a separately qualified model")
            mapped = {pin(component, v).number for v in model.pin_map.values()}
            ignored = {pin(component, v).number for v in model.ignored_pins}
            if mapped & ignored or mapped | ignored != {p.number for p in part.pins.values()}:
                raise SimulationError(f"{ref}: model mapping must account for every package pin exactly as mapped or ignored")
            nodes = tuple(connection(model.pin_map[t]) for t in model.terminals)
            used_models[model_id] = model
            elements.append(Element(ref, model.kind, nodes, model=model))
        else:
            primitive = {"passive.resistor": ("resistor", Resistance), "passive.capacitor": ("capacitor", Capacitance),
                         "passive.inductor": ("inductor", Inductance)}.get(part.category)
            if primitive is None:
                raise SimulationError(f"{ref} ({component.part}): missing simulation model; select a subset or bind a checked model")
            kind, dimension = primitive
            value = quantity(plan.values[ref], dimension, f"value {ref}", positive=True) if ref in plan.values else component.value
            if type(value) is not dimension or not value.base_value.is_finite() or not math.isfinite(float(value.base_value)) or float(value.base_value) <= 0:
                raise SimulationError(f"{ref}: {kind} requires a positive finite typed value")
            numbers = sorted({p.number for p in part.pins.values()})
            if len(numbers) != 2:
                raise SimulationError(f"{ref}: native {kind} requires exactly two package terminals")
            elements.append(Element(ref, kind, tuple(connection(n) for n in numbers), value))
    included_nets = {n for e in elements for n in e.nodes}
    model_names = [m.name.lower() for m in used_models.values()]
    if len(model_names) != len(set(model_names)):
        raise SimulationError("selected model IDs define the same SPICE model name")
    reference = aliases.get(plan.reference_node, plan.reference_node)
    if reference not in included_nets:
        raise SimulationError("reference_node must be a net in the selected circuit")
    nodes = {reference: "0"}
    nodes.update({n: f"n{i:06d}" for i, n in enumerate(sorted(included_nets - {reference}), 1)})
    boundary_endpoints = {e: (owner, number, net) for e, (owner, number, net) in endpoints.items()
                          if owner not in selected and net in included_nets}
    coverage = {}
    for source in plan.sources:
        p, n = aliases.get(source.positive, source.positive), aliases.get(source.negative, source.negative)
        if p not in nodes or n not in nodes or p == n:
            raise SimulationError(f"source/load {source.name}: terminals must be distinct selected circuit nodes")
        for endpoint in source.covers:
            if endpoint not in boundary_endpoints or boundary_endpoints[endpoint][2] not in {p, n} or endpoint in coverage:
                raise SimulationError(f"{endpoint}: invalid or duplicate boundary replacement by {source.name}")
            coverage[endpoint] = source.name
    for endpoint, reason in plan.open_boundaries.items():
        if endpoint not in boundary_endpoints or endpoint in coverage:
            raise SimulationError(f"{endpoint}: unknown or already covered open boundary")
        coverage[endpoint] = "open: " + reason
    missing = boundary_endpoints.keys() - coverage.keys()
    if missing:
        raise SimulationError(f"unexplained subset boundaries: {', '.join(sorted(missing))}; declare covers or open_boundaries")
    circuit = SimulationCircuit(flat.name, plan, tuple(elements), nodes, aliases,
        tuple((e, boundary_endpoints[e][2], coverage[e]) for e in sorted(coverage)), tuple(used_models[n] for n in sorted(used_models)))
    for probe in plan.probes:
        if probe.kind == "voltage":
            circuit.node(probe.positive); circuit.node(probe.negative)
    for net in plan.initial_voltages:
        resolved = circuit.node(net)
        if resolved == "0" and plan.initial_voltages[net].base_value != 0:
            raise SimulationError("reference node cannot have a nonzero initial voltage")
    return circuit
