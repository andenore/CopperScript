"""Derive a flat electrical view from authoritative hierarchical IR."""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Mapping

from .model import (
    Board,
    ComponentInstance,
    Constraint,
    DeviceDefinition,
    ElaboratedModuleInstance,
    Endpoint,
    FlatElectricalView,
    Interface,
    ModuleDefinition,
    Net,
    PartDefinition,
    PeripheralSelection,
    PinType,
    Supply,
)


@dataclass(frozen=True, slots=True)
class _Template:
    name: str
    ports: Mapping[str, PinType]
    body: FlatElectricalView


def elaborate(board: Board) -> FlatElectricalView:
    """Return a deterministic global view without mutating hierarchical IR."""

    result = FlatElectricalView(
        name=board.name,
        library=board.library,
        components=board.components,
        nets=board.nets,
        supplies=board.supplies,
        interfaces=board.interfaces,
        constraints=board.constraints,
        devices=board.devices,
        peripheral_selections=board.peripheral_selections,
    )
    cache: dict[str, _Template] = {}
    for instance in board.module_instances:
        template = _template_for(instance.module, board.module_definitions, cache, ())
        result = _expand_one(result, instance.ref, template)
    return result


def _template_for(
    name: str,
    definitions: Mapping[str, ModuleDefinition],
    cache: dict[str, _Template],
    stack: tuple[str, ...],
) -> _Template:
    if name in cache:
        return cache[name]
    if name in stack:
        raise ValueError(f"cyclic module instantiation: {' -> '.join((*stack, name))}")
    definition = definitions[name]
    body = FlatElectricalView(
        name=definition.name,
        library=definition.library,
        components=definition.components,
        nets=definition.nets,
        supplies=definition.supplies,
        interfaces=definition.interfaces,
        constraints=definition.constraints,
        devices=definition.devices,
        peripheral_selections=definition.peripheral_selections,
    )
    for instance in definition.module_instances:
        child = _template_for(instance.module, definitions, cache, (*stack, name))
        body = _expand_one(body, instance.ref, child)
    template = _Template(definition.name, definition.ports, body)
    cache[name] = template
    return template


