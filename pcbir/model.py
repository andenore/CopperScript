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
from typing import Generic, Mapping, TypeVar

from .constraint_coverage import ConstraintMode
from .quantities import Current, Quantity, Voltage


class PinType(str, Enum):
    """Boundary-port shorthand. Device pins use :class:`ElectricalProfile`."""

    PASSIVE = "passive"
    INPUT = "input"
    OUTPUT = "output"
    BIDIRECTIONAL = "bidirectional"
    OPEN_DRAIN = "open_drain"
    POWER_IN = "power_in"
    POWER_OUT = "power_out"


class SignalDomain(str, Enum):
    DIGITAL = "digital"
    ANALOG = "analog"
    POWER = "power"
    GROUND = "ground"
    CLOCK = "clock"
    RF = "rf"


class Direction(str, Enum):
    INPUT = "input"
    OUTPUT = "output"
    BIDIRECTIONAL = "bidirectional"
    PASSIVE = "passive"


class DriveMode(str, Enum):
    PUSH_PULL = "push_pull"
    OPEN_DRAIN = "open_drain"
    HIGH_IMPEDANCE = "high_impedance"


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


class ConnectionPolicy(str, Enum):
    NORMAL = "normal"
    REQUIRED = "required"
    DO_NOT_CONNECT = "do_not_connect"
    OPTIONAL = "optional"


class GroupKind(str, Enum):
    DIFFERENTIAL_PAIR = "differential_pair"


Q = TypeVar("Q", bound=Quantity)


@dataclass(frozen=True, slots=True)
class QuantityRange(Generic[Q]):
    minimum: Q | None = None
    typical: Q | None = None
    maximum: Q | None = None
    rating: str = "operating"
    when: "Condition | None" = None


@dataclass(frozen=True, slots=True)
class ElectricalProfile:
    domains: frozenset[SignalDomain]
    directions: frozenset[Direction]
    drive_modes: frozenset[DriveMode] = frozenset()
    traits: frozenset[str] = frozenset()
    voltage: QuantityRange[Voltage] | None = None
    current: QuantityRange[Current] | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "domains", frozenset(self.domains))
        object.__setattr__(self, "directions", frozenset(self.directions))
        object.__setattr__(self, "drive_modes", frozenset(self.drive_modes))
        object.__setattr__(self, "traits", frozenset(self.traits))

    def satisfies(self, required: "ElectricalProfile") -> bool:
        provided_directions = set(self.directions)
        if Direction.BIDIRECTIONAL in provided_directions:
            provided_directions.update({Direction.INPUT, Direction.OUTPUT})
        return (
            required.domains <= self.domains
            and required.directions <= provided_directions
            and required.drive_modes <= self.drive_modes
            and required.traits <= self.traits
        )


@dataclass(frozen=True, slots=True)
class Condition:
    selections: Mapping[str, str]

    def __post_init__(self) -> None:
        object.__setattr__(self, "selections", MappingProxyType(dict(self.selections)))

    def matches(self, selected: Mapping[str, str]) -> bool:
        return all(selected.get(group) == choice for group, choice in self.selections.items())


class ConstraintKind(str, Enum):
    """Known semantic constraints retained for dedicated compiler passes."""

    MAX_DISTANCE = "max_distance"
    MIN_DISTANCE = "min_distance"
    PLACEMENT_REGION = "placement_region"
    FIXED_PLACEMENT = "fixed_placement"
    ALLOWED_ORIENTATIONS = "allowed_orientations"
    ALIGN = "align"
    PLACEMENT_GROUP = "placement_group"
    KEEPOUT = "keepout"
    NOTE = "note"
    ROUTING = "routing"


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
    profile: ElectricalProfile
    power_domain: str | None = None
    unpowered_behavior: UnpoweredBehavior = UnpoweredBehavior.UNKNOWN
    when: Condition | None = None


@dataclass(frozen=True, slots=True)
class BondDefinition:
    pad: str
    when: Condition | None = None


@dataclass(frozen=True, slots=True)
class PackagePinDefinition:
    """One physical package pin with explicit bonds and connection policy."""

    name: str
    number: str
    profile: ElectricalProfile | None = None
    bonds: tuple[BondDefinition, ...] = ()
    connection_policy: ConnectionPolicy = ConnectionPolicy.NORMAL
    required_net_traits: frozenset[str] = frozenset()

    def __post_init__(self) -> None:
        object.__setattr__(self, "bonds", tuple(self.bonds))
        object.__setattr__(self, "required_net_traits", frozenset(self.required_net_traits))


@dataclass(frozen=True, slots=True)
class PowerDomainDefinition:
    name: str
    supply_pads: tuple[str, ...]
    voltage: QuantityRange[Voltage] | None = None
    requires: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class PeripheralSignalDefinition:
    name: str
    profile: ElectricalProfile
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
    when: Condition | None = None


@dataclass(frozen=True, slots=True)
class TerminalBinding:
    pad: str
    profile: ElectricalProfile | None = None


@dataclass(frozen=True, slots=True)
class FunctionalUnitDefinition:
    name: str
    kind: str
    terminals: Mapping[str, TerminalBinding]
    shared: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "terminals", MappingProxyType(dict(self.terminals)))


