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
    PowerState,
    RelativeVoltage,
    SourceReference,
    Supply,
)
from .quantities import Quantity, decimal_text
from .design import Design
from .mechanical_profiles import mechanical_provenance


def board_to_dict(board: Board | Design) -> dict[str, object]:
    if isinstance(board, Design):
        result = board_to_dict(board.electrical)
        if board.mechanical is not None:
            m = board.mechanical
            point = lambda p: {"x_nm": p.x_nm, "y_nm": p.y_nm}
            circle = m.outline.circular_boundary
            from .mechanical_references import reference_scene
            result["mechanical"] = {
                "outline": {
                    "vertices": [point(p) for p in m.outline.vertices],
                    "circle": ({"center": point(circle.center), "radius_nm": circle.radius_nm,
                                "maximum_chord_error_nm": circle.maximum_chord_error_nm} if circle else None),
                    "cutouts": [{"id": c.id, "vertices": [point(p) for p in c.vertices]}
                                for c in m.outline.cutouts],
                },
                "holes": [{"id": h.id, "position": point(h.position), "diameter_nm": h.diameter_nm,
                           "head_clearance_radius_nm": h.head_clearance_radius_nm} for h in m.holes],
                "datums": [{"id": d.id, "position": point(d.position), "relative_to": d.relative_to, "offset": point(d.offset)} for d in m.datums],
                "boundary_edges": [{"id": e.id, "start": point(e.start), "end": point(e.end)} for e in m.boundary_edges],
                "attachments": [{"id": a.id, "reference": a.reference, "target": a.target, "position": point(a.position),
                                 "offset": point(a.offset), "anchor": a.anchor, "anchor_pad": a.anchor_pad,
                                 "anchor_point": point(a.anchor_point) if a.anchor_point else None,
                                 "rotation": str(a.rotation), "side": a.side.value} for a in m.attachments],
                "rule_overrides": dict(m.rule_overrides),
                "slots": [{"id":s.id,"start":point(s.start),"end":point(s.end),"width_nm":s.width_nm} for s in m.slots],
                "references": [reference_scene(r) for r in m.references],
                "boundary_path": {"maximum_chord_error_nm":m.outline.boundary_path.maximum_chord_error_nm,
                    "segments":[{"id":s.id,"kind":"arc" if hasattr(s,'mid') else "line","start":point(s.start),"end":point(s.end),
                        **({"mid":point(s.mid)} if hasattr(s,'mid') else {})} for s in m.outline.boundary_path.segments]} if m.outline.boundary_path else None,
                "body_overhangs": [{"id":a.id,"reference":a.reference,"edge":a.edge,"start_nm":a.start_nm,"end_nm":a.end_nm,"distance_nm":a.distance_nm,"reason":a.reason} for a in m.body_overhangs],
                "component_heights": [{"id":a.id,"reference":a.reference,"height_nm":a.height_nm} for a in m.component_heights],
                "assembly_envelopes": [{"id":a.id,"vertices":[point(p) for p in a.outline.vertices],"side":a.side.value,"maximum_height_nm":a.maximum_height_nm} for a in m.assembly_envelopes],
                "assembly_access": [{"id":a.id,"reference":a.reference,"vertices":[point(p) for p in a.outline.vertices],"side":a.side,"purpose":a.purpose} for a in m.assembly_access],
                "connectors": [{"role": c.role, "reference": c.reference, "anchor_pad": c.anchor_pad,
                                "position": point(c.position), "rotation": str(c.rotation),
                                "side": c.side.value, "footprint": c.footprint} for c in m.connectors],
                "keepouts": [{"name": k.name, "vertices": [point(p) for p in k.outline.vertices],
                              "side": k.side.value if k.side else None,
                              "maximum_height_nm": k.maximum_component_height_nm} for k in m.keepouts],
                "copper_keepouts": [{"id": k.id, "layers": [l.value for l in k.layers],
                                     "vertices": [point(p) for p in k.outline.outer.vertices],
                                     **{name: getattr(k, name) for name in ("block_tracks", "block_vias", "block_pads", "block_zones", "block_footprints")}}
                                    for k in m.copper_keepouts],
                "provenance": mechanical_provenance(m),
            }
        return result
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
            board.power_states,
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


def board_to_json(board: Board | Design, *, indent: int = 2) -> str:
    return json.dumps(board_to_dict(board), indent=indent, sort_keys=True) + "\n"


def write_json(board: Board | Design, path: str | Path) -> None:
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
            (),
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
    power_states: tuple[PowerState, ...],
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
                "modes": dict(component.modes),
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
                "type": interface.type_name,
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
                  "id": constraint.constraint_id,
                  "mode": constraint.mode.value,
                  "consumers": list(constraint.consumers),
                  "verifier": constraint.verifier,
                  "origins": list(constraint.origins),
              }
            for constraint in constraints
        ],
        "peripheral_selections": [
            {
                "name": selection.name,
                "component": selection.component,
                "peripheral": selection.peripheral,
                "usage": selection.usage.value,
                "signals": {
                    name: {
                        "pin": signal.pin,
                        "pad": signal.pad,
                        "selector": signal.selector,
                        "resource": signal.resource,
                        "setting": signal.setting,
                    }
                    for name, signal in selection.signals.items()
                },
            }
            for selection in peripheral_selections
        ],
        "power_states": [
            {
                "name": state.name,
                "rails": {name: value.value for name, value in state.rails.items()},
            }
            for state in power_states
        ],
    }


