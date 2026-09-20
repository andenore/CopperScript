"""Electrical-rules checks for the v0.1 semantic IR."""

from __future__ import annotations

from collections import Counter, defaultdict
from dataclasses import dataclass
from enum import Enum
from typing import Callable, Iterable, TypeVar

from .elaborate import elaborate
from .model import (
    Board,
    ComponentInstance,
    Endpoint,
    FlatElectricalView,
    Interface,
    InterfaceKind,
    Net,
    PartDefinition,
    PartKind,
    PinDefinition,
    PinType,
    PeripheralSelection,
    Supply,
)


class Severity(str, Enum):
    ERROR = "error"
    WARNING = "warning"


@dataclass(frozen=True, slots=True)
class Diagnostic:
    severity: Severity
    code: str
    message: str
    subject: str | None = None

    def __str__(self) -> str:
        location = f" [{self.subject}]" if self.subject else ""
        return f"{self.severity.value.upper()} {self.code}{location}: {self.message}"


@dataclass(slots=True)
class _Context:
    board: FlatElectricalView
    components: dict[str, ComponentInstance]
    nets: dict[str, Net]
    supplies: dict[str, Supply]
    pin_to_nets: dict[Endpoint, list[str]]

    def resolve_pin(
        self, endpoint: Endpoint
    ) -> tuple[ComponentInstance, PartDefinition, PinDefinition] | None:
        component = self.components.get(endpoint.component)
        if component is None:
            return None
        part = self.board.library.get(component.part)
        if part is None:
            return None
        pin = part.pins.get(endpoint.pin)
        if pin is None:
            return None
        return component, part, pin


def check(board: Board | FlatElectricalView) -> list[Diagnostic]:
    """Run ERC on a derived flat view while preserving hierarchical input."""

    diagnostics: list[Diagnostic] = []
    if isinstance(board, Board):
        board = elaborate(board)
    components = _first_by(board.components, lambda item: item.ref)
    nets = _first_by(board.nets, lambda item: item.name)
    supplies = _first_by(board.supplies, lambda item: item.name)
    pin_to_nets: dict[Endpoint, list[str]] = defaultdict(list)
    for net in board.nets:
        for endpoint in net.endpoints:
            pin_to_nets[endpoint].append(net.name)

    context = _Context(board, components, nets, supplies, pin_to_nets)

    diagnostics.extend(_check_duplicates(board))
    diagnostics.extend(_check_references(context))
    diagnostics.extend(_check_pin_membership(context))
    diagnostics.extend(_check_output_conflicts(context))
    diagnostics.extend(_check_supplies(context))
    diagnostics.extend(_check_power_inputs(context))
    diagnostics.extend(_check_interfaces(context))
    diagnostics.extend(_check_peripheral_selections(context))
    return diagnostics


def has_errors(diagnostics: Iterable[Diagnostic]) -> bool:
    return any(item.severity is Severity.ERROR for item in diagnostics)


T = TypeVar("T")


def _first_by(items: Iterable[T], key: Callable[[T], str]) -> dict[str, T]:
    result: dict[str, T] = {}
    for item in items:
        result.setdefault(key(item), item)
    return result


def _duplicates(values: Iterable[str]) -> list[str]:
    return sorted(value for value, count in Counter(values).items() if count > 1)


def _check_duplicates(board: Board | FlatElectricalView) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    groups = (
        ("DUPLICATE_COMPONENT", "component reference", (c.ref for c in board.components)),
        ("DUPLICATE_NET", "net name", (n.name for n in board.nets)),
        ("DUPLICATE_SUPPLY", "supply name", (s.name for s in board.supplies)),
        ("DUPLICATE_INTERFACE", "interface name", (i.name for i in board.interfaces)),
        (
            "DUPLICATE_PERIPHERAL_CONFIGURATION",
            "peripheral configuration name",
            (selection.name for selection in board.peripheral_selections),
        ),
    )
    for code, label, values in groups:
        for value in _duplicates(values):
            diagnostics.append(
                Diagnostic(Severity.ERROR, code, f"duplicate {label} {value!r}", value)
            )
    return diagnostics


