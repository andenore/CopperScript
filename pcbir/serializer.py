"""Stable JSON serialization of authoritative hierarchical CopperScript IR."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Mapping

from .model import (
    Board,
    ComponentInstance,
    Constraint,
    Dependency,
    DeviceDefinition,
    Interface,
    ModuleDefinition,
    ModuleInstance,
    Net,
    PartDefinition,
    PeripheralSelection,
    Supply,
)
from .quantities import Quantity


def board_to_dict(board: Board) -> dict[str, object]:
    result = {
        "schema": "copperscript-ir/v0.1",
        "kind": "board",
        "name": board.name,
        **_unit_fields(
            board.library,
            board.components,
            board.module_instances,
            board.nets,
            board.supplies,
            board.interfaces,
            board.constraints,
            board.devices,
            board.peripheral_selections,
        ),
        "module_definitions": [
            _module_to_dict(definition)
            for definition in sorted(
                board.module_definitions.values(), key=lambda item: item.name
            )
        ],
        "dependencies": [
            {
                "import_path": dependency.import_path,
                "module_path": dependency.module_path,
                "version": dependency.version,
                "checksum": dependency.checksum,
            }
            for dependency in sorted(board.dependencies, key=lambda item: item.import_path)
        ],
    }
    return result


def board_to_json(board: Board, *, indent: int = 2) -> str:
    return json.dumps(board_to_dict(board), indent=indent, sort_keys=True) + "\n"


def write_json(board: Board, path: str | Path) -> None:
    Path(path).write_text(board_to_json(board), encoding="utf-8")


def _module_to_dict(definition: ModuleDefinition) -> dict[str, object]:
    return {
        "kind": "module",
        "name": definition.name,
        "ports": {
            name: pin_type.value for name, pin_type in definition.ports.items()
        },
        **_unit_fields(
            definition.library,
            definition.components,
            definition.module_instances,
            definition.nets,
            definition.supplies,
            definition.interfaces,
            definition.constraints,
            definition.devices,
            definition.peripheral_selections,
        ),
    }


def _unit_fields(
    library: Mapping[str, PartDefinition],
    components: tuple[ComponentInstance, ...],
    module_instances: tuple[ModuleInstance, ...],
    nets: tuple[Net, ...],
    supplies: tuple[Supply, ...],
    interfaces: tuple[Interface, ...],
    constraints: tuple[Constraint, ...],
    devices: Mapping[str, DeviceDefinition],
    peripheral_selections: tuple[PeripheralSelection, ...],
) -> dict[str, object]:
    return {
        "devices": [
            _device_to_dict(device)
            for device in sorted(devices.values(), key=lambda item: item.name)
        ],
        "parts": [_part_to_dict(part) for part in sorted(library.values(), key=lambda item: item.name)],
        "components": [
            {
                "ref": component.ref,
                "part": component.part,
                "value": _value(component.value),
                "footprint": component.footprint,
                "properties": dict(component.properties),
            }
            for component in components
        ],
        "module_instances": [
            {"ref": instance.ref, "module": instance.module}
            for instance in module_instances
        ],
        "nets": [
            {"name": net.name, "endpoints": [str(endpoint) for endpoint in net.endpoints]}
            for net in nets
        ],
        "supplies": [
            {
                "name": supply.name,
                "voltage": _value(supply.voltage),
                "net": supply.net,
                "source": str(supply.source) if supply.source else None,
                "externally_driven": supply.externally_driven,
            }
            for supply in supplies
        ],
        "interfaces": [
            {
                "name": interface.name,
                "kind": interface.kind.value,
                "signals": dict(interface.signals),
                "bindings": {
                    component: dict(bindings)
                    for component, bindings in interface.bindings.items()
                },
                "pullup_supply": interface.pullup_supply,
            }
            for interface in interfaces
        ],
        "constraints": [
            {
                "kind": constraint.kind.value,
                "targets": list(constraint.targets),
                "parameters": {
                    name: _value(value) for name, value in constraint.parameters.items()
                },
            }
            for constraint in constraints
        ],
        "peripheral_selections": [
            {
                "name": selection.name,
                "component": selection.component,
                "peripheral": selection.peripheral,
                "signals": {
                    name: {
                        "pin": signal.pin,
                        "selector": signal.selector,
                        "resource": signal.resource,
                        "setting": signal.setting,
                    }
                    for name, signal in selection.signals.items()
                },
            }
            for selection in peripheral_selections
        ],
    }


def _part_to_dict(part: PartDefinition) -> dict[str, object]:
    return {
        "name": part.name,
        "kind": part.kind.value,
        "manufacturer": part.manufacturer,
        "device": part.device,
        "footprints": list(part.footprints),
        "metadata": dict(part.metadata),
        "pins": [
            {
                "name": pin.name,
                "number": pin.number,
                "type": pin.pin_type.value,
                "voltage_min": _value(pin.voltage_min),
                "voltage_max": _value(pin.voltage_max),
            }
            for pin in part.pins.values()
        ],
    }


def _device_to_dict(device: DeviceDefinition) -> dict[str, object]:
    return {
        "name": device.name,
        "metadata": dict(device.metadata),
        "resources": list(device.resources),
        "peripherals": [
            {
                "name": peripheral.name,
                "kind": peripheral.kind,
                "signals": [
                    {
                        "name": signal.name,
                        "type": signal.pin_type.value,
                        "required": signal.required,
                    }
                    for signal in peripheral.signals.values()
                ],
            }
            for peripheral in device.peripherals.values()
        ],
        "mux_options": [
            {
                "pin": option.pin,
                "peripheral": option.peripheral,
                "signal": option.signal,
                "selector": option.selector,
                "resource": option.resource,
                "setting": option.setting,
            }
            for option in device.mux_options
        ],
    }


def _value(value: object) -> object:
    if isinstance(value, Quantity):
        return {
            "value": str(value.value.normalize()),
            "unit": value.display_unit,
            "base_value": str(value.base_value.normalize()),
        }
    return value
