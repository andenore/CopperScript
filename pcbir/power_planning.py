"""Supply-net physical membership and normalized, soft distribution heuristics.

No connectivity, copper, or electrical power-state inference is changed here.
Domain membership can overlap: one component can participate on several rails.
This is not a current, impedance, voltage-drop or return-path solver.
"""

from __future__ import annotations

from collections import Counter
from collections.abc import Callable, Iterable, Mapping

from .model import ComponentInstance, DeviceDefinition, Direction, FlatElectricalView, PartDefinition, PinType, SignalDomain
from .physical import PadReference, PhysicalBoard, PhysicalNet, PhysicalPowerDomain, Placement, Point


def lower_power_feeds(flat, physical_nets, pin_number):
    """Orient explicit passive paths by distance from a declared real source.

    Equal-distance bridges are not guessed directions. An external supply can
    establish upstream reachability, but does not invent a physical source pad.
    """
    from .physical import PowerFeedBoundary
    domains = lower_power_domains(flat, physical_nets, pin_number)
    distance = {d.net: 0 for d in domains if d.sources}
    distance.update({s.net: 0 for s in flat.supplies if s.externally_driven or s.source is not None})
    memberships = {}
    for n in physical_nets:
        for p in n.pads:
            memberships.setdefault(p, set()).add(n.name)
    membership = {p: next(iter(names)) for p, names in memberships.items() if len(names) == 1}
    links = []
    for component in flat.components:
        part = flat.library[component.part]
        if not part.assembled or not part.dc_power_path:
            continue
        pads = tuple(PadReference(component.ref, number) for name in part.dc_power_path
                     if (number := pin_number(component, part, flat.devices, name)) is not None)
        if len(pads) == 2 and all(p in membership for p in pads):
            a, b = pads
            if membership[a] != membership[b]:
                links.append((membership[a], a, membership[b], b))
    # Bounded breadth expansion, without turning cycles into sources.
    for depth in range(len(links)):
        additions = {}
        for a, _, b, _ in links:
            if distance.get(a) == depth and b not in distance:
                additions[b] = depth + 1
            if distance.get(b) == depth and a not in distance:
                additions[a] = depth + 1
        if not additions:
            break
        distance.update(additions)
    feeds = []
    for a, ap, b, bp in links:
        if a not in distance or b not in distance or distance[a] == distance[b]:
            continue
        if distance[a] > distance[b]:
            a, ap, b, bp = b, bp, a, ap
        feeds.append(PowerFeedBoundary(b, bp, a, ap))
    return tuple(sorted(feeds, key=lambda f: (f.net, f.terminal, f.upstream_net)))


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
    physical_nets = tuple(physical_nets)
    declared = {supply.net for supply in flat.supplies if supply.voltage.base_value != 0}
    zero_rails = {supply.net for supply in flat.supplies if supply.voltage.base_value == 0}
    sources = {}
    source_ports = {}
    # Elaboration represents a module-sourced supply as external to its flat
    # components. Its explicit power_out port still identifies a physical
    # boundary. Keep every internal terminal of that boundary, not an arbitrary
    # capacitor/inductor chosen from a net name. Connectivity remains unproven.
    for instance in flat.module_instances:
        for port, net_name in instance.connections.items():
            if instance.ports.get(port) is not PinType.POWER_OUT:
                continue
            terminals = {p for net in physical_nets if net.name == net_name for p in net.pads
                         if p.component.startswith(instance.path + '/')}
            if terminals:
                declared.add(net_name)
                sources.setdefault(net_name, set()).update(terminals)
                source_ports.setdefault(net_name, set()).add(f'{instance.path}.{port}')
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
                # A compatible-domain set is not the pin's assigned function.
                # General test points/connectors accepting ground AND signals
                # must not erase a regulator rail or manufacture a supply.
                if profile.domains == frozenset({SignalDomain.GROUND}):
                    zero_rails.add(net.name)
                elif profile.domains == frozenset({SignalDomain.POWER}):
                    declared.add(net.name)
                    if Direction.OUTPUT in profile.directions and number is not None:
                        sources.setdefault(net.name, set()).add(PadReference(component.ref, number))
    return tuple(PhysicalPowerDomain(net.name, net.pads,
        tuple(sorted(sources.get(net.name, set()) & set(net.pads))), tuple(sorted(source_ports.get(net.name, ()))))
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