def _check_references(context: _Context) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    for component in context.board.components:
        if component.part not in context.board.library:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "UNKNOWN_PART",
                    f"component {component.ref} uses unknown part {component.part!r}",
                    component.ref,
                )
            )
            continue
        part = context.board.library[component.part]
        if part.device is not None and part.device not in context.board.devices:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "UNKNOWN_DEVICE",
                    f"part {part.name} references unknown device {part.device!r}",
                    component.ref,
                )
            )

    for net in context.board.nets:
        for endpoint in net.endpoints:
            component = context.components.get(endpoint.component)
            if component is None:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "UNKNOWN_COMPONENT",
                        f"net {net.name} references unknown component {endpoint.component!r}",
                        str(endpoint),
                    )
                )
                continue
            part = context.board.library.get(component.part)
            if part is not None and endpoint.pin not in part.pins:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "UNKNOWN_PIN",
                        f"part {part.name} has no pin {endpoint.pin!r}",
                        str(endpoint),
                    )
                )

    for supply in context.board.supplies:
        if supply.net not in context.nets:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "UNKNOWN_NET",
                    f"supply {supply.name} references unknown net {supply.net!r}",
                    supply.name,
                )
            )
        if supply.source is not None:
            resolved_source = context.resolve_pin(supply.source)
            if resolved_source is None:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "UNKNOWN_SUPPLY_SOURCE",
                        f"supply source {supply.source} does not resolve to a known pin",
                        supply.name,
                    )
                )
            elif resolved_source[2].pin_type is not PinType.POWER_OUT:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "SUPPLY_SOURCE_NOT_OUTPUT",
                        f"supply source {supply.source} is {resolved_source[2].pin_type.value}, not power_out",
                        supply.name,
                    )
                )
            elif supply.net in context.nets and supply.source not in context.nets[supply.net].endpoints:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "SUPPLY_SOURCE_NOT_ON_NET",
                        f"source {supply.source} is not connected to net {supply.net}",
                        supply.name,
                    )
                )
    return diagnostics


def _check_pin_membership(context: _Context) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    for endpoint, net_names in sorted(context.pin_to_nets.items(), key=lambda item: str(item[0])):
        distinct_names = sorted(set(net_names))
        if len(distinct_names) > 1:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "PIN_ON_MULTIPLE_NETS",
                    f"pin is connected to multiple nets: {', '.join(distinct_names)}",
                    str(endpoint),
                )
            )
    return diagnostics


def _check_output_conflicts(context: _Context) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    driving_types = {PinType.OUTPUT, PinType.POWER_OUT}
    for net in context.board.nets:
        drivers: list[str] = []
        for endpoint in net.endpoints:
            resolved = context.resolve_pin(endpoint)
            if resolved is not None and resolved[2].pin_type in driving_types:
                drivers.append(str(endpoint))
        if len(drivers) > 1:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "OUTPUT_CONFLICT",
                    f"multiple push-pull outputs drive this net: {', '.join(drivers)}",
                    net.name,
                )
            )
    return diagnostics


def _check_supplies(context: _Context) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    for supply in context.board.supplies:
        net = context.nets.get(supply.net)
        if net is None:
            continue
        for endpoint in net.endpoints:
            resolved = context.resolve_pin(endpoint)
            if resolved is None:
                continue
            pin = resolved[2]
            if pin.voltage_min is not None and supply.voltage < pin.voltage_min:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "SUPPLY_VOLTAGE_LOW",
                        f"{supply.voltage} is below the pin minimum {pin.voltage_min}",
                        str(endpoint),
                    )
                )
            if pin.voltage_max is not None and pin.voltage_max < supply.voltage:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "SUPPLY_VOLTAGE_HIGH",
                        f"{supply.voltage} exceeds the pin maximum {pin.voltage_max}",
                        str(endpoint),
                    )
                )
    return diagnostics


def _check_power_inputs(context: _Context) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    powered_nets: set[str] = {
        supply.net
        for supply in context.board.supplies
        if supply.externally_driven or supply.source is not None
    }
    for net in context.board.nets:
        if any(
            resolved is not None and resolved[2].pin_type is PinType.POWER_OUT
            for endpoint in net.endpoints
            if (resolved := context.resolve_pin(endpoint)) is not None
        ):
            powered_nets.add(net.name)

    for component in context.board.components:
        part = context.board.library.get(component.part)
        if part is None:
            continue
        for pin in part.pins.values():
            if pin.pin_type is not PinType.POWER_IN:
                continue
            endpoint = Endpoint(component.ref, pin.name)
            connected_nets = context.pin_to_nets.get(endpoint, [])
            if not connected_nets:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "UNSOURCED_POWER_INPUT",
                        "power input is not connected to a net",
                        str(endpoint),
                    )
                )
            elif not any(name in powered_nets for name in connected_nets):
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "UNSOURCED_POWER_INPUT",
                        f"power input has no power source on net {connected_nets[0]}",
                        str(endpoint),
                    )
                )
    return diagnostics


def _check_interfaces(context: _Context) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    for interface in context.board.interfaces:
        if interface.kind is InterfaceKind.I2C:
            diagnostics.extend(_check_i2c(context, interface))
    return diagnostics