def _expand_one(
    parent: FlatElectricalView, instance_ref: str, template: _Template
) -> FlatElectricalView:
    net_entries: list[tuple[str, list[Endpoint]]] = [
        (net.name, list(net.endpoints)) for net in parent.nets
    ]
    external_connections: dict[str, str] = {}
    for net_name, endpoints in net_entries:
        retained: list[Endpoint] = []
        for endpoint in endpoints:
            if endpoint.component == instance_ref:
                external_connections[endpoint.pin] = net_name
            else:
                retained.append(endpoint)
        endpoints[:] = retained

    child_net_map: dict[str, str] = {}
    port_connections: dict[str, str] = {}
    for child_net in template.body.nets:
        boundary_ports = [
            endpoint.pin for endpoint in child_net.endpoints if endpoint.component == "port"
        ]
        mapped_names = {
            external_connections[port_name]
            for port_name in boundary_ports
            if port_name in external_connections
        }
        if len(mapped_names) > 1:
            raise ValueError(
                f"ports on {template.name}.{child_net.name} map to different parent nets"
            )
        mapped_name = next(iter(mapped_names)) if mapped_names else _qualify(instance_ref, child_net.name)
        child_net_map[child_net.name] = mapped_name
        for port_name in boundary_ports:
            port_connections[port_name] = mapped_name

        internal_endpoints = [
            _qualify_endpoint(instance_ref, endpoint)
            for endpoint in child_net.endpoints
            if endpoint.component != "port"
        ]
        _append_to_net(net_entries, mapped_name, internal_endpoints)

    library = dict(parent.library)
    _merge_library(library, template.body.library)
    devices = dict(parent.devices)
    _merge_devices(devices, template.body.devices)

    components = list(parent.components)
    components.extend(
        replace(component, ref=_qualify(instance_ref, component.ref))
        for component in template.body.components
    )

    parent_supplies: list[Supply] = []
    for supply in parent.supplies:
        if supply.source is not None and supply.source.component == instance_ref:
            parent_supplies.append(replace(supply, source=None, externally_driven=True))
        else:
            parent_supplies.append(supply)

    supplies = parent_supplies
    for supply in template.body.supplies:
        source = supply.source
        externally_driven = supply.externally_driven
        if source is not None and source.component == "port":
            source = None
            externally_driven = True
        elif source is not None:
            source = _qualify_endpoint(instance_ref, source)
        supplies.append(
            Supply(
                name=_qualify(instance_ref, supply.name),
                voltage=supply.voltage,
                net=child_net_map.get(supply.net, _qualify(instance_ref, supply.net)),
                source=source,
                externally_driven=externally_driven,
            )
        )

    interfaces = list(parent.interfaces)
    for interface in template.body.interfaces:
        interfaces.append(
            Interface(
                name=_qualify(instance_ref, interface.name),
                kind=interface.kind,
                signals={
                    signal: child_net_map.get(net, _qualify(instance_ref, net))
                    for signal, net in interface.signals.items()
                },
                bindings={
                    _qualify(instance_ref, component): dict(bindings)
                    for component, bindings in interface.bindings.items()
                },
                pullup_supply=(
                    _qualify(instance_ref, interface.pullup_supply)
                    if interface.pullup_supply
                    else None
                ),
            )
        )

    constraints = list(parent.constraints)
    constraints.extend(
        Constraint(
            constraint.kind,
            tuple(
                _qualify_target(instance_ref, target, template.body, child_net_map)
                for target in constraint.targets
            ),
            constraint.parameters,
        )
        for constraint in template.body.constraints
    )

    peripheral_selections = list(parent.peripheral_selections)
    peripheral_selections.extend(
        PeripheralSelection(
            component=_qualify(instance_ref, selection.component),
            peripheral=selection.peripheral,
            name=_qualify(instance_ref, selection.name),
            signals=selection.signals,
        )
        for selection in template.body.peripheral_selections
    )

    module_instances = list(parent.module_instances)
    module_instances.append(
        ElaboratedModuleInstance(instance_ref, template.name, template.ports, port_connections)
    )
    module_instances.extend(
        ElaboratedModuleInstance(
            path=_qualify(instance_ref, nested.path),
            module=nested.module,
            ports=nested.ports,
            connections={
                port: child_net_map.get(net, _qualify(instance_ref, net))
                for port, net in nested.connections.items()
            },
        )
        for nested in template.body.module_instances
    )

    return FlatElectricalView(
        name=parent.name,
        library=library,
        components=tuple(components),
        nets=tuple(Net(name, tuple(endpoints)) for name, endpoints in net_entries),
        supplies=tuple(supplies),
        interfaces=tuple(interfaces),
        constraints=tuple(constraints),
        module_instances=tuple(module_instances),
        devices=devices,
        peripheral_selections=tuple(peripheral_selections),
    )


def _append_to_net(
    entries: list[tuple[str, list[Endpoint]]], name: str, endpoints: list[Endpoint]
) -> None:
    for existing_name, existing_endpoints in entries:
        if existing_name == name:
            existing_endpoints.extend(endpoints)
            return
    entries.append((name, list(endpoints)))


def _merge_library(
    destination: dict[str, PartDefinition], incoming: Mapping[str, PartDefinition]
) -> None:
    for part_name, part in incoming.items():
        existing = destination.get(part_name)
        if existing is not None and existing != part:
            raise ValueError(f"conflicting definitions for part {part_name!r}")
        destination.setdefault(part_name, part)


def _merge_devices(
    destination: dict[str, DeviceDefinition], incoming: Mapping[str, DeviceDefinition]
) -> None:
    for device_name, device in incoming.items():
        existing = destination.get(device_name)
        if existing is not None and existing != device:
            raise ValueError(f"conflicting definitions for device {device_name!r}")
        destination.setdefault(device_name, device)


def _qualify(prefix: str, name: str) -> str:
    return f"{prefix}/{name}"


def _qualify_endpoint(prefix: str, endpoint: Endpoint) -> Endpoint:
    return Endpoint(_qualify(prefix, endpoint.component), endpoint.pin)


def _qualify_target(
    prefix: str,
    target: str,
    body: FlatElectricalView,
    child_net_map: dict[str, str],
) -> str:
    if target in child_net_map:
        return child_net_map[target]
    if "." in target:
        owner, pin = target.split(".", 1)
        return f"{_qualify(prefix, owner)}.{pin}"
    named_objects = {
        *(component.ref for component in body.components),
        *(supply.name for supply in body.supplies),
        *(interface.name for interface in body.interfaces),
    }
    return _qualify(prefix, target) if target in named_objects else target