def _part_to_dict(part: PartDefinition) -> dict[str, object]:
    return {
        "name": part.name,
        "category": part.category,
        "traits": sorted(part.traits),
        "manufacturer": part.manufacturer,
        "assembled": part.assembled,
        "device": part.device,
        "source": _source_to_dict(part.source),
        "footprints": list(part.footprints),
        "internal_pad_groups": [list(group.numbers) for group in part.internal_pad_groups],
        "metadata": dict(part.metadata),
        "pins": [
            {
                "name": pin.name,
                "number": pin.number,
                "profile": _profile_to_dict(pin.profile),
                "bonds": [
                    {"pad": bond.pad, "when": _condition_to_dict(bond.when)}
                    for bond in pin.bonds
                ],
                "connection_policy": pin.connection_policy.value,
                "required_net_traits": sorted(pin.required_net_traits),
            }
            for pin in part.pins.values()
        ],
    }


def _device_to_dict(device: DeviceDefinition) -> dict[str, object]:
    return {
        "name": device.name,
        "metadata": dict(device.metadata),
        "source": _source_to_dict(device.source),
        "pads": [
            {
                "name": pad.name,
                "profile": _profile_to_dict(pad.profile),
                "power_domain": pad.power_domain,
                "unpowered_behavior": pad.unpowered_behavior.value,
                "when": _condition_to_dict(pad.when),
            }
            for pad in device.pads.values()
        ],
        "power_domains": [
            {
                "name": domain.name,
                "supply_pads": list(domain.supply_pads),
                "voltage": _range_to_dict(domain.voltage),
                "requires": list(domain.requires),
            }
            for domain in device.power_domains.values()
        ],
        "resources": list(device.resources),
        "peripherals": [
            {
                "name": peripheral.name,
                "kind": peripheral.kind,
                "signals": [
                    {
                        "name": signal.name,
                        "profile": _profile_to_dict(signal.profile),
                        "required": signal.required,
                    }
                    for signal in peripheral.signals.values()
                ],
            }
            for peripheral in device.peripherals.values()
        ],
        "mux_options": [
            {
                "pad": option.pad,
                "peripheral": option.peripheral,
                "signal": option.signal,
                "selector": option.selector,
                "resource": option.resource,
                "setting": option.setting,
                "when": _condition_to_dict(option.when),
            }
            for option in device.mux_options
        ],
        "units": [
            {
                "name": unit.name,
                "kind": unit.kind,
                "shared": unit.shared,
                "terminals": {
                    name: {"pad": terminal.pad, "profile": _profile_to_dict(terminal.profile)}
                    for name, terminal in unit.terminals.items()
                },
            }
            for unit in device.units.values()
        ],
        "signal_groups": [
            {
                "name": group.name,
                "kind": getattr(group.kind, "value", group.kind),
                "members": dict(group.members),
                "profile": _profile_to_dict(group.profile),
                "when": _condition_to_dict(group.when),
            }
            for group in device.signal_groups.values()
        ],
        "mode_groups": [
            {"name": group.name, "choices": list(group.choices), "default": group.default}
            for group in device.mode_groups.values()
        ],
        "pad_sets": [
            {"name": pad_set.name, "pads": list(pad_set.pads)}
            for pad_set in device.pad_sets.values()
        ],
        "route_rules": [
            {
                "peripheral": rule.peripheral,
                "signal": rule.signal,
                "pad_set": rule.pad_set,
                "selector": {
                    "kind": rule.selector.kind,
                    "parameters": dict(rule.selector.parameters),
                },
                "when": _condition_to_dict(rule.when),
            }
            for rule in device.route_rules
        ],
    }


def _profile_to_dict(profile) -> object:
    if profile is None:
        return None
    result = {
        "domains": sorted(item.value for item in profile.domains),
        "directions": sorted(item.value for item in profile.directions),
        "drive_modes": sorted(item.value for item in profile.drive_modes),
        "traits": sorted(profile.traits),
        "voltage": _range_to_dict(profile.voltage),
        "current": _range_to_dict(profile.current),
    }
    # Emitted only when declared so the electrical digest of designs without
    # absolute-maximum ratings is unchanged.
    if profile.absolute_voltage is not None:
        result["absolute_voltage"] = _range_to_dict(profile.absolute_voltage)
    return result


def _range_to_dict(value) -> object:
    if value is None:
        return None
    return {
        "minimum": _value(value.minimum),
        "typical": _value(value.typical),
        "maximum": _value(value.maximum),
        "rating": value.rating,
        "when": _condition_to_dict(value.when),
    }


def _condition_to_dict(condition) -> object:
    return dict(condition.selections) if condition is not None else None


def _source_to_dict(source: SourceReference | None) -> object:
    if source is None:
        return None
    return {
        "document": source.document,
        "revision": source.revision,
        "location": source.location,
        "url": source.url,
        "checksum": source.checksum,
    }


def _value(value: object) -> object:
    if isinstance(value, RelativeVoltage):
        return {
            "reference": value.reference,
            "offset": _value(value.offset),
            "limit": _value(value.limit),
        }
    if isinstance(value, Quantity):
        return {
            "value": decimal_text(value.value),
            "unit": value.display_unit,
            "base_value": decimal_text(value.base_value),
        }
    return value