def _check_i2c(context: _Context, interface: Interface) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    required_signals = ("sda", "scl")
    for signal in required_signals:
        if signal not in interface.signals:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "I2C_MISSING_SIGNAL",
                    f"I2C interface has no {signal.upper()} net mapping",
                    interface.name,
                )
            )
        elif interface.signals[signal] not in context.nets:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "UNKNOWN_NET",
                    f"I2C {signal.upper()} references unknown net {interface.signals[signal]!r}",
                    interface.name,
                )
            )

    if len(interface.bindings) < 2:
        diagnostics.append(
            Diagnostic(
                Severity.ERROR,
                "I2C_TOO_FEW_PARTICIPANTS",
                "I2C interface must bind at least two components",
                interface.name,
            )
        )

    allowed_types = {PinType.OPEN_DRAIN, PinType.BIDIRECTIONAL}
    for component_ref, bindings in interface.bindings.items():
        if component_ref not in context.components:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "UNKNOWN_COMPONENT",
                    f"I2C interface references unknown component {component_ref!r}",
                    interface.name,
                )
            )
            continue
        for signal in required_signals:
            pin_name = bindings.get(signal)
            if pin_name is None:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "I2C_MISSING_BINDING",
                        f"component {component_ref} has no {signal.upper()} pin binding",
                        interface.name,
                    )
                )
                continue
            endpoint = Endpoint(component_ref, pin_name)
            resolved = context.resolve_pin(endpoint)
            if resolved is None:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "I2C_UNKNOWN_PIN",
                        f"I2C binding {endpoint} does not resolve to a known pin",
                        interface.name,
                    )
                )
                continue
            net_name = interface.signals.get(signal)
            if net_name in context.nets and endpoint not in context.nets[net_name].endpoints:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "I2C_BINDING_NOT_ON_NET",
                        f"{endpoint} is not connected to mapped {signal.upper()} net {net_name}",
                        interface.name,
                    )
                )
            if resolved[2].pin_type not in allowed_types:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "I2C_INCOMPATIBLE_PIN",
                        f"{endpoint} is {resolved[2].pin_type.value}, expected open_drain or bidirectional",
                        interface.name,
                    )
                )

    pullup_supply = context.supplies.get(interface.pullup_supply or "")
    if interface.pullup_supply is None:
        diagnostics.append(
            Diagnostic(
                Severity.ERROR,
                "I2C_NO_PULLUP_SUPPLY",
                "I2C interface does not name a pull-up supply",
                interface.name,
            )
        )
    elif pullup_supply is None:
        diagnostics.append(
            Diagnostic(
                Severity.ERROR,
                "I2C_UNKNOWN_PULLUP_SUPPLY",
                f"unknown pull-up supply {interface.pullup_supply!r}",
                interface.name,
            )
        )
    else:
        for signal in required_signals:
            signal_net = interface.signals.get(signal)
            if signal_net in context.nets and not _has_resistor_between(
                context, signal_net, pullup_supply.net
            ):
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "I2C_MISSING_PULLUP",
                        f"{signal.upper()} net {signal_net} has no resistor to {pullup_supply.net}",
                        interface.name,
                    )
                )
    return diagnostics


