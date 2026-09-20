"""Typed semantic intermediate representation for CopperScript v0.1.

The model contains data only. It deliberately does not parse source text,
perform ERC, or encode presentation geometry. ``Board`` is the authoritative
hierarchical IR; ``FlatElectricalView`` is derived for consumers such as ERC.
``Board.constraints`` remains transitional until constraints move into their
own set as required by the design specification.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from types import MappingProxyType
from typing import Mapping

from .quantities import Quantity, Voltage


class PinType(str, Enum):
    """Electrical direction of ports and peripheral signals."""

    PASSIVE = "passive"
    INPUT = "input"
    OUTPUT = "output"
    BIDIRECTIONAL = "bidirectional"
    OPEN_DRAIN = "open_drain"
    POWER_IN = "power_in"
    POWER_OUT = "power_out"


class PinCapability(str, Enum):
    PASSIVE = "passive"
    DIGITAL_INPUT = "digital_input"
    PUSH_PULL_OUTPUT = "push_pull_output"
    OPEN_DRAIN_OUTPUT = "open_drain_output"
    ANALOG = "analog"
    POWER_INPUT = "power_input"
    POWER_OUTPUT = "power_output"


class UnpoweredBehavior(str, Enum):
    UNKNOWN = "unknown"
    HIGH_IMPEDANCE = "high_impedance"
    CLAMPED = "clamped"
    TOLERANT = "tolerant"


class SelectionUsage(str, Enum):
    EXCLUSIVE = "exclusive"
    FIRMWARE_MANAGED = "firmware_managed"


class PowerRailState(str, Enum):
    ON = "on"
    OFF = "off"
    UNKNOWN = "unknown"


class PartKind(str, Enum):
    GENERIC = "generic"
    RESISTOR = "resistor"
    CAPACITOR = "capacitor"
    POWER_SOURCE = "power_source"
    REGULATOR = "regulator"
    MCU = "mcu"
    SENSOR = "sensor"


class InterfaceKind(str, Enum):
    I2C = "i2c"


class ConstraintKind(str, Enum):
    """Known constraints; physical constraints are stored but not checked yet."""

    MAX_DISTANCE = "max_distance"
    PLACEMENT_REGION = "placement_region"
    NOTE = "note"


@dataclass(frozen=True, slots=True)
class SourceReference:
    document: str | None = None
    revision: str | None = None
    location: str | None = None
    url: str | None = None
    checksum: str | None = None


@dataclass(frozen=True, slots=True)
class DevicePadDefinition:
    name: str
    capabilities: frozenset[PinCapability]
    role: str = "io"
    power_domain: str | None = None
    unpowered_behavior: UnpoweredBehavior = UnpoweredBehavior.UNKNOWN
    voltage_min: Voltage | None = None
    voltage_max: Voltage | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "capabilities", frozenset(self.capabilities))


@dataclass(frozen=True, slots=True)
class PinDefinition:
    """One physical package pin, optionally bonded to device pads."""

    name: str
    number: str
    capabilities: frozenset[PinCapability] = frozenset()
    role: str = "io"
    bonded_pads: tuple[str, ...] = ()
    voltage_min: Voltage | None = None
    voltage_max: Voltage | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "capabilities", frozenset(self.capabilities))


@dataclass(frozen=True, slots=True)
class PowerDomainDefinition:
    name: str
    supply_pads: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class PeripheralSignalDefinition:
    name: str
    pin_type: PinType
    required: bool = True


@dataclass(frozen=True, slots=True)
class PeripheralDefinition:
    name: str
    kind: str
    signals: Mapping[str, PeripheralSignalDefinition]

    def __post_init__(self) -> None:
        object.__setattr__(self, "signals", MappingProxyType(dict(self.signals)))


@dataclass(frozen=True, slots=True)
class MuxOption:
    pad: str
    peripheral: str
    signal: str
    selector: str
    resource: str | None = None
    setting: str | None = None


@dataclass(frozen=True, slots=True)
class DeviceDefinition:
    """Package-independent silicon capabilities and pin-mux choices."""

    name: str
    pads: Mapping[str, DevicePadDefinition]
    peripherals: Mapping[str, PeripheralDefinition]
    mux_options: tuple[MuxOption, ...]
    power_domains: Mapping[str, PowerDomainDefinition] = field(default_factory=dict)
    resources: tuple[str, ...] = ()
    metadata: Mapping[str, str] = field(default_factory=dict)
    source: SourceReference | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "pads", MappingProxyType(dict(self.pads)))
        object.__setattr__(self, "peripherals", MappingProxyType(dict(self.peripherals)))
        object.__setattr__(
            self, "power_domains", MappingProxyType(dict(self.power_domains))
        )
        object.__setattr__(self, "metadata", MappingProxyType(dict(self.metadata)))


@dataclass(frozen=True, slots=True)
class PartDefinition:
    name: str
    pins: Mapping[str, PinDefinition]
    kind: PartKind = PartKind.GENERIC
    footprints: tuple[str, ...] = ()
    manufacturer: str | None = None
    device: str | None = None
    source: SourceReference | None = None
    metadata: Mapping[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        # Defensive copies keep an otherwise frozen IR from being mutated via a
        # caller-owned dictionary.
        object.__setattr__(self, "pins", MappingProxyType(dict(self.pins)))
        object.__setattr__(self, "metadata", MappingProxyType(dict(self.metadata)))


@dataclass(frozen=True, slots=True)
class ComponentInstance:
    ref: str
    part: str
    value: Quantity | str | None = None
    footprint: str | None = None
    properties: Mapping[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        object.__setattr__(self, "properties", MappingProxyType(dict(self.properties)))


@dataclass(frozen=True, slots=True, order=True)
class Endpoint:
    component: str
    pin: str

    @classmethod
    def parse(cls, text: str) -> "Endpoint":
        component, separator, pin = text.partition(".")
        if not separator or not component or not pin:
            raise ValueError(f"endpoint must look like COMPONENT.PIN, got {text!r}")
        return cls(component, pin)

    def __str__(self) -> str:
        return f"{self.component}.{self.pin}"


def ep(text: str) -> Endpoint:
    """Concise helper for board authoring: ``ep('U1.VDD')``."""

    return Endpoint.parse(text)


@dataclass(frozen=True, slots=True)
class Net:
    name: str
    endpoints: tuple[Endpoint, ...]


@dataclass(frozen=True, slots=True)
class Supply:
    name: str
    voltage: Voltage
    net: str
    source: Endpoint | None = None
    externally_driven: bool = False


@dataclass(frozen=True, slots=True)
class Interface:
    name: str
    kind: InterfaceKind
    signals: Mapping[str, str]
    bindings: Mapping[str, Mapping[str, str]]
    pullup_supply: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "signals", MappingProxyType(dict(self.signals)))
        copied_bindings = {
            component: MappingProxyType(dict(signal_pins))
            for component, signal_pins in self.bindings.items()
        }
        object.__setattr__(self, "bindings", MappingProxyType(copied_bindings))


@dataclass(frozen=True, slots=True)
class Constraint:
    kind: ConstraintKind
    targets: tuple[str, ...]
    parameters: Mapping[str, Quantity | str | int | float] = field(default_factory=dict)

    def __post_init__(self) -> None:
        object.__setattr__(self, "parameters", MappingProxyType(dict(self.parameters)))


@dataclass(frozen=True, slots=True)
class ModuleInstance:
    """A source-level instance of a reusable module definition."""

    ref: str
    module: str


@dataclass(frozen=True, slots=True)
class PeripheralSignalSelection:
    pin: str
    selector: str | None = None
    resource: str | None = None
    setting: str | None = None


@dataclass(frozen=True, slots=True)
class PeripheralSelection:
    """Concrete pin-mux selection for one component peripheral instance."""

    component: str
    peripheral: str
    name: str
    signals: Mapping[str, PeripheralSignalSelection]
    usage: SelectionUsage = SelectionUsage.EXCLUSIVE

    def __post_init__(self) -> None:
        object.__setattr__(self, "signals", MappingProxyType(dict(self.signals)))


@dataclass(frozen=True, slots=True)
class PowerState:
    name: str
    rails: Mapping[str, PowerRailState]

    def __post_init__(self) -> None:
        object.__setattr__(self, "rails", MappingProxyType(dict(self.rails)))


@dataclass(frozen=True, slots=True)
class Dependency:
    """Resolved source-package provenance retained by the authoritative IR."""

    import_path: str
    module_path: str
    version: str
    checksum: str


@dataclass(frozen=True, slots=True)
class ModuleDefinition:
    """A reusable hierarchical electrical design with typed boundary ports."""

    name: str
    ports: Mapping[str, PinType]
    library: Mapping[str, PartDefinition]
    components: tuple[ComponentInstance, ...]
    module_instances: tuple[ModuleInstance, ...]
    nets: tuple[Net, ...]
    supplies: tuple[Supply, ...] = ()
    interfaces: tuple[Interface, ...] = ()
    constraints: tuple[Constraint, ...] = ()
    devices: Mapping[str, DeviceDefinition] = field(default_factory=dict)
    peripheral_selections: tuple[PeripheralSelection, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "ports", MappingProxyType(dict(self.ports)))
        object.__setattr__(self, "library", MappingProxyType(dict(self.library)))
        object.__setattr__(self, "devices", MappingProxyType(dict(self.devices)))


@dataclass(frozen=True, slots=True)
class ElaboratedModuleInstance:
    """Module provenance retained in a derived flat electrical view."""

    path: str
    module: str
    ports: Mapping[str, PinType]
    connections: Mapping[str, str]

    def __post_init__(self) -> None:
        object.__setattr__(self, "ports", MappingProxyType(dict(self.ports)))
        object.__setattr__(self, "connections", MappingProxyType(dict(self.connections)))


@dataclass(frozen=True, slots=True)
class Board:
    """Authoritative hierarchical electrical design IR."""

    name: str
    library: Mapping[str, PartDefinition]
    components: tuple[ComponentInstance, ...]
    nets: tuple[Net, ...]
    supplies: tuple[Supply, ...] = ()
    interfaces: tuple[Interface, ...] = ()
    constraints: tuple[Constraint, ...] = ()
    module_instances: tuple[ModuleInstance, ...] = ()
    module_definitions: Mapping[str, ModuleDefinition] = field(default_factory=dict)
    dependencies: tuple[Dependency, ...] = ()
    devices: Mapping[str, DeviceDefinition] = field(default_factory=dict)
    peripheral_selections: tuple[PeripheralSelection, ...] = ()
    power_states: tuple[PowerState, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "library", MappingProxyType(dict(self.library)))
        object.__setattr__(
            self, "module_definitions", MappingProxyType(dict(self.module_definitions))
        )
        object.__setattr__(self, "devices", MappingProxyType(dict(self.devices)))


@dataclass(frozen=True, slots=True)
class FlatElectricalView:
    """Derived, non-authoritative global view used by ERC and some backends."""

    name: str
    library: Mapping[str, PartDefinition]
    components: tuple[ComponentInstance, ...]
    nets: tuple[Net, ...]
    supplies: tuple[Supply, ...] = ()
    interfaces: tuple[Interface, ...] = ()
    constraints: tuple[Constraint, ...] = ()
    module_instances: tuple[ElaboratedModuleInstance, ...] = ()
    devices: Mapping[str, DeviceDefinition] = field(default_factory=dict)
    peripheral_selections: tuple[PeripheralSelection, ...] = ()
    power_states: tuple[PowerState, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "library", MappingProxyType(dict(self.library)))
        object.__setattr__(self, "devices", MappingProxyType(dict(self.devices)))
