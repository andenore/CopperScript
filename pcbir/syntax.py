"""Source-aware syntax tree nodes for CopperScript v0.1."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class SourceLocation:
    filename: str
    offset: int
    line: int
    column: int

    def __str__(self) -> str:
        return f"{self.filename}:{self.line}:{self.column}"


@dataclass(frozen=True, slots=True)
class RawQuantity:
    value: str
    unit: str


Scalar = str | bool | RawQuantity


@dataclass(frozen=True, slots=True)
class ImportDecl:
    location: SourceLocation
    alias: str
    path: str


@dataclass(frozen=True, slots=True)
class ComponentDecl:
    location: SourceLocation
    ref: str
    part: str
    attributes: dict[str, Scalar]


@dataclass(frozen=True, slots=True)
class PortDecl:
    location: SourceLocation
    name: str
    pin_type: str


@dataclass(frozen=True, slots=True)
class PinDecl:
    location: SourceLocation
    name: str
    attributes: dict[str, Scalar]


@dataclass(frozen=True, slots=True)
class PadDecl:
    location: SourceLocation
    name: str
    attributes: dict[str, Scalar]


@dataclass(frozen=True, slots=True)
class PowerDomainDecl:
    location: SourceLocation
    name: str
    attributes: dict[str, Scalar]


@dataclass(frozen=True, slots=True)
class PowerStateDecl:
    location: SourceLocation
    name: str
    rails: dict[str, str]


@dataclass(frozen=True, slots=True)
class PartPropertyDecl:
    location: SourceLocation
    name: str
    value: Scalar


@dataclass(frozen=True, slots=True)
class DevicePropertyDecl:
    location: SourceLocation
    name: str
    value: Scalar


@dataclass(frozen=True, slots=True)
class PeripheralSignalDecl:
    location: SourceLocation
    name: str
    pin_type: str
    attributes: dict[str, Scalar]


@dataclass(frozen=True, slots=True)
class PeripheralDecl:
    location: SourceLocation
    name: str
    kind: str
    signals: tuple[PeripheralSignalDecl, ...]


@dataclass(frozen=True, slots=True)
class MuxDecl:
    location: SourceLocation
    pad: str
    peripheral: str
    signal: str
    attributes: dict[str, Scalar]


@dataclass(frozen=True, slots=True)
class ResourceDecl:
    location: SourceLocation
    name: str


@dataclass(frozen=True, slots=True)
class ConfigurationDecl:
    location: SourceLocation
    component: str
    peripheral: str
    name: str
    signals: dict[str, str]


@dataclass(frozen=True, slots=True)
class ModuleInstanceDecl:
    location: SourceLocation
    ref: str
    module: str


@dataclass(frozen=True, slots=True)
class NetDecl:
    location: SourceLocation
    name: str
    endpoints: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class SupplyDecl:
    location: SourceLocation
    name: str
    attributes: dict[str, Scalar]


@dataclass(frozen=True, slots=True)
class BindingDecl:
    location: SourceLocation
    component: str
    signals: dict[str, str]


@dataclass(frozen=True, slots=True)
class InterfaceDecl:
    location: SourceLocation
    name: str
    kind: str
    attributes: dict[str, Scalar]
    bindings: tuple[BindingDecl, ...]


@dataclass(frozen=True, slots=True)
class ConstraintDecl:
    location: SourceLocation
    kind: str
    targets: tuple[str, ...]
    parameters: dict[str, Scalar]


Declaration = (
    ComponentDecl
    | PortDecl
    | PinDecl
    | PadDecl
    | PowerDomainDecl
    | PowerStateDecl
    | PartPropertyDecl
    | DevicePropertyDecl
    | PeripheralDecl
    | MuxDecl
    | ResourceDecl
    | ConfigurationDecl
    | ModuleInstanceDecl
    | NetDecl
    | SupplyDecl
    | InterfaceDecl
    | ConstraintDecl
)


@dataclass(frozen=True, slots=True)
class Document:
    location: SourceLocation
    kind: str
    name: str
    libraries: tuple[str, ...]
    imports: tuple[ImportDecl, ...]
    declarations: tuple[Declaration, ...]


class CopperScriptError(Exception):
    """A source-located lexical, syntax, or lowering error."""

    def __init__(self, code: str, message: str, location: SourceLocation):
        self.code = code
        self.message = message
        self.location = location
        super().__init__(f"{location}: {code}: {message}")