def _check_peripheral_selections(context: _Context) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    pin_uses: dict[tuple[str, str], list[str]] = defaultdict(list)
    peripheral_uses: dict[tuple[str, str], list[str]] = defaultdict(list)
    resource_settings: dict[tuple[str, str], dict[str, list[str]]] = defaultdict(
        lambda: defaultdict(list)
    )

    for selection in context.board.peripheral_selections:
        component = context.components.get(selection.component)
        if component is None:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "UNKNOWN_CONFIGURATION_COMPONENT",
                    f"configuration references unknown component {selection.component!r}",
                    selection.name,
                )
            )
            continue
        part = context.board.library.get(component.part)
        if part is None:
            continue
        if part.device is None:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "PART_HAS_NO_DEVICE",
                    f"part {part.name} has no device capability definition",
                    selection.name,
                )
            )
            continue
        device = context.board.devices.get(part.device)
        if device is None:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "UNKNOWN_DEVICE",
                    f"part {part.name} references unknown device {part.device!r}",
                    selection.name,
                )
            )
            continue
        peripheral = device.peripherals.get(selection.peripheral)
        if peripheral is None:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "UNKNOWN_PERIPHERAL",
                    f"device {device.name} has no peripheral {selection.peripheral!r}",
                    selection.name,
                )
            )
            continue

        peripheral_uses[(selection.component, selection.peripheral)].append(selection.name)
        missing = sorted(
            signal.name
            for signal in peripheral.signals.values()
            if signal.required and signal.name not in selection.signals
        )
        for signal_name in missing:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "MISSING_PERIPHERAL_SIGNAL",
                    f"required signal {selection.peripheral}.{signal_name} is not configured",
                    selection.name,
                )
            )

        for signal_name, chosen in selection.signals.items():
            signal = peripheral.signals.get(signal_name)
            if signal is None:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "UNKNOWN_PERIPHERAL_SIGNAL",
                        f"peripheral {selection.peripheral} has no signal {signal_name!r}",
                        selection.name,
                    )
                )
                continue
            physical_pin = part.pins.get(chosen.pin)
            if physical_pin is None:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "UNKNOWN_MUX_PIN",
                        f"part {part.name} has no physical pin {chosen.pin!r}",
                        selection.name,
                    )
                )
                continue
            options = [
                option
                for option in device.mux_options
                if option.pin == chosen.pin
                and option.peripheral == selection.peripheral
                and option.signal == signal_name
            ]
            if len(options) != 1:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "INVALID_MUX_OPTION",
                        f"{chosen.pin} cannot carry {selection.peripheral}.{signal_name}",
                        selection.name,
                    )
                )
                continue
            option = options[0]
            if (
                chosen.selector != option.selector
                or chosen.resource != option.resource
                or chosen.setting != option.setting
            ):
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "MUX_SELECTOR_MISMATCH",
                        f"resolved mux metadata for {chosen.pin} does not match device definition",
                        selection.name,
                    )
                )
            if not _pin_supports_signal(physical_pin.pin_type, signal.pin_type):
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "MUX_INCOMPATIBLE_PIN",
                        f"pin {chosen.pin} is {physical_pin.pin_type.value}, incompatible with "
                        f"{signal.pin_type.value} signal {selection.peripheral}.{signal_name}",
                        selection.name,
                    )
                )
            pin_uses[(selection.component, chosen.pin)].append(
                f"{selection.name}.{signal_name}"
            )
            if option.resource is not None and option.setting is not None:
                resource_settings[(selection.component, option.resource)][option.setting].append(
                    f"{selection.name}.{signal_name}"
                )

    for (component, peripheral), uses in sorted(peripheral_uses.items()):
        if len(uses) > 1:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "PERIPHERAL_CONFLICT",
                    f"{component}.{peripheral} is configured more than once: {', '.join(uses)}",
                    f"{component}.{peripheral}",
                )
            )
    for (component, pin), uses in sorted(pin_uses.items()):
        if len(uses) > 1:
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "PIN_MUX_CONFLICT",
                    f"physical pin is selected by multiple signals: {', '.join(uses)}",
                    f"{component}.{pin}",
                )
            )
    for (component, resource), settings in sorted(resource_settings.items()):
        if len(settings) > 1:
            details = ", ".join(
                f"{setting} ({', '.join(uses)})" for setting, uses in sorted(settings.items())
            )
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "MUX_RESOURCE_CONFLICT",
                    f"resource requires incompatible settings: {details}",
                    f"{component}.{resource}",
                )
            )
    return diagnostics


def _pin_supports_signal(pin_type: PinType, signal_type: PinType) -> bool:
    compatible = {
        PinType.PASSIVE: {PinType.PASSIVE, PinType.BIDIRECTIONAL},
        PinType.INPUT: {PinType.INPUT, PinType.BIDIRECTIONAL},
        PinType.OUTPUT: {PinType.OUTPUT, PinType.BIDIRECTIONAL},
        PinType.BIDIRECTIONAL: {PinType.BIDIRECTIONAL},
        PinType.OPEN_DRAIN: {PinType.OPEN_DRAIN, PinType.BIDIRECTIONAL},
        PinType.POWER_IN: {PinType.POWER_IN},
        PinType.POWER_OUT: {PinType.POWER_OUT},
    }
    return pin_type in compatible[signal_type]


def _has_resistor_between(context: _Context, net_a: str, net_b: str) -> bool:
    endpoints_a = context.nets[net_a].endpoints
    endpoints_b = context.nets[net_b].endpoints
    components_a = {endpoint.component for endpoint in endpoints_a}
    components_b = {endpoint.component for endpoint in endpoints_b}
    for component_ref in components_a & components_b:
        component = context.components.get(component_ref)
        if component is None:
            continue
        part = context.board.library.get(component.part)
        if part is not None and part.kind is PartKind.RESISTOR:
            pins_on_a = {ep.pin for ep in endpoints_a if ep.component == component_ref}
            pins_on_b = {ep.pin for ep in endpoints_b if ep.component == component_ref}
            if pins_on_a and pins_on_b and pins_on_a.isdisjoint(pins_on_b):
                return True
    return False
