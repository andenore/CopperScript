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
    ConnectionPolicy,
    Direction,
    DriveMode,
    ElectricalProfile,
    Endpoint,
    FlatElectricalView,
    Interface,
    Net,
    PartDefinition,
    PackagePinDefinition,
    PeripheralSelection,
    SelectionUsage,
    Supply,
    SignalDomain,
)
from .quantities import Voltage


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
    ) -> tuple[ComponentInstance, PartDefinition, PackagePinDefinition] | None:
        component = self.components.get(endpoint.component)
        if component is None:
            return None
        part = self.board.library.get(component.part)
        if part is None:
            return None
        pin = part.pins.get(endpoint.pin)
        if pin is not None:
            return component, part, pin
        unit_name, separator, terminal_name = endpoint.pin.partition(".")
        device = self.board.devices.get(part.device or "")
        unit = device.units.get(unit_name) if device and separator else None
        terminal = unit.terminals.get(terminal_name) if unit else None
        if terminal is None:
            return None
        matches = [
            package_pin
            for package_pin in part.pins.values()
            if terminal.pad in self.active_bonded_pads(component, package_pin)
        ]
        return (component, part, matches[0]) if len(matches) == 1 else None

    def canonical_endpoint(self, endpoint: Endpoint) -> Endpoint | None:
        resolved = self.resolve_pin(endpoint)
        return Endpoint(endpoint.component, resolved[2].name) if resolved else None

    def active_bonded_pads(
        self, component: ComponentInstance, pin: PackagePinDefinition
    ) -> tuple[str, ...]:
        return tuple(
            bond.pad
            for bond in pin.bonds
            if bond.when is None or bond.when.matches(component.modes)
        )

    def pin_profile(
        self, component: ComponentInstance, part: PartDefinition, pin: PackagePinDefinition
    ) -> ElectricalProfile | None:
        profiles = [pin.profile] if pin.profile is not None else []
        if part.device is not None:
            device = self.board.devices.get(part.device)
            if device is not None:
                for pad_name in self.active_bonded_pads(component, pin):
                    pad = device.pads.get(pad_name)
                    if pad is not None and (pad.when is None or pad.when.matches(component.modes)):
                        profiles.append(pad.profile)
        if not profiles:
            return None
        voltage = next((profile.voltage for profile in profiles if profile.voltage), None)
        return ElectricalProfile(
            frozenset().union(*(profile.domains for profile in profiles)),
            frozenset().union(*(profile.directions for profile in profiles)),
            frozenset().union(*(profile.drive_modes for profile in profiles)),
            frozenset().union(*(profile.traits for profile in profiles)),
            voltage,
        )

    def pin_voltage_limits(
        self, component: ComponentInstance, part: PartDefinition, pin: PackagePinDefinition
    ) -> tuple[Voltage | None, Voltage | None]:
        profile = self.pin_profile(component, part, pin)
        voltage = profile.voltage if profile else None
        return (
            voltage.minimum if voltage else None,
            voltage.maximum if voltage else None,
        )