@dataclass(frozen=True, slots=True)
class SignalGroupDefinition:
    name: str
    kind: GroupKind | str
    members: Mapping[str, str]
    profile: ElectricalProfile | None = None
    when: Condition | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "members", MappingProxyType(dict(self.members)))


@dataclass(frozen=True, slots=True)
class ModeGroupDefinition:
    name: str
    choices: tuple[str, ...]
    default: str | None = None


@dataclass(frozen=True, slots=True)
class PadSetDefinition:
    name: str
    pads: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class SelectorScheme:
    kind: str
    parameters: Mapping[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        object.__setattr__(self, "parameters", MappingProxyType(dict(self.parameters)))


@dataclass(frozen=True, slots=True)
class RouteRule:
    peripheral: str
    signal: str
    pad_set: str
    selector: SelectorScheme
    when: Condition | None = None


@dataclass(frozen=True, slots=True)
class DeviceDefinition:
    """Package-independent silicon capabilities and pin-mux choices."""

    name: str
    pads: Mapping[str, DevicePadDefinition]
    peripherals: Mapping[str, PeripheralDefinition]
    mux_options: tuple[MuxOption, ...]
    power_domains: Mapping[str, PowerDomainDefinition] = field(default_factory=dict)
    units: Mapping[str, FunctionalUnitDefinition] = field(default_factory=dict)
    signal_groups: Mapping[str, SignalGroupDefinition] = field(default_factory=dict)
    mode_groups: Mapping[str, ModeGroupDefinition] = field(default_factory=dict)
    pad_sets: Mapping[str, PadSetDefinition] = field(default_factory=dict)
    route_rules: tuple[RouteRule, ...] = ()
    resources: tuple[str, ...] = ()
    metadata: Mapping[str, str] = field(default_factory=dict)
    source: SourceReference | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "pads", MappingProxyType(dict(self.pads)))
        object.__setattr__(self, "peripherals", MappingProxyType(dict(self.peripherals)))
        object.__setattr__(
            self, "power_domains", MappingProxyType(dict(self.power_domains))
        )
        object.__setattr__(self, "units", MappingProxyType(dict(self.units)))
        object.__setattr__(self, "signal_groups", MappingProxyType(dict(self.signal_groups)))
        object.__setattr__(self, "mode_groups", MappingProxyType(dict(self.mode_groups)))
        object.__setattr__(self, "pad_sets", MappingProxyType(dict(self.pad_sets)))
        object.__setattr__(self, "metadata", MappingProxyType(dict(self.metadata)))


@dataclass(frozen=True, slots=True)
class PartDefinition:
    name: str
    pins: Mapping[str, PackagePinDefinition]
    category: str = "component.generic"
    traits: frozenset[str] = frozenset()
    footprints: tuple[str, ...] = ()
    manufacturer: str | None = None
    assembled: bool = True
    device: str | None = None
    source: SourceReference | None = None
    metadata: Mapping[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        # Defensive copies keep an otherwise frozen IR from being mutated via a
        # caller-owned dictionary.
        if not isinstance(self.assembled, bool):
            raise ValueError("part assembled must be a boolean")
        object.__setattr__(self, "pins", MappingProxyType(dict(self.pins)))
        object.__setattr__(self, "traits", frozenset(self.traits))
        object.__setattr__(self, "metadata", MappingProxyType(dict(self.metadata)))


@dataclass(frozen=True, slots=True)
class ComponentInstance:
    ref: str
    part: str
    value: Quantity | str | None = None
    footprint: str | None = None
    properties: Mapping[str, str] = field(default_factory=dict)
    modes: Mapping[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        object.__setattr__(self, "properties", MappingProxyType(dict(self.properties)))
        object.__setattr__(self, "modes", MappingProxyType(dict(self.modes)))


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
    type_name: str
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
    constraint_id: str | None = None
    mode: ConstraintMode = ConstraintMode.REQUIRE
    consumers: tuple[str, ...] = ()
    verifier: str | None = None
    origins: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "parameters", MappingProxyType(dict(self.parameters)))
        object.__setattr__(self, "consumers", tuple(self.consumers))
        object.__setattr__(self, "origins", tuple(self.origins))
        if self.constraint_id is not None and not self.constraint_id:
            raise ValueError("constraint id cannot be empty")
        if self.mode in {ConstraintMode.REQUIRE, ConstraintMode.EXTERNAL}:
            if bool(self.consumers) != (self.verifier is not None):
                raise ValueError(
                    "hard constraint ownership requires both consumers and verifier"
                )


@dataclass(frozen=True, slots=True)
class ModuleInstance:
    """A source-level instance of a reusable module definition."""

    ref: str
    module: str


@dataclass(frozen=True, slots=True)
class PeripheralSignalSelection:
    pin: str
    pad: str | None = None
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
class InterfaceTypeDefinition:
    name: str
    signals: Mapping[str, ElectricalProfile]
    groups: tuple[SignalGroupDefinition, ...] = ()
    validator: str | None = None

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
