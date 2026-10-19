"""Bounded DC nodal analysis for explicitly supplied resistor networks.

Not a copper extractor or a thermal/AC qualification model. Input fractions are
retained exactly; the linear solve uses floating point and verifies KCL residuals.
Positive withdrawal means current leaving a node (negative values model returns).
"""
from dataclasses import dataclass
from fractions import Fraction
from math import fsum, isfinite
from types import MappingProxyType
from typing import Mapping


def _exact(value, name):
    if isinstance(value, (bool, float)):
        raise ValueError(f'{name} requires an exact integer, decimal or fraction')
    try:
        value = Fraction(value)
        if not isfinite(float(value)):
            raise ValueError()
        return value
    except (TypeError, ValueError, OverflowError, ZeroDivisionError) as exc:
        raise ValueError(f'invalid {name}') from exc


@dataclass(frozen=True)
class ResistorEdge:
    id: str
    first: str
    second: str
    resistance_ohms: Fraction

    def __post_init__(self):
        resistance = _exact(self.resistance_ohms, 'resistance')
        if (any(not isinstance(v, str) or not v for v in (self.id, self.first, self.second))
                or self.first == self.second or resistance <= 0):
            raise ValueError('resistor requires distinct nodes and positive resistance')
        if float(resistance) == 0 or not isfinite(1 / float(resistance)):
            raise ValueError('resistance outside solver range')
        object.__setattr__(self, 'resistance_ohms', resistance)


@dataclass(frozen=True)
class DCNetwork:
    edges: tuple[ResistorEdge, ...]
    fixed_voltages_v: Mapping[str, Fraction]
    withdrawals_a: Mapping[str, Fraction]

    def __post_init__(self):
        object.__setattr__(self, 'edges', tuple(self.edges))
        if len({e.id for e in self.edges}) != len(self.edges):
            raise ValueError('resistor IDs must be unique')
        for name in ('fixed_voltages_v', 'withdrawals_a'):
            values = getattr(self, name)
            if any(not isinstance(node, str) or not node for node in values):
                raise ValueError('network nodes must have nonempty names')
            object.__setattr__(self, name, MappingProxyType({n: _exact(v, name) for n, v in values.items()}))


@dataclass(frozen=True)
class DCSolution:
    status: str
    voltages_v: Mapping[str, float]
    edge_currents_a: Mapping[str, float]
    source_currents_a: Mapping[str, float]
    loss_w: float | None
    maximum_kcl_residual_a: float | None
    iterations: int
    unresolved_nodes: tuple[str, ...] = ()


