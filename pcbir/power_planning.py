"""Supply-net physical membership and normalized, soft distribution heuristics.

No connectivity, copper, or electrical power-state inference is changed here.
Domain membership can overlap: one component can participate on several rails.
This is not a current, impedance, voltage-drop or return-path solver.
"""

from __future__ import annotations

from collections import Counter
from collections.abc import Callable, Iterable, Mapping

from .model import ComponentInstance, DeviceDefinition, Direction, FlatElectricalView, PartDefinition, SignalDomain
from .physical import PadReference, PhysicalBoard, PhysicalNet, PhysicalPowerDomain, Placement, Point


def lower_power_domains(
    flat: FlatElectricalView, physical_nets: Iterable[PhysicalNet],
    pin_number: Callable[[ComponentInstance, PartDefinition, Mapping[str, DeviceDefinition], str], str | None],
) -> tuple[PhysicalPowerDomain, ...]:
    """Infer rails from explicit supplies and active power-pin profiles.

    Zero-volt supplies and pins explicitly marked ground are not clustering
    domains. Distinct same-voltage nets stay distinct. Aliases on one net merge.
    The callback is the normal frontend's unit/terminal-to-package resolution.
    """
    components = {component.ref: component for component in flat.components}
    declared = {supply.net for supply in flat.supplies if supply.voltage.base_value != 0}
    zero_rails = {supply.net for supply in flat.supplies if supply.voltage.base_value == 0}
    sources = {}
    for supply in flat.supplies:
        if supply.source is not None:
            component = components.get(supply.source.component)
            if component is not None:
                part = flat.library[component.part]
                number = pin_number(component, part, flat.devices, supply.source.pin)
                if number is not None:
                    sources.setdefault(supply.net, set()).add(PadReference(component.ref, number))
    for net in flat.nets:
        for endpoint in net.endpoints:
            component = components.get(endpoint.component)
            if component is None:
                continue
            part = flat.library[component.part]
            number = pin_number(component, part, flat.devices, endpoint.pin)
            profiles = []
            device = flat.devices.get(part.device or "")
            for pin in part.pins.values():
                if pin.number != number:
                    continue
                if pin.profile is not None:
                    profiles.append(pin.profile)
                if device is not None:
                    for bond in pin.bonds:
                        pad = device.pads.get(bond.pad)
                        if (pad is not None and (bond.when is None or bond.when.matches(component.modes))
                                and (pad.when is None or pad.when.matches(component.modes))):
                            profiles.append(pad.profile)
            for profile in profiles:
                if SignalDomain.GROUND in profile.domains:
                    zero_rails.add(net.name)
                elif SignalDomain.POWER in profile.domains:
                    declared.add(net.name)
                    if Direction.OUTPUT in profile.directions and number is not None:
                        sources.setdefault(net.name, set()).add(PadReference(component.ref, number))
    return tuple(PhysicalPowerDomain(net.name, net.pads,
        tuple(sorted(sources.get(net.name, set()) & set(net.pads))))
        for net in physical_nets if net.name in declared - zero_rails)


def domain_terminal_points(board: PhysicalBoard, placements: Mapping[str, Placement],
                           domain: PhysicalPowerDomain) -> dict[PadReference, Point]:
    from .placement import transformed_pad_position
    return {pad: transformed_pad_position(board, placements[pad.component], pad.pad)
            for pad in domain.members if pad.component in placements}


def _weights(pads: Iterable[PadReference]) -> dict[PadReference, float]:
    """Each consumer has equal rail weight, regardless of supply-pin count."""
    pads = tuple(pads)
    counts = Counter(pad.component for pad in pads)
    return {pad: 1 / (len(counts) * counts[pad.component]) for pad in pads}


def _targets(points: Mapping[PadReference, Point], domain: PhysicalPowerDomain):
    sources = [(pad, points[pad]) for pad in domain.sources if pad in points]
    if sources:
        return sources
    if not points:
        return []
    weights = _weights(points)
    return [(None, Point(round(sum(p.x_nm * weights[pad] for pad, p in points.items())),
                         round(sum(p.y_nm * weights[pad] for pad, p in points.items()))))]


def power_domain_penalty(board: PhysicalBoard, placements: Mapping[str, Placement], weight: float = 0.25) -> int:
    """Sum per-rail mean L1 distribution distance, with bounded soft weight."""
    if weight == 0:
        return 0
    total = 0.0
    for domain in board.power_domains:
        points = domain_terminal_points(board, placements, domain)
        targets = _targets(points, domain)
        loads = {pad: p for pad, p in points.items() if pad not in domain.sources}
        weights = _weights(loads)
        if targets and loads:
            total += sum(weights[pad] * min(abs(p.x_nm - t.x_nm) + abs(p.y_nm - t.y_nm)
                             for _, t in targets) for pad, p in loads.items())
    return round(total * weight)


def power_domain_gradient(board: PhysicalBoard, placements: Mapping[str, Placement],
                          weight: float) -> dict[str, list[float]]:
    """L1 subgradient; source and load forces balance within each rail.

    Normalize per rail rather than per pin globally. Source-free rails use
    pairwise centroid attraction; their aggregate translation remains neutral.
    """
    gradients = {ref: [0.0, 0.0] for ref in placements}
    if weight == 0:
        return gradients
    for domain in board.power_domains:
        points = domain_terminal_points(board, placements, domain)
        targets = _targets(points, domain)
        loads = [(pad, p) for pad, p in points.items() if pad not in domain.sources]
        weights = _weights(pad for pad, _ in loads)
        if not targets or not loads:
            continue
        forces = []
        for pad, point in loads:
            source, target = min(targets, key=lambda item: (
                abs(point.x_nm - item[1].x_nm) + abs(point.y_nm - item[1].y_nm),
                (item[0].component, item[0].pad) if item[0] is not None else ("", "")))
            force = [weight * weights[pad] * ((a > b) - (a < b))
                     for a, b in ((point.x_nm, target.x_nm), (point.y_nm, target.y_nm))]
            forces.append(force)
            for axis in (0, 1):
                gradients[pad.component][axis] += force[axis]
                if source is not None:
                    gradients[source.component][axis] -= force[axis]
        if targets[0][0] is None:
            for pad, _ in loads:
                for axis in (0, 1):
                    gradients[pad.component][axis] -= sum(f[axis] for f in forces) * weights[pad]
    return gradients
