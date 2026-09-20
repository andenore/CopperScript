"""Discrete power-state analysis for powered and unpowered I/O domains."""

from __future__ import annotations

from collections import defaultdict
from typing import Mapping

from .elaborate import elaborate
from .erc import Diagnostic, Severity
from .model import (
    Board,
    ComponentInstance,
    DevicePadDefinition,
    Direction,
    Endpoint,
    FlatElectricalView,
    PartDefinition,
    PackagePinDefinition,
    PowerRailState,
    SignalDomain,
    UnpoweredBehavior,
)


def analyze_power_states(board: Board | FlatElectricalView) -> list[Diagnostic]:
    """Check explicit steady-state scenarios without simulating firmware."""

    if isinstance(board, Board):
        board = elaborate(board)
    diagnostics: list[Diagnostic] = []
    supplies = {supply.name: supply for supply in board.supplies}
    components = {component.ref: component for component in board.components}
    pin_nets: dict[Endpoint, list[str]] = defaultdict(list)
    for net in board.nets:
        for endpoint in net.endpoints:
            pin_nets[endpoint].append(net.name)

    seen_states: set[str] = set()
    for state in board.power_states:
        if state.name in seen_states:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "DUPLICATE_POWER_STATE",
                    f"duplicate power state {state.name!r}",
                    state.name,
                )
            )
            continue
        seen_states.add(state.name)
        for rail in state.rails:
            if rail not in supplies:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "UNKNOWN_POWER_STATE_RAIL",
                        f"power state references unknown supply {rail!r}",
                        state.name,
                    )
                )

        domain_states = _component_domain_states(board, state.rails, pin_nets)
        active_output_pins = _active_peripheral_outputs(board, domain_states)

        for net in board.nets:
            rail_is_on = any(
                state.rails.get(supply.name) is PowerRailState.ON
                for supply in board.supplies
                if supply.net == net.name
            )
            has_active_driver = rail_is_on
            for endpoint in net.endpoints:
                resolved = _resolve_pin(board, components, endpoint)
                if resolved is None:
                    continue
                part, pin = resolved
                if _is_power_output(board, components[endpoint.component], part, pin):
                    has_active_driver = True
                if endpoint in active_output_pins:
                    has_active_driver = True
            if not has_active_driver:
                continue

            for endpoint in net.endpoints:
                resolved = _resolve_pin(board, components, endpoint)
                if resolved is None:
                    continue
                part, pin = resolved
                if _is_power_input(board, components[endpoint.component], part, pin):
                    continue
                for pad in _bonded_pads(board, components[endpoint.component], part, pin):
                    if pad.power_domain is None:
                        continue
                    if domain_states.get((endpoint.component, pad.power_domain)) is not PowerRailState.OFF:
                        continue
                    if pad.unpowered_behavior in {
                        UnpoweredBehavior.TOLERANT,
                        UnpoweredBehavior.HIGH_IMPEDANCE,
                    }:
                        continue
                    diagnostics.append(
                        Diagnostic(
                            Severity.WARNING,
                            "POSSIBLE_BACKPOWER",
                            f"net {net.name} is driven while domain {pad.power_domain} is off; "
                            f"unpowered behavior is {pad.unpowered_behavior.value}",
                            f"{state.name}:{endpoint}",
                        )
                    )
    return diagnostics


def _component_domain_states(
    board: FlatElectricalView,
    rail_states: Mapping[str, PowerRailState],
    pin_nets: dict[Endpoint, list[str]],
) -> dict[tuple[str, str], PowerRailState]:
    result: dict[tuple[str, str], PowerRailState] = {}
    for component in board.components:
        part = board.library.get(component.part)
        device = board.devices.get(part.device or "") if part else None
        if part is None or device is None:
            continue
        for domain in device.power_domains.values():
            states: list[PowerRailState] = []
            for pin in part.pins.values():
                if not {bond.pad for bond in pin.bonds if bond.when is None or bond.when.matches(component.modes)} & set(domain.supply_pads):
                    continue
                endpoint = Endpoint(component.ref, pin.name)
                for net_name in pin_nets.get(endpoint, []):
                    for supply in board.supplies:
                        if supply.net == net_name:
                            states.append(rail_states.get(supply.name, PowerRailState.UNKNOWN))
            if states and all(value is PowerRailState.ON for value in states):
                result[(component.ref, domain.name)] = PowerRailState.ON
            elif states and all(value is PowerRailState.OFF for value in states):
                result[(component.ref, domain.name)] = PowerRailState.OFF
            else:
                result[(component.ref, domain.name)] = PowerRailState.UNKNOWN
    return result


def _active_peripheral_outputs(
    board: FlatElectricalView,
    domain_states: dict[tuple[str, str], PowerRailState],
) -> set[Endpoint]:
    result: set[Endpoint] = set()
    components = {component.ref: component for component in board.components}
    for selection in board.peripheral_selections:
        component = components.get(selection.component)
        part = board.library.get(component.part) if component else None
        device = board.devices.get(part.device or "") if part else None
        peripheral = device.peripherals.get(selection.peripheral) if device else None
        if part is None or peripheral is None:
            continue
        for signal_name, chosen in selection.signals.items():
            signal = peripheral.signals.get(signal_name)
            pin = part.pins.get(chosen.pin)
            if signal is None or pin is None:
                continue
            if Direction.OUTPUT not in signal.profile.directions:
                continue
            pads = _bonded_pads(board, component, part, pin)
            if any(
                pad.power_domain is not None
                and domain_states.get((component.ref, pad.power_domain)) is PowerRailState.OFF
                for pad in pads
            ):
                continue
            result.add(Endpoint(component.ref, pin.name))
    return result


def _resolve_pin(
    board: FlatElectricalView,
    components: dict[str, ComponentInstance],
    endpoint: Endpoint,
) -> tuple[PartDefinition, PackagePinDefinition] | None:
    component = components.get(endpoint.component)
    part = board.library.get(component.part) if component else None
    pin = part.pins.get(endpoint.pin) if part else None
    return (part, pin) if part is not None and pin is not None else None


def _bonded_pads(
    board: FlatElectricalView,
    component: ComponentInstance,
    part: PartDefinition,
    pin: PackagePinDefinition,
) -> tuple[DevicePadDefinition, ...]:
    device = board.devices.get(part.device or "")
    if device is None:
        return ()
    return tuple(
        device.pads[bond.pad]
        for bond in pin.bonds
        if bond.pad in device.pads
        and (bond.when is None or bond.when.matches(component.modes))
    )


def _profiles(board, component, part, pin):
    result = [pin.profile] if pin.profile else []
    result.extend(pad.profile for pad in _bonded_pads(board, component, part, pin))
    return result


def _is_power_input(board, component, part, pin) -> bool:
    return any(
        (SignalDomain.POWER in profile.domains or SignalDomain.GROUND in profile.domains)
        and Direction.INPUT in profile.directions
        for profile in _profiles(board, component, part, pin)
    )


def _is_power_output(board, component, part, pin) -> bool:
    return any(
        SignalDomain.POWER in profile.domains and Direction.OUTPUT in profile.directions
        for profile in _profiles(board, component, part, pin)
    )