def check(board: Board | FlatElectricalView) -> list[Diagnostic]:
    """Run ERC on a derived flat view while preserving hierarchical input."""

    diagnostics: list[Diagnostic] = []
    if isinstance(board, Board):
        board = elaborate(board)
    components = _first_by(board.components, lambda item: item.ref)
    nets = _first_by(board.nets, lambda item: item.name)
    supplies = _first_by(board.supplies, lambda item: item.name)
    context = _Context(board, components, nets, supplies, defaultdict(list))
    for net in board.nets:
        for endpoint in net.endpoints:
            context.pin_to_nets[context.canonical_endpoint(endpoint) or endpoint].append(net.name)

    diagnostics.extend(_check_duplicates(board))
    diagnostics.extend(_check_references(context))
    diagnostics.extend(_check_pin_membership(context))
    diagnostics.extend(_check_output_conflicts(context))
    diagnostics.extend(_check_supplies(context))
    diagnostics.extend(_check_power_inputs(context))
    diagnostics.extend(_check_interfaces(context))
    diagnostics.extend(_check_peripheral_selections(context))
    diagnostics.extend(_check_package_rules(context))
    diagnostics.extend(_check_modes_and_groups(context))
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
        elif part.device is not None:
            device = context.board.devices[part.device]
            for pin in part.pins.values():
                for bond in pin.bonds:
                    pad_name = bond.pad
                    if pad_name not in device.pads:
                        diagnostics.append(
                            Diagnostic(
                                Severity.ERROR,
                                "UNKNOWN_DEVICE_PAD",
                                f"package pin {pin.name} bonds unknown device pad {pad_name!r}",
                                f"{component.ref}.{pin.name}",
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
            if part is not None and context.resolve_pin(endpoint) is None:
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
            elif not _is_power_output(
                context.pin_profile(resolved_source[0], resolved_source[1], resolved_source[2])
            ):
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "SUPPLY_SOURCE_NOT_OUTPUT",
                        f"supply source {supply.source} is not power_output capable",
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
    selected_outputs: set[Endpoint] = set()
    for selection in context.board.peripheral_selections:
        component = context.components.get(selection.component)
        part = context.board.library.get(component.part) if component else None
        device = context.board.devices.get(part.device or "") if part else None
        peripheral = device.peripherals.get(selection.peripheral) if device else None
        if peripheral is None:
            continue
        for signal_name, chosen in selection.signals.items():
            signal = peripheral.signals.get(signal_name)
            if signal is not None and Direction.OUTPUT in signal.profile.directions:
                selected_outputs.add(Endpoint(selection.component, chosen.pin))
    for net in context.board.nets:
        drivers: list[str] = []
        for endpoint in net.endpoints:
            resolved = context.resolve_pin(endpoint)
            if resolved is not None:
                profile = context.pin_profile(resolved[0], resolved[1], resolved[2])
                if (
                    _is_power_output(profile)
                    or _is_push_pull_output(profile)
                    or endpoint in selected_outputs
                ):
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
            voltage_min, voltage_max = context.pin_voltage_limits(resolved[0], resolved[1], pin)
            if voltage_min is not None and supply.voltage < voltage_min:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "SUPPLY_VOLTAGE_LOW",
                        f"{supply.voltage} is below the pin minimum {voltage_min}",
                        str(endpoint),
                    )
                )
            if voltage_max is not None and voltage_max < supply.voltage:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "SUPPLY_VOLTAGE_HIGH",
                        f"{supply.voltage} exceeds the pin maximum {voltage_max}",
                        str(endpoint),
                    )
                )
            component, part, pin = resolved
            device = context.board.devices.get(part.device or "")
            if device is None:
                continue
            for pad_name in context.active_bonded_pads(component, pin):
                for domain in device.power_domains.values():
                    if pad_name not in domain.supply_pads or domain.voltage is None:
                        continue
                    if domain.voltage.minimum is not None and supply.voltage < domain.voltage.minimum:
                        diagnostics.append(
                            Diagnostic(
                                Severity.ERROR,
                                "POWER_DOMAIN_VOLTAGE_LOW",
                                f"{supply.voltage} is below {domain.name} minimum {domain.voltage.minimum}",
                                str(endpoint),
                            )
                        )
                    if domain.voltage.maximum is not None and domain.voltage.maximum < supply.voltage:
                        diagnostics.append(
                            Diagnostic(
                                Severity.ERROR,
                                "POWER_DOMAIN_VOLTAGE_HIGH",
                                f"{supply.voltage} exceeds {domain.name} maximum {domain.voltage.maximum}",
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
            resolved is not None
            and _is_power_output(
                context.pin_profile(resolved[0], resolved[1], resolved[2])
            )
            for endpoint in net.endpoints
            if (resolved := context.resolve_pin(endpoint)) is not None
        ):
            powered_nets.add(net.name)

    for component in context.board.components:
        part = context.board.library.get(component.part)
        if part is None:
            continue
        for pin in part.pins.values():
            profile = context.pin_profile(component, part, pin)
            if not _is_power_input(profile):
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
        if interface.type_name == "std.i2c":
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
            profile = context.pin_profile(resolved[0], resolved[1], resolved[2])
            if not (
                profile
                and SignalDomain.DIGITAL in profile.domains
                and Direction.BIDIRECTIONAL in profile.directions
                and DriveMode.OPEN_DRAIN in profile.drive_modes
            ):
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "I2C_INCOMPATIBLE_PIN",
                        f"{endpoint} lacks digital_input and open_drain_output capabilities",
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
    pin_uses: dict[tuple[str, str], list[tuple[str, SelectionUsage]]] = defaultdict(list)
    peripheral_uses: dict[tuple[str, str], list[tuple[str, SelectionUsage]]] = defaultdict(list)
    resource_settings: dict[
        tuple[str, str], dict[str, list[tuple[str, SelectionUsage]]]
    ] = defaultdict(
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

        peripheral_uses[(selection.component, selection.peripheral)].append(
            (selection.name, selection.usage)
        )
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
            active_pads = context.active_bonded_pads(component, physical_pin)
            options = [
                option
                for option in device.mux_options
                if option.pad in active_pads
                and option.peripheral == selection.peripheral
                and option.signal == signal_name
                and (option.when is None or option.when.matches(component.modes))
            ]
            routes = [
                route
                for route in device.route_rules
                if route.peripheral == selection.peripheral
                and route.signal == signal_name
                and (route.when is None or route.when.matches(component.modes))
                and any(
                    pad in device.pad_sets[route.pad_set].pads for pad in active_pads
                )
            ]
            if len(options) + len(routes) != 1:
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "INVALID_MUX_OPTION",
                        f"{chosen.pin} cannot carry {selection.peripheral}.{signal_name}",
                        selection.name,
                    )
                )
                continue
            option = options[0] if options else None
            if option is not None and (
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
            profile = context.pin_profile(component, part, physical_pin)
            if profile is None or not profile.satisfies(signal.profile):
                diagnostics.append(
                    Diagnostic(
                        Severity.ERROR,
                        "MUX_INCOMPATIBLE_PIN",
                        f"pin {chosen.pin} electrical profile is incompatible with "
                        f"signal {selection.peripheral}.{signal_name}",
                        selection.name,
                    )
                )
            pin_uses[(selection.component, chosen.pin)].append(
                (f"{selection.name}.{signal_name}", selection.usage)
            )
            if option is not None and option.resource is not None and option.setting is not None:
                resource_settings[(selection.component, option.resource)][option.setting].append(
                    (f"{selection.name}.{signal_name}", selection.usage)
                )

    for (component, peripheral), uses in sorted(peripheral_uses.items()):
        if len(uses) > 1 and not _all_firmware_managed(uses):
            labels = [label for label, _usage in uses]
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "PERIPHERAL_CONFLICT",
                    f"{component}.{peripheral} is configured more than once: {', '.join(labels)}",
                    f"{component}.{peripheral}",
                )
            )
    for (component, pin), uses in sorted(pin_uses.items()):
        if len(uses) > 1 and not _all_firmware_managed(uses):
            labels = [label for label, _usage in uses]
            diagnostics.append(
                Diagnostic(
                    Severity.ERROR,
                    "PIN_MUX_CONFLICT",
                    f"physical pin is selected by multiple signals: {', '.join(labels)}",
                    f"{component}.{pin}",
                )
            )
    for (component, resource), settings in sorted(resource_settings.items()):
        uses = [use for setting_uses in settings.values() for use in setting_uses]
        if len(settings) > 1 and not _all_firmware_managed(uses):
            details = ", ".join(
                f"{setting} ({', '.join(label for label, _usage in setting_uses)})"
                for setting, setting_uses in sorted(settings.items())
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


def _all_firmware_managed(uses: list[tuple[str, SelectionUsage]]) -> bool:
    return all(usage is SelectionUsage.FIRMWARE_MANAGED for _label, usage in uses)


def _is_power_input(profile: ElectricalProfile | None) -> bool:
    return bool(
        profile
        and (SignalDomain.POWER in profile.domains or SignalDomain.GROUND in profile.domains)
        and Direction.INPUT in profile.directions
    )


def _is_power_output(profile: ElectricalProfile | None) -> bool:
    return bool(
        profile
        and SignalDomain.POWER in profile.domains
        and Direction.OUTPUT in profile.directions
    )


def _is_push_pull_output(profile: ElectricalProfile | None) -> bool:
    return bool(
        profile
        and Direction.OUTPUT in profile.directions
        and DriveMode.PUSH_PULL in profile.drive_modes
    )


def _check_package_rules(context: _Context) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    net_by_pin = context.pin_to_nets
    for component in context.board.components:
        part = context.board.library.get(component.part)
        if part is None:
            continue
        for pin in part.pins.values():
            endpoint = Endpoint(component.ref, pin.name)
            nets = net_by_pin.get(endpoint, [])
            if pin.connection_policy is ConnectionPolicy.DO_NOT_CONNECT and nets:
                diagnostics.append(
                    Diagnostic(Severity.ERROR, "DO_NOT_CONNECT", "do-not-connect pin is connected", str(endpoint))
                )
            if pin.connection_policy is ConnectionPolicy.REQUIRED and not nets:
                diagnostics.append(
                    Diagnostic(Severity.ERROR, "REQUIRED_PIN_UNCONNECTED", "required package pin is not connected", str(endpoint))
                )
            for net_name in nets:
                traits = _net_traits(context, net_name)
                missing = pin.required_net_traits - traits
                if missing:
                    diagnostics.append(
                        Diagnostic(
                            Severity.ERROR,
                            "REQUIRED_NET_TRAIT",
                            f"net lacks required traits: {', '.join(sorted(missing))}",
                            str(endpoint),
                        )
                    )
    return diagnostics


def _check_modes_and_groups(context: _Context) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    for component in context.board.components:
        part = context.board.library.get(component.part)
        device = context.board.devices.get(part.device or "") if part else None
        if device is None:
            continue
        effective_modes = {
            name: component.modes.get(name, group.default)
            for name, group in device.mode_groups.items()
        }
        for name in component.modes:
            if name not in device.mode_groups:
                diagnostics.append(Diagnostic(Severity.ERROR, "UNKNOWN_MODE_GROUP", f"unknown mode group {name!r}", component.ref))
        for name, choice in effective_modes.items():
            group = device.mode_groups[name]
            if choice is None:
                diagnostics.append(Diagnostic(Severity.ERROR, "MODE_NOT_SELECTED", f"mode group {name!r} has no selection", component.ref))
            elif choice not in group.choices:
                diagnostics.append(Diagnostic(Severity.ERROR, "UNKNOWN_MODE_CHOICE", f"{choice!r} is not a choice of {name}", component.ref))
        connected_pads = {
            pad
            for pin in part.pins.values()
            if context.pin_to_nets.get(Endpoint(component.ref, pin.name))
            for pad in context.active_bonded_pads(component, pin)
        }
        for group in device.signal_groups.values():
            if group.when is not None and not group.when.matches(effective_modes):
                continue
            if (group.kind == "differential_pair" or getattr(group.kind, "value", None) == "differential_pair"):
                present = {name for name, pad in group.members.items() if pad in connected_pads}
                if present and present != {"positive", "negative"}:
                    diagnostics.append(
                        Diagnostic(Severity.ERROR, "INCOMPLETE_DIFFERENTIAL_PAIR", f"differential group {group.name} must connect both polarities", component.ref)
                    )
    return diagnostics


def _net_traits(context: _Context, net_name: str) -> frozenset[str]:
    traits: set[str] = set()
    supply = next((item for item in context.board.supplies if item.net == net_name), None)
    if supply is not None:
        traits.add("ground" if supply.voltage.base_value == 0 else "power")
    return frozenset(traits)


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
        if part is not None and part.category == "passive.resistor":
            pins_on_a = {ep.pin for ep in endpoints_a if ep.component == component_ref}
            pins_on_b = {ep.pin for ep in endpoints_b if ep.component == component_ref}
            if pins_on_a and pins_on_b and pins_on_a.isdisjoint(pins_on_b):
                return True
    return False