def solve_dc(network: DCNetwork, *, maximum_nodes=4096, maximum_iterations=2048,
             tolerance_a=1e-9) -> DCSolution:
    """Jacobi-preconditioned conjugate gradients with absolute KCL acceptance.

Every connected component must contain a declared voltage boundary, including
zero-load islands. Budget exhaustion and floating islands produce no voltages.
Parallel resistors are separate branches; no series-sum shortcut is used.
"""
    if (type(maximum_nodes) is not int or maximum_nodes <= 0
            or type(maximum_iterations) is not int or maximum_iterations <= 0
            or isinstance(tolerance_a, bool) or not isfinite(tolerance_a) or tolerance_a <= 0):
        raise ValueError('invalid DC solve bounds')
    nodes = set(network.fixed_voltages_v) | set(network.withdrawals_a)
    for edge in network.edges:
        nodes.update((edge.first, edge.second))
    def failed(status, unresolved=(), iterations=0, residual=None):
        return DCSolution(status, MappingProxyType({}), MappingProxyType({}),
                          MappingProxyType({}), None, residual, iterations, tuple(sorted(unresolved)))
    if len(nodes) > maximum_nodes:
        return failed('node_budget_exhausted', nodes)
    adjacent = {n: [] for n in nodes}
    for edge in network.edges:
        adjacent[edge.first].append(edge.second)
        adjacent[edge.second].append(edge.first)
    reached, pending = set(network.fixed_voltages_v), list(network.fixed_voltages_v)
    while pending:
        for neighbor in adjacent[pending.pop()]:
            if neighbor not in reached:
                reached.add(neighbor)
                pending.append(neighbor)
    if not nodes or nodes - reached:
        return failed('missing_voltage_boundary', nodes - reached)
    unknown = sorted(nodes - network.fixed_voltages_v.keys())
    indices = {n: i for i, n in enumerate(unknown)}
    diagonal, rows = [0.] * len(unknown), [[] for _ in unknown]
    rhs = [-float(network.withdrawals_a.get(n, 0)) for n in unknown]
    for edge in network.edges:
        conductance = 1 / float(edge.resistance_ohms)
        for first, second in ((edge.first, edge.second), (edge.second, edge.first)):
            if first not in indices:
                continue
            i = indices[first]
            diagonal[i] += conductance
            if second in indices:
                rows[i].append((indices[second], -conductance))
            else:
                rhs[i] += conductance * float(network.fixed_voltages_v[second])
    def multiply(values):
        return [diagonal[i] * values[i] + fsum(g * values[j] for j, g in row)
                for i, row in enumerate(rows)]
    def dot(a, b):
        return fsum(x*y for x, y in zip(a, b))
    x, residual = [0.] * len(unknown), rhs[:]
    z = [r / d for r, d in zip(residual, diagonal)]
    direction, rz, iterations = z[:], dot(residual, z), 0
    while max(map(abs, residual), default=0.) > tolerance_a and iterations < maximum_iterations:
        product = multiply(direction)
        denominator = dot(direction, product)
        if not isfinite(denominator) or denominator <= 0:
            return failed('numerical_failure', unknown, iterations)
        alpha = rz / denominator
        x = [a + alpha*b for a, b in zip(x, direction)]
        residual = [a - alpha*b for a, b in zip(residual, product)]
        iterations += 1
        if max(map(abs, residual), default=0.) <= tolerance_a:
            break
        z = [r / d for r, d in zip(residual, diagonal)]
        next_rz = dot(residual, z)
        direction = [a + next_rz/rz*b for a, b in zip(z, direction)]
        rz = next_rz
    actual = [b-a for b, a in zip(rhs, multiply(x))]
    error = max(map(abs, actual), default=0.)
    if not all(isfinite(v) for v in (*x, *actual)) or error > tolerance_a:
        return failed('iteration_budget_exhausted' if iterations == maximum_iterations else 'numerical_failure',
                      unknown, iterations, error)
    voltages = {n: float(v) for n, v in network.fixed_voltages_v.items()}
    voltages.update(zip(unknown, x))
    currents = {e.id: (voltages[e.first]-voltages[e.second])/float(e.resistance_ohms) for e in network.edges}
    sources = {n: float(network.withdrawals_a.get(n, 0)) for n in network.fixed_voltages_v}
    for e in network.edges:
        if e.first in sources:
            sources[e.first] += currents[e.id]
        if e.second in sources:
            sources[e.second] -= currents[e.id]
    loss = fsum(currents[e.id]**2 * float(e.resistance_ohms) for e in network.edges)
    return DCSolution('solved', MappingProxyType(voltages), MappingProxyType(currents),
                      MappingProxyType(sources), loss, error, iterations)


def strip_resistance(*, length_nm: int, width_nm: int, thickness_nm: int,
                     resistivity_ohm_m: Fraction) -> Fraction:
    """Uniform rectangular conductor at the caller's stated material temperature.

    Exact geometry conversion; not valid for arbitrary branched/filled regions.
    Vias must be provided as explicit model resistors, never zero-cost edges.
    """
    if any(type(v) is not int or v <= 0 for v in (length_nm, width_nm, thickness_nm)):
        raise ValueError('conductor dimensions require positive integer nanometers')
    rho = _exact(resistivity_ohm_m, 'resistivity')
    if rho <= 0:
        raise ValueError('resistivity must be positive')
    return rho * length_nm * 1_000_000_000 / (width_nm * thickness_nm)
